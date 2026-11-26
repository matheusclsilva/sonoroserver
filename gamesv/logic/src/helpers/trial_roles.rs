use std::collections::BTreeMap;

use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_fs::echo_info::{self, EchoInfo, Prop};
use sonoro_gamesv_fs::formation_info::FormationRole;
use sonoro_gamesv_fs::role_info::{Reson, RoleInfo, SkillNode};
use sonoro_gamesv_fs::special_item_incr::TRIAL_INCR_MIN;
use sonoro_gamesv_fs::weapon_item::WeaponItem;
use sonoro_proto::enums::{EAttributeType, ESceneRolesGroup, LivingStatus};
use sonoro_proto::pb::{
    EntityAddNotify, EntityRemoveInfo, EntityRemoveNotify, FightRoleInfo, FightRoleInfos,
    GroupFormation, PbGetRoleListNotify, SwitchSceneGroupNotify, UpdateGroupFormationNotify,
};
use sonoro_proto::wire::PbEnum;

use crate::component::entity::small::{ConfigType, EntityType};
use crate::component::player::{
    PlayerBasicComponent, PlayerEchoComponent, PlayerQuestComponent, PlayerRoleComponent,
    PlayerWeaponComponent,
};
use crate::event_queue::EventQueue;
use crate::events::{BuffAdditionEntry, Event};
use crate::helpers::{entity_proto, role as role_helper, role_stats};
use crate::scene::{Scene, now_millis};
use crate::sink::PushSink;
use crate::templates::role_entity_templates;

fn next_trial_incr_id<V>(map: &BTreeMap<i32, V>) -> i32 {
    let mut candidate = TRIAL_INCR_MIN;
    while map.contains_key(&candidate) {
        candidate += 1;
    }
    candidate
}

fn breach_for_role_level(assets: &Assets, breach_id: i32, level: i32) -> i32 {
    let mut best_breach = 0i32;
    for entry in &assets.tables.role_breach.items {
        if entry.BreachGroupId == breach_id && level > entry.MaxLevel {
            best_breach = best_breach.max(entry.BreachLevel + 1);
        }
    }
    best_breach
}

fn breach_for_weapon_level(assets: &Assets, weapon_id: i32, level: i32) -> i32 {
    let Some(conf) = assets.tables.weapon_conf.get_data_by_id(&weapon_id) else {
        return 0;
    };
    let mut best_breach = 0i32;
    for entry in &assets.tables.weapon_breach.items {
        if entry.BreachId == conf.BreachId && level > entry.LevelLimit {
            best_breach = best_breach.max(entry.Level + 1);
        }
    }
    best_breach
}

fn cleanup_trial_data_for_role(
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    trial_id: i32,
) {
    role_comp.remove_equipment_cache(trial_id);
    if let Some(role_val) = role_comp.role_map.remove(&trial_id)
        && role_val.weapon >= TRIAL_INCR_MIN
    {
        weapon_comp.weapon_map.remove(&role_val.weapon);
    }
    role_comp.trial_roles.remove(&trial_id);

    let echo_keys: Vec<i32> = echo_comp
        .echo_map
        .iter()
        .filter_map(|(&k, v)| {
            (k >= TRIAL_INCR_MIN && v.role_id == Some(trial_id)).then_some(k)
        })
        .collect();
    for ek in echo_keys {
        echo_comp.echo_map.remove(&ek);
    }
}

pub fn materialize_trial_role(
    assets: &Assets,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    trial_id: i32,
) -> bool {
    let Some(trial) = assets.tables.trial_role_info.get_data_by_id(&trial_id) else {
        return false;
    };
    let Some(parent_cfg) = assets.tables.resolve_base_role_info(trial_id) else {
        return false;
    };

    if role_comp.role_map.contains_key(&trial_id) && !role_comp.is_trial_role(trial_id) {
        return false;
    }
    if role_comp.is_trial_role(trial_id) && role_comp.role_map.contains_key(&trial_id) {
        return true;
    }

    let tw_opt = assets
        .tables
        .trial_weapon_info
        .get_data_by_id(&trial.TrailWeapon);
    let weapon_cfg_id = tw_opt
        .as_ref()
        .filter(|tw| tw.WeaponId != 0)
        .map_or(parent_cfg.InitWeaponItemId, |tw| tw.WeaponId);
    let weapon_level = tw_opt.as_ref().map_or(1, |tw| tw.WeaponLevel.max(1));
    let weapon_reson = tw_opt
        .as_ref()
        .map_or(1, |tw| tw.WeaponResonanceLevel.max(1));
    let weapon_skin = tw_opt.as_ref().map_or(0, |tw| tw.WeaponSkinId);
    let weapon_incr_id = next_trial_incr_id(&weapon_comp.weapon_map);

    weapon_comp.weapon_map.insert(
        weapon_incr_id,
        WeaponItem {
            id: weapon_cfg_id,
            level: weapon_level,
            breach: breach_for_weapon_level(assets, weapon_cfg_id, weapon_level),
            reson_level: weapon_reson,
            func_value: WeaponItem::default_func_value(assets, weapon_cfg_id),
            role_id: Some(trial_id),
            ..Default::default()
        },
    );

    for (slot_idx, entry) in trial.PhantomEquipList.iter().take(5).enumerate() {
        if entry.Item1 == 0 {
            continue;
        }
        let echo_incr_id = next_trial_incr_id(&echo_comp.echo_map);
        let tpp_opt = assets
            .tables
            .trail_phantom_prop
            .get_data_by_id(&entry.Item2);

        let mut main_prop = Vec::new();
        let mut sub_prop = Vec::new();
        let mut level = 0;
        let mut fetter_group_id = 0;
        let mut skin_id = 0;

        if let Some(tpp) = tpp_opt {
            level = tpp.Level.max(0);
            fetter_group_id = tpp.FetterGroupId;
            skin_id = tpp.SkinItemId;

            for &mp_id in &tpp.MainProps {
                if let Some(pmp) = assets.tables.phantom_main_prop_item.get_data_by_id(&mp_id) {
                    main_prop.push(Prop {
                        id: pmp.Id,
                        value: echo_info::main_prop_value(assets, &pmp, level),
                    });
                } else if let Some(tppi) =
                    assets.tables.trial_phantom_prop_item.get_data_by_id(&mp_id)
                {
                    main_prop.push(Prop {
                        id: tppi.Id,
                        value: tppi.Prop.Value,
                    });
                }
            }
            if main_prop.is_empty() && tpp.MainProp.Id != 0 {
                main_prop.push(Prop {
                    id: tpp.MainProp.Id,
                    value: tpp.MainProp.Value,
                });
            }

            for &sp_id in &tpp.SubPropList {
                if let Some(tppi) = assets.tables.trial_phantom_prop_item.get_data_by_id(&sp_id) {
                    sub_prop.push(Prop {
                        id: tppi.Id,
                        value: tppi.Prop.Value,
                    });
                }
            }
        }

        echo_comp.echo_map.insert(
            echo_incr_id,
            EchoInfo {
                id: entry.Item1,
                level,
                fetter_group_id,
                skin_id,
                role_id: Some(trial_id),
                equipped_pos: slot_idx as i32,
                main_prop,
                sub_prop,
                ..Default::default()
            },
        );
    }

    let level = trial.Level.max(1);
    let breach = breach_for_role_level(assets, parent_cfg.BreachId, level);

    let mut nodes = Vec::new();
    let mut skills = Vec::new();
    let skill_lvl = trial.UnlockSkillLevel.max(1);
    for node in &assets.tables.skill_tree.items {
        if node.NodeGroup != parent_cfg.SkillTreeGroupId {
            continue;
        }
        let default_active = node.Condition.is_empty() && node.UnLockCondition == 0;
        let active = default_active || trial.UnlockSkillNodeList.contains(&node.NodeIndex);
        nodes.push(SkillNode {
            node_id: node.Id,
            active,
            skill_id: node.SkillId,
        });
        if active && node.SkillId != 0 {
            skills.push((node.SkillId, skill_lvl));
        }
    }

    let mut reson = Vec::new();
    for chain in &assets.tables.resonant_chain.items {
        if chain.GroupId == parent_cfg.ResonantChainGroupId {
            reson.push(Reson {
                id: chain.Id,
                open: chain.GroupIndex <= trial.ResonanceLevel,
                increase: 0,
            });
        }
    }

    let base_prop = RoleInfo::compute_scaled_base_props(assets, parent_cfg.Id, level, breach)
        .unwrap_or_else(|| vec![0i32; EAttributeType::MAX as usize]);

    let role_info = RoleInfo {
        level,
        breakthrough: breach,
        star: parent_cfg.QualityId.max(4),
        weapon: weapon_incr_id,
        role_skin_id: if trial.RoleSkin != 0 {
            trial.RoleSkin
        } else {
            parent_cfg.SkinId
        },
        weapon_skin_id: weapon_skin,
        skill_branch: if trial.DefaultSkillBranchId != 0 {
            trial.DefaultSkillBranchId
        } else {
            parent_cfg.DefaultSkillBranchId
        },
        resonant_chain_group_index: trial.ResonanceLevel,
        base_prop,
        skills,
        skill_node_state: nodes,
        reson,
        ..Default::default()
    };

    role_comp.role_map.insert(trial_id, role_info);
    role_comp.trial_roles.insert(trial_id);
    true
}

pub fn materialize_active_trial_roles(
    assets: &Assets,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    trial_ids: &[i32],
) {
    for &rid in trial_ids {
        let _ = materialize_trial_role(assets, role_comp, weapon_comp, echo_comp, rid);
    }
}

pub fn sanitize_formation_for_login(
    formation_info: &mut sonoro_gamesv_fs::formation_info::FormationInfo,
    role_comp: &PlayerRoleComponent,
    basic_comp: &PlayerBasicComponent,
    active_trial_ids: &[i32],
) {
    let cur_idx = formation_info
        .cur_formation
        .clamp(0, (formation_info.formations.len() - 1) as i32) as usize;
    for form in &mut formation_info.formations {
        let mut any_valid = false;
        let mut has_cur = false;
        for maybe_slot in &mut form.roles {
            if let Some(slot) = maybe_slot {
                let is_trial_allowed = active_trial_ids.contains(&slot.role_id);
                let valid_owned = role_comp.role_map.contains_key(&slot.role_id)
                    && (!role_comp.is_trial_role(slot.role_id) || is_trial_allowed);
                if !valid_owned {
                    *maybe_slot = None;
                    continue;
                }
                any_valid = true;
                if slot.role_id == form.cur_role {
                    has_cur = true;
                }
            }
        }
        if any_valid && !has_cur
            && let Some(first_slot) = form.roles.iter().flatten().next()
        {
            form.cur_role = first_slot.role_id;
        }
    }

    let cur_form = &mut formation_info.formations[cur_idx];
    if cur_form.roles.iter().all(|r| r.is_none()) {
        let fallback_id = if basic_comp.info.selected_main_role_id != 0
            && role_comp
                .role_map
                .contains_key(&basic_comp.info.selected_main_role_id)
        {
            basic_comp.info.selected_main_role_id
        } else {
            role_comp
                .role_map
                .keys()
                .copied()
                .find(|&rid| !role_comp.is_trial_role(rid))
                .unwrap_or(1502)
        };
        cur_form.roles[0] = Some(FormationRole {
            role_id: fallback_id,
            entity_id: -1,
            on_stage_without_control: false,
        });
        cur_form.cur_role = fallback_id;
    }
}

pub fn push_trial_role_list<S: PushSink + ?Sized>(
    sink: &mut S,
    assets: &Assets,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &PlayerWeaponComponent,
    echo_comp: &PlayerEchoComponent,
    role_ids: &[i32],
) -> Result<(), &'static str> {
    let mut list = Vec::new();
    for &rid in role_ids {
        let Some(role) = role_comp.role_map.get(&rid).cloned() else {
            continue;
        };
        list.push(role_stats::to_client_role_info(
            assets,
            role_comp,
            rid,
            &role,
            weapon_comp,
            echo_comp,
        )?);
    }
    if !list.is_empty() {
        sink.push(
            PbGetRoleListNotify::MSG_ID,
            &PbGetRoleListNotify { RoleList: list },
        );
    }
    Ok(())
}

pub fn save_pre_trial_formation(quest_comp: &mut PlayerQuestComponent, scene: &Scene) {
    if !quest_comp.info.env.pre_trial_roles.is_empty() {
        return;
    }
    let Ok(cur_idx) = usize::try_from(scene.formation_info.cur_formation) else {
        return;
    };
    let Some(cur_form) = scene.formation_info.formations.get(cur_idx) else {
        return;
    };
    let saved: Vec<i32> = cur_form.roles.iter().flatten().map(|s| s.role_id).collect();
    if !saved.is_empty() {
        quest_comp.info.env.pre_trial_roles = saved;
        quest_comp.info.env.pre_trial_cur_role = cur_form.cur_role;
    }
}

pub fn clear_pre_trial_formation(quest_comp: &mut PlayerQuestComponent) {
    quest_comp.info.env.pre_trial_roles.clear();
    quest_comp.info.env.pre_trial_cur_role = 0;
}

pub fn cleanup_removed_trial_roles(
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    removed_ids: &[i32],
) {
    for &rid in removed_ids {
        cleanup_trial_data_for_role(role_comp, weapon_comp, echo_comp, rid);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_trial_roles_to_scene_formation<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    events: &mut EventQueue,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &PlayerWeaponComponent,
    echo_comp: &PlayerEchoComponent,
    trial_ids: &[i32],
    create_temp_team: bool,
    auto_change: bool,
) -> Result<(), &'static str> {
    if trial_ids.is_empty() {
        return Ok(());
    }
    let Ok(cur_idx) = usize::try_from(scene.formation_info.cur_formation) else {
        return Ok(());
    };
    if cur_idx >= scene.formation_info.formations.len() {
        return Ok(());
    }

    let mut target_ids = Vec::new();
    for &rid in trial_ids {
        if target_ids.len() >= 3 {
            break;
        }
        if role_comp.role_map.contains_key(&rid) && !target_ids.contains(&rid) {
            target_ids.push(rid);
        }
    }
    if !create_temp_team {
        for slot in scene.formation_info.formations[cur_idx].roles.iter().flatten() {
            if target_ids.len() >= 3 {
                break;
            }
            if !role_comp.role_map.contains_key(&slot.role_id)
                || target_ids.contains(&slot.role_id)
                || conflicts_with_target_roles(assets, &target_ids, slot.role_id)
            {
                continue;
            }
            target_ids.push(slot.role_id);
        }
    }
    if target_ids.is_empty() {
        return Ok(());
    }

    let cur_role = scene.formation_info.formations[cur_idx].cur_role;
    let desired_cur = if auto_change || !target_ids.contains(&cur_role) {
        target_ids[0]
    } else {
        cur_role
    };

    sync_formation_entities(
        scene,
        fs,
        assets,
        sink,
        events,
        role_comp,
        weapon_comp,
        echo_comp,
        &target_ids,
        desired_cur,
    )
}

fn canonical_role_family(assets: &Assets, role_id: i32) -> i32 {
    let base_id = assets
        .tables
        .trial_role_info
        .get_data_by_id(&role_id)
        .filter(|t| t.ParentId > 0)
        .map_or(role_id, |t| t.ParentId);
    match base_id {
        1406 | 1408 | 1501 | 1502 | 1604 | 1605 => 1501,
        _ => base_id,
    }
}

fn conflicts_with_target_roles(assets: &Assets, targets: &[i32], candidate: i32) -> bool {
    let cand_fam = canonical_role_family(assets, candidate);
    targets
        .iter()
        .any(|&tid| canonical_role_family(assets, tid) == cand_fam)
}

#[allow(clippy::too_many_arguments)]
pub fn restore_formation_after_trial_removal<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    events: &mut EventQueue,
    basic_comp: &PlayerBasicComponent,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &PlayerWeaponComponent,
    echo_comp: &PlayerEchoComponent,
    quest_comp: &mut PlayerQuestComponent,
    removed_ids: &[i32],
    remaining_trial_ids: &[i32],
) -> Result<(), &'static str> {
    let Ok(cur_idx) = usize::try_from(scene.formation_info.cur_formation) else {
        return Ok(());
    };
    if cur_idx >= scene.formation_info.formations.len() {
        return Ok(());
    }

    let mut next_ids = Vec::new();
    for slot in scene.formation_info.formations[cur_idx].roles.iter().flatten() {
        if removed_ids.contains(&slot.role_id) {
            continue;
        }
        if role_comp.is_trial_role(slot.role_id) && !remaining_trial_ids.contains(&slot.role_id) {
            continue;
        }
        if !role_comp.role_map.contains_key(&slot.role_id) {
            continue;
        }
        if !next_ids.contains(&slot.role_id) {
            next_ids.push(slot.role_id);
        }
    }

    if remaining_trial_ids.is_empty() {
        for &rid in &quest_comp.info.env.pre_trial_roles {
            if next_ids.len() >= 3 {
                break;
            }
            if !role_comp.role_map.contains_key(&rid) || role_comp.is_trial_role(rid) {
                continue;
            }
            if !next_ids.contains(&rid) {
                next_ids.push(rid);
            }
        }
    }

    if next_ids.is_empty() {
        if basic_comp.info.selected_main_role_id != 0
            && role_comp
                .role_map
                .contains_key(&basic_comp.info.selected_main_role_id)
        {
            next_ids.push(basic_comp.info.selected_main_role_id);
        } else if let Some(&rid) = role_comp
            .role_map
            .keys()
            .find(|&&rid| !role_comp.is_trial_role(rid))
        {
            next_ids.push(rid);
        }
    }
    if next_ids.is_empty() {
        return Ok(());
    }

    let cur_role = scene.formation_info.formations[cur_idx].cur_role;
    let desired_cur = if remaining_trial_ids.is_empty()
        && next_ids.contains(&quest_comp.info.env.pre_trial_cur_role)
    {
        quest_comp.info.env.pre_trial_cur_role
    } else if next_ids.contains(&cur_role) {
        cur_role
    } else {
        next_ids[0]
    };

    sync_formation_entities(
        scene,
        fs,
        assets,
        sink,
        events,
        role_comp,
        weapon_comp,
        echo_comp,
        &next_ids,
        desired_cur,
    )?;
    if remaining_trial_ids.is_empty() {
        clear_pre_trial_formation(quest_comp);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn sync_formation_entities<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    events: &mut EventQueue,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &PlayerWeaponComponent,
    echo_comp: &PlayerEchoComponent,
    target_role_ids: &[i32],
    desired_cur_role: i32,
) -> Result<(), &'static str> {
    let cur_idx = scene.formation_info.cur_formation as usize;
    let old_roles = scene.formation_info.formations[cur_idx].roles.clone();

    let already_synced = {
        let cur_form = &scene.formation_info.formations[cur_idx];
        let old_count = old_roles.iter().filter(|r| r.is_some()).count();
        let expected_count = target_role_ids.len().min(3);
        cur_form.cur_role == desired_cur_role
            && old_count == expected_count
            && target_role_ids[..expected_count]
                .iter()
                .enumerate()
                .all(|(i, &rid)| {
                    old_roles[i].as_ref().is_some_and(|old| {
                        old.role_id == rid
                            && old.entity_id != -1
                            && scene.net_id_map.contains_key(&old.entity_id)
                    })
                })
    };
    if already_synced {
        return Ok(());
    }

    let mut remove_infos = Vec::new();
    for old in old_roles.iter().flatten() {
        if target_role_ids.contains(&old.role_id) {
            continue;
        }
        if old.entity_id != -1
            && let Some(&entity_index) = scene.net_id_map.get(&old.entity_id)
        {
            if let Some(concomitant) = scene.entities[entity_index].concomitant.clone() {
                for cid in concomitant.vision_entity_id {
                    let _ = scene.remove(fs, cid);
                    remove_infos.push(EntityRemoveInfo {
                        EntityId: cid,
                        ..Default::default()
                    });
                }
                for cid in concomitant.custom_entity_ids {
                    let _ = scene.remove(fs, cid);
                    remove_infos.push(EntityRemoveInfo {
                        EntityId: cid,
                        ..Default::default()
                    });
                }
            }
            let _ = scene.remove(fs, old.entity_id);
            remove_infos.push(EntityRemoveInfo {
                EntityId: old.entity_id,
                ..Default::default()
            });
        }
    }
    if !remove_infos.is_empty() {
        sink.push(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                IsRemove: true,
                RemoveInfos: remove_infos,
            },
        );
    }

    let mut new_roles: [Option<FormationRole>; 3] = [None, None, None];
    for (idx, &rid) in target_role_ids.iter().take(3).enumerate() {
        let kept_eid = old_roles
            .iter()
            .flatten()
            .find(|old| old.role_id == rid)
            .map_or(-1, |old| old.entity_id);
        new_roles[idx] = Some(FormationRole {
            role_id: rid,
            entity_id: kept_eid,
            on_stage_without_control: false,
        });
    }
    scene.formation_info.formations[cur_idx].roles = new_roles;
    scene.formation_info.formations[cur_idx].cur_role = desired_cur_role;

    let Some(instance_dungeon) = assets.tables.instance_dungeon.get_data_by_id(&scene.instance_id)
    else {
        return Ok(());
    };
    let now_ms = now_millis();

    let mut new_role_net_ids = Vec::new();
    for slot_idx in 0..3 {
        let Some(slot_snap) = scene.formation_info.formations[cur_idx].roles[slot_idx].clone()
        else {
            continue;
        };
        if slot_snap.entity_id == -1 || !scene.net_id_map.contains_key(&slot_snap.entity_id) {
            let entity = role_entity_templates::create_role_entity(
                fs,
                scene,
                scene.player_id,
                assets,
                role_comp,
                weapon_comp,
                echo_comp,
                &instance_dungeon,
                slot_snap.role_id,
            )?;
            if let Some(slot) = scene.formation_info.formations[cur_idx].roles[slot_idx].as_mut() {
                slot.entity_id = entity.net_id;
            }
            new_role_net_ids.push(entity.net_id);
        }
    }

    for e in &mut scene.entities {
        if e.config.entity_type == EntityType::Player && e.config.config_type == ConfigType::Character
        {
            e.visible = e.config.config_id == desired_cur_role;
        }
    }

    let mut add_role_pbs = Vec::new();
    let mut add_concom_pbs = Vec::new();
    for net_id in new_role_net_ids {
        if !scene.net_id_map.contains_key(&net_id) {
            continue;
        }
        add_role_pbs.push(entity_proto::build_optional(
            assets, scene, None, net_id, now_ms,
        )?);
        let concomitant_opt = scene.entity(net_id).and_then(|s| s.concomitant.clone());
        if let Some(concomitant) = concomitant_opt {
            for cid in concomitant
                .vision_entity_id
                .into_iter()
                .chain(concomitant.custom_entity_ids)
            {
                if scene.net_id_map.contains_key(&cid) {
                    add_concom_pbs.push(entity_proto::build_optional(
                        assets, scene, None, cid, now_ms,
                    )?);
                }
            }
        }
    }

    if !add_role_pbs.is_empty() {
        sink.push(
            EntityAddNotify::MSG_ID,
            &EntityAddNotify {
                EntityPbs: add_role_pbs,
                RemoveTagIds: false,
            },
        );
    }
    if !add_concom_pbs.is_empty() {
        sink.push(
            EntityAddNotify::MSG_ID,
            &EntityAddNotify {
                EntityPbs: add_concom_pbs,
                RemoveTagIds: false,
            },
        );
    }

    let cur_form = &scene.formation_info.formations[cur_idx];
    let fight_role_infos: Vec<FightRoleInfo> = cur_form
        .roles
        .iter()
        .flatten()
        .map(|slot| FightRoleInfo {
            RoleId: slot.role_id,
            EntityId: slot.entity_id,
            OnStageWithoutControl: false,
        })
        .collect();
    sink.push(
        UpdateGroupFormationNotify::MSG_ID,
        &UpdateGroupFormationNotify {
            GroupFormation: vec![GroupFormation {
                PlayerId: scene.player_id,
                FightRoleInfos: vec![FightRoleInfos {
                    GroupType: 1,
                    FightRoleInfos: fight_role_infos,
                    CurRole: cur_form.cur_role,
                    LivingStatus: Some(role_helper::formation_living_status(scene, cur_form)),
                    IsFixedLocation: false,
                }],
                CurrentGroupType: 1,
            }],
        },
    );

    let _ = scene.save(fs);
    events.enqueue(Event::UpdateFormations {
        player_id: scene.player_id,
        formation_id: scene.formation_info.cur_formation,
        role_ids: target_role_ids.to_vec(),
    });
    Ok(())
}

/// Roles of a `PhantomFormation` row. Each is a TrialRoleInfo id whose parent RoleInfo has
/// RoleType 2 (Id 1 = [5001001], parent 5001).
pub fn phantom_formation_roles(assets: &Assets, formation_id: i32) -> Vec<i32> {
    assets
        .read_raw_json_array("PhantomFormation")
        .iter()
        .find(|row| row.get("Id").and_then(|v| v.as_i64()) == Some(i64::from(formation_id)))
        .and_then(|row| row.get("Roles"))
        .and_then(|v| v.as_array())
        .map(|roles| {
            roles
                .iter()
                .filter_map(|v| v.as_i64().and_then(|id| i32::try_from(id).ok()))
                .filter(|&id| id > 0)
                .collect()
        })
        .unwrap_or_default()
}

fn concomitant_ids(scene: &Scene, entity_id: i64) -> Vec<i64> {
    scene
        .entity(entity_id)
        .and_then(|s| s.concomitant.as_ref())
        .map(|c| c.vision_entity_id.iter().chain(&c.custom_entity_ids).copied().collect())
        .unwrap_or_default()
}

fn remove_role_entities(
    scene: &mut Scene,
    fs: &FileSystem,
    entity_ids: &[i64],
) -> Vec<EntityRemoveInfo> {
    let mut remove_infos = Vec::new();
    for &eid in entity_ids {
        for id in concomitant_ids(scene, eid).into_iter().chain([eid]) {
            if scene.net_id_map.contains_key(&id) {
                let _ = scene.remove(fs, id);
                remove_infos.push(EntityRemoveInfo {
                    EntityId: id,
                    ..Default::default()
                });
            }
        }
    }
    remove_infos
}

/// ChangePhantomFormation (server action). The client action only waits until its scene team
/// is the Phantom group (LevelEventChangePhantomFormation: SceneTeamModel.IsPhantomTeam, set in
/// SceneTeamModel.SwitchGroupInternal from UpdateSceneGroupDataNotify.CurrentGroup or
/// SwitchSceneGroupNotify.Group). TeamGoBattle then waits for the group's role entities, so the
/// formation's roles are spawned as role entities first, like trial roles. `AppendBuffIds` go
/// on those roles. Does nothing while the Phantom group is in use, like the client's own
/// SceneTeamController.ChangePhantomTeam.
#[allow(clippy::too_many_arguments)]
pub fn change_phantom_formation<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    events: &mut EventQueue,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    formation_id: i32,
    append_buff_ids: &[i64],
) -> Result<(), &'static str> {
    if !scene.phantom_role_entity_ids.is_empty() {
        return Ok(());
    }
    let role_ids: Vec<i32> = phantom_formation_roles(assets, formation_id)
        .into_iter()
        .filter(|&rid| materialize_trial_role(assets, role_comp, weapon_comp, echo_comp, rid))
        .collect();
    let Some(&cur_role) = role_ids.first() else {
        return Ok(());
    };
    let instance_dungeon = assets
        .tables
        .instance_dungeon
        .get_data_by_id(&scene.instance_id)
        .or_else(|| assets.tables.instance_dungeon.get_data_by_id(&8))
        .ok_or("InstanceDungeonNotFound")?;
    push_trial_role_list(sink, assets, role_comp, weapon_comp, echo_comp, &role_ids)?;

    let now_ms = now_millis();
    let mut entity_ids = Vec::new();
    let mut add_role_pbs = Vec::new();
    let mut add_concom_pbs = Vec::new();
    let mut fight_role_infos = Vec::new();
    for &rid in &role_ids {
        let mut spawn = || -> Result<_, &'static str> {
            let entity = role_entity_templates::create_role_entity(
                fs,
                scene,
                scene.player_id,
                assets,
                role_comp,
                weapon_comp,
                echo_comp,
                &instance_dungeon,
                rid,
            )?;
            entity_ids.push(entity.net_id);
            if let Some(s) = scene.entity_mut(entity.net_id) {
                s.visible = rid == cur_role;
            }
            add_role_pbs.push(entity_proto::build_optional(
                assets,
                scene,
                None,
                entity.net_id,
                now_ms,
            )?);
            for cid in concomitant_ids(scene, entity.net_id) {
                add_concom_pbs.push(entity_proto::build_optional(assets, scene, None, cid, now_ms)?);
            }
            Ok(entity)
        };
        let entity = match spawn() {
            Ok(entity) => entity,
            Err(err) => {
                let _ = remove_role_entities(scene, fs, &entity_ids);
                return Err(err);
            }
        };
        fight_role_infos.push(FightRoleInfo {
            RoleId: rid,
            EntityId: entity.net_id,
            OnStageWithoutControl: false,
        });
        let buffs: Vec<BuffAdditionEntry> = append_buff_ids
            .iter()
            .filter(|&&id| id > 0)
            .map(|&id| BuffAdditionEntry {
                id,
                stack_count: 1,
                is_active: true,
                ..Default::default()
            })
            .collect();
        if !buffs.is_empty() {
            events.enqueue(Event::BuffAddition {
                target: entity,
                instigator: entity,
                buffs,
            });
        }
    }
    scene.phantom_role_entity_ids = entity_ids;

    sink.push(
        EntityAddNotify::MSG_ID,
        &EntityAddNotify {
            EntityPbs: add_role_pbs,
            RemoveTagIds: false,
        },
    );
    if !add_concom_pbs.is_empty() {
        sink.push(
            EntityAddNotify::MSG_ID,
            &EntityAddNotify {
                EntityPbs: add_concom_pbs,
                RemoveTagIds: false,
            },
        );
    }
    let phantom = ESceneRolesGroup::Phantom.to_i32();
    sink.push(
        UpdateGroupFormationNotify::MSG_ID,
        &UpdateGroupFormationNotify {
            GroupFormation: vec![GroupFormation {
                PlayerId: scene.player_id,
                FightRoleInfos: vec![FightRoleInfos {
                    GroupType: phantom,
                    FightRoleInfos: fight_role_infos,
                    CurRole: cur_role,
                    LivingStatus: Some(LivingStatus::Alive),
                    IsFixedLocation: false,
                }],
                CurrentGroupType: phantom,
            }],
        },
    );
    sink.push(
        SwitchSceneGroupNotify::MSG_ID,
        &SwitchSceneGroupNotify {
            PlayerId: scene.player_id,
            Group: phantom,
        },
    );
    Ok(())
}

/// RestorePhantomFormation (server action): back to the Battle group, then the Echo role
/// entities and their trial role data go. The client's own SceneTeamController.RevertPhantomTeam
/// also only switches to Battle, and only while the Phantom group is in use. The current role
/// goes back to the battle formation's cur_role. Trial roles in `keep_trial_ids` (from
/// AddTrialCharacter) keep their data.
pub fn restore_phantom_formation<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    sink: &mut S,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    keep_trial_ids: &[i32],
) {
    let entity_ids = std::mem::take(&mut scene.phantom_role_entity_ids);
    if entity_ids.is_empty() {
        return;
    }
    if std::mem::take(&mut scene.phantom_cur_role) > 0 {
        let battle_eid = scene.current_role_entity_id();
        if let Some(s) = scene.entity_mut(battle_eid) {
            s.visible = true;
        }
    }
    sink.push(
        SwitchSceneGroupNotify::MSG_ID,
        &SwitchSceneGroupNotify {
            PlayerId: scene.player_id,
            Group: ESceneRolesGroup::Battle.to_i32(),
        },
    );
    let role_ids: Vec<i32> = entity_ids
        .iter()
        .filter_map(|&eid| scene.entity(eid).map(|s| s.config.config_id))
        .filter(|rid| role_comp.is_trial_role(*rid) && !keep_trial_ids.contains(rid))
        .collect();
    let remove_infos = remove_role_entities(scene, fs, &entity_ids);
    if !remove_infos.is_empty() {
        sink.push(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                IsRemove: true,
                RemoveInfos: remove_infos,
            },
        );
    }
    cleanup_removed_trial_roles(role_comp, weapon_comp, echo_comp, &role_ids);
}
