use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::tables::resonant_chain::ResonantChain;
use sonoro_gamesv_data::tables::role_breach::RoleBreach;
use sonoro_gamesv_data::tables::role_info::RoleInfo as RoleConfig;
use sonoro_gamesv_data::tables::skill_level::SkillLevel;
use sonoro_gamesv_fs::account::Account;
use sonoro_gamesv_fs::inventory_info::InventoryInfo;
use sonoro_gamesv_fs::role_info::{RoleInfo, SkillNode};
use sonoro_gamesv_logic::component::comp_util;
use sonoro_gamesv_logic::component::player::{PlayerEchoComponent, PlayerWeaponComponent};
use sonoro_gamesv_logic::event_queue::EventQueue;
use sonoro_gamesv_logic::events::{BuffAdditionEntry, EntityHandle, Event};
use sonoro_gamesv_logic::helpers::{
    condition, consume, quest_engine, reward, role as role_helper, role_attribute_sync, role_stats,
};
use sonoro_gamesv_logic::scene::Scene;
use sonoro_proto::enums::{EquipPos, ErrorCode, FavorItemStatus};
use sonoro_proto::pb::{
    ArrayIntDouble, ArrayIntInt, ArraySkillNode, CharacterDeathRequest, CharacterDeathResponse,
    ClientCurrentRoleReportPush, EquipTakeOnNotify, FavorItem, LogoutNotify, NormalItem,
    NormalItemUpdateNotify, PbGetRoleListRequest, PbGetRoleListResponse, PbOverRoleRequest,
    PbOverRoleResponse, PbUpLevelRoleRequest, PbUpLevelRoleResponse, PbUpLevelSkillRequest,
    PbUpLevelSkillResponse, PhantomPutOnNotify, PlayerAttr, PlayerAttrNotify,
    PlayerAttrValueOneof, ResonantChainUnlockRequest, ResonantChainUnlockResponse,
    RoleActivateSkillRequest, RoleActivateSkillResponse, RoleBreakThroughViewRequest,
    RoleBreakThroughViewResponse, RoleChangeNotify, RoleChangeUnlockNotify,
    RoleElementChangeRequest, RoleElementChangeResponse, RoleFavor, RoleFavorListRequest,
    RoleFavorListResponse, RoleLevelUpViewRequest, RoleLevelUpViewResponse, RoleLoadEquipData,
    RolePropInfo, RolePropRequest, RolePropResponse, RoleSexChangeRequest, RoleSexChangeResponse,
    RoleSkillBranchModifyRequest, RoleSkillBranchModifyResponse, RoleSkillLevelUpViewRequest,
    RoleSkillLevelUpViewResponse, RoleSkillNodeNotify, RoleSkillQuickLevelUpRequest,
    RoleSkillQuickLevelUpResponse, RoleSkillViewRequest, RoleSkillViewResponse, RoleVoice,
    SwitchRoleRequest, SwitchRoleResponse, PlayerRoleVoiceSetRequest,
    PlayerRoleVoiceSetResponse, PlayerVoiceLanguageRequest, PlayerVoiceLanguageResponse, roleInfo,
};
use sonoro_proto::enums::{PlayerAttrKey, PlayerAttrType};

use super::{State, echo};
use sonoro_gamesv_logic::sink::PushSink;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn save_role_immediate(
    fs: &sonoro_common::fs::FileSystem,
    player_id: i32,
    role_id: i32,
    role: &RoleInfo,
) -> Result<(), String> {
    let path = format!("player/{player_id}/{}/{role_id}", RoleInfo::DATA_DIR);
    comp_util::save_struct(fs, role, &path).map_err(|e| e.to_string())
}

fn find_skill_level_config(
    assets: &Assets,
    skill_level_group_id: i32,
    target_level: i32,
) -> Option<SkillLevel> {
    assets
        .tables
        .skill_level
        .items
        .iter()
        .find(|entry| entry.SkillLevelGroupId == skill_level_group_id && entry.SkillId == target_level)
        .cloned()
}

fn is_skill_node_active(role: &RoleInfo, node_id: i32) -> bool {
    role.skill_node_state
        .iter()
        .any(|entry| entry.node_id == node_id && entry.active)
}

fn find_role_breach_entry(
    assets: &Assets,
    breach_group_id: i32,
    target_breach: i32,
) -> Option<RoleBreach> {
    assets
        .tables
        .role_breach
        .items
        .iter()
        .find(|entry| entry.BreachGroupId == breach_group_id && entry.BreachLevel == target_breach)
        .cloned()
}

fn is_valid_skill_branch_for_role(
    assets: &Assets,
    role_config: &RoleConfig,
    role_id: i32,
    branch_id: i32,
) -> bool {
    if branch_id == 0 {
        return role_config.DefaultSkillBranchId == 0;
    }
    if role_config.SkillBranchIds.contains(&branch_id) {
        return true;
    }
    assets
        .tables
        .skill_branch_buff
        .get_data_by_id(&branch_id)
        .is_some_and(|branch_data| branch_data.RoleId == role_id)
}

fn build_preview_final_props(
    assets: &Assets,
    role_id: i32,
    role: &RoleInfo,
    preview_level: i32,
    preview_breakthrough: i32,
    weapon_comp: &PlayerWeaponComponent,
    echo_comp: &PlayerEchoComponent,
) -> Vec<ArrayIntDouble> {
    let mut tmp_role = role.clone();
    tmp_role.level = preview_level;
    tmp_role.breakthrough = preview_breakthrough;
    if let Some(scaled) =
        RoleInfo::compute_scaled_base_props(assets, role_id, preview_level, preview_breakthrough)
    {
        tmp_role.base_prop = scaled;
    }

    let snapshot =
        role_stats::build_preview_snapshot(assets, role_id, &tmp_role, weapon_comp, echo_comp);
    snapshot
        .base_props
        .iter()
        .enumerate()
        .map(|(i, &base_val)| {
            let add_val = snapshot.add_props.get(i).copied().unwrap_or(0);
            ArrayIntDouble {
                Key: i as i32,
                Value: f64::from(base_val + add_val),
            }
        })
        .collect()
}

fn push_weapon_equip_notify(state: &mut State, role_id: i32, weapon_id: i32) {
    if weapon_id == 0 {
        return;
    }
    state.conn.push(
        EquipTakeOnNotify::MSG_ID,
        &EquipTakeOnNotify {
            DataList: vec![RoleLoadEquipData {
                RoleId: role_id,
                Pos: Some(EquipPos::Weapon),
                EquipIncId: weapon_id,
            }],
        },
    );
}

fn push_phantom_equip_notify(state: &mut State, role_id: i32) -> Result<(), String> {
    let changed = [role_id];
    echo::shared::push_role_prop_update(state, &changed)?;
    let equip_info_list =
        echo::shared::changed_equip_info_list(&state.player_components.echo, &changed);
    state.conn.push(
        PhantomPutOnNotify::MSG_ID,
        &PhantomPutOnNotify {
            EquipInfoList: equip_info_list,
        },
    );
    Ok(())
}

pub fn handle_role_favor_list_request(
    state: &mut State,
    _req: RoleFavorListRequest,
) -> Result<RoleFavorListResponse, String> {
    let favor_list = state
        .player_components
        .role
        .role_map
        .keys()
        .map(|&role_id| {
            let word_ids = state
                .assets
                .tables
                .favor_word
                .items
                .iter()
                .filter(|w| w.RoleId == role_id)
                .map(|w| FavorItem {
                    Id: w.Id,
                    Status: Some(FavorItemStatus::ItemUnLocked),
                })
                .collect();
            let story_ids = state
                .assets
                .tables
                .favor_story
                .items
                .iter()
                .filter(|s| s.RoleId == role_id)
                .map(|s| FavorItem {
                    Id: s.Id,
                    Status: Some(FavorItemStatus::ItemUnLocked),
                })
                .collect();
            let goods_ids = state
                .assets
                .tables
                .favor_goods
                .items
                .iter()
                .filter(|g| g.RoleId == role_id)
                .map(|g| FavorItem {
                    Id: g.Id,
                    Status: Some(FavorItemStatus::ItemUnLocked),
                })
                .collect();

            RoleFavor {
                RoleId: role_id,
                Level: 5,
                Exp: 16800,
                WordIds: word_ids,
                StoryIds: story_ids,
                GoodsIds: goods_ids,
                FavorQuest: None,
            }
        })
        .collect();

    Ok(RoleFavorListResponse {
        ErrorCode: Some(ErrorCode::Success),
        FavorList: favor_list,
    })
}

pub fn handle_role_sex_change_request(
    state: &mut State,
    events: &mut EventQueue,
    req: RoleSexChangeRequest,
) -> Result<RoleSexChangeResponse, String> {
    let now_ms_val = now_ms();
    let target_gender = req.Sex;
    let current_gender = state.player_components.basic.info.attributes.sex;

    if target_gender == current_gender {
        return Ok(RoleSexChangeResponse {
            ErrorCode: Some(ErrorCode::Success),
            Sex: current_gender,
        });
    }

    let source_role_id = state
        .scene
        .as_ref()
        .and_then(|s| role_helper::current_formation_main_role_id(&state.assets, s, current_gender))
        .or_else(|| {
            role_helper::selected_main_role_id(
                &state.assets,
                &state.player_components.role,
                current_gender,
                state.player_components.basic.info.selected_main_role_id,
            )
        });
    let Some(source_role_id) = source_role_id else {
        return Ok(RoleSexChangeResponse {
            ErrorCode: Some(ErrorCode::ErrRoleSexFuncNotOpen),
            Sex: current_gender,
        });
    };
    let Some(source_role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&source_role_id)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleSexChangeResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            Sex: current_gender,
        });
    };
    let Some(target_role_id) = role_helper::main_role_id_for_element(
        &state.assets,
        target_gender,
        source_role_config.ElementId,
    ) else {
        return Ok(RoleSexChangeResponse {
            ErrorCode: Some(ErrorCode::ErrRoleSexFuncNotOpen),
            Sex: current_gender,
        });
    };

    let transfer = {
        let State {
            fs,
            assets,
            player_components,
            scene,
            ..
        } = state;
        role_helper::transfer_main_role_state(
            fs,
            assets,
            scene.as_mut(),
            &mut player_components.basic,
            &mut player_components.role,
            &mut player_components.weapon,
            &mut player_components.echo,
            source_role_id,
            target_role_id,
        )?
    };
    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: target_role_id,
    });

    state.player_components.basic.info.attributes.sex = target_gender;
    role_helper::save_basic_info(&state.fs, &state.player_components.basic)
        .map_err(|e| e.to_string())?;
    Account::sync_login_sex_for_player(&state.fs, state.player_components.basic.player_id, target_gender)
        .map_err(|e| e.to_string())?;

    if transfer.formation_changed {
        let State {
            conn,
            fs,
            assets,
            player_components,
            scene: scene_opt,
            buff_timers,
            ..
        } = state;
        if let Some(scene) = scene_opt.as_mut() {
            role_helper::reset_roles(
                scene,
                fs,
                assets,
                &mut player_components.role,
                &player_components.weapon,
                &player_components.echo,
                conn,
                Some(&[target_role_id]),
                buff_timers,
                now_ms_val,
            )
            .map_err(|e| e.to_string())?;
            buff_timers.ensure_all_registered(scene, assets, now_ms_val);
            buff_timers.sync_all_left_durations(scene, now_ms_val);
            scene.save(fs).map_err(|e| e.to_string())?;
        }
        events.enqueue(Event::UpdateFormations {
            player_id: state.player_id,
            formation_id: 0,
            role_ids: Vec::new(),
        });
    }

    let Some(target_role) = state
        .player_components
        .role
        .role_map
        .get(&target_role_id)
        .cloned()
    else {
        return Ok(RoleSexChangeResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            Sex: current_gender,
        });
    };

    let client_role_info = role_stats::to_client_role_info(
        &state.assets,
        &mut state.player_components.role,
        target_role_id,
        &target_role,
        &state.player_components.weapon,
        &state.player_components.echo,
    )
    .map_err(|e| e.to_string())?;
    state.conn.push(
        RoleChangeNotify::MSG_ID,
        &RoleChangeNotify {
            SourceRoleId: source_role_id,
            RoleInfo: Some(client_role_info),
        },
    );
    state.conn.push(
        RoleChangeUnlockNotify::MSG_ID,
        &RoleChangeUnlockNotify {
            UnlockRoleIds: role_helper::main_role_unlock_list(&state.assets, target_gender),
            NextAllowChangeTime: 0,
        },
    );
    state.conn.push(
        LogoutNotify::MSG_ID,
        &LogoutNotify {
            ErrorCode: Some(ErrorCode::ErrSexChangeLogout),
            ..Default::default()
        },
    );

    Ok(RoleSexChangeResponse {
        ErrorCode: Some(ErrorCode::Success),
        Sex: target_gender,
    })
}

pub fn handle_role_element_change_request(
    state: &mut State,
    events: &mut EventQueue,
    req: RoleElementChangeRequest,
) -> Result<RoleElementChangeResponse, String> {
    let now_ms_val = now_ms();
    let gender = state.player_components.basic.info.attributes.sex;
    let Some(target_role_id) =
        role_helper::main_role_id_for_element(&state.assets, gender, req.ElementType)
    else {
        return Ok(RoleElementChangeResponse {
            ErrorCode: Some(ErrorCode::ErrRoleChangeElementFunc),
        });
    };
    let source_role_id = state
        .scene
        .as_ref()
        .and_then(|s| role_helper::current_formation_main_role_id(&state.assets, s, gender))
        .or_else(|| {
            role_helper::selected_main_role_id(
                &state.assets,
                &state.player_components.role,
                gender,
                state.player_components.basic.info.selected_main_role_id,
            )
        })
        .unwrap_or(target_role_id);

    let transfer = {
        let State {
            fs,
            assets,
            player_components,
            scene,
            ..
        } = state;
        role_helper::transfer_main_role_state(
            fs,
            assets,
            scene.as_mut(),
            &mut player_components.basic,
            &mut player_components.role,
            &mut player_components.weapon,
            &mut player_components.echo,
            source_role_id,
            target_role_id,
        )?
    };
    if transfer.basic_changed {
        role_helper::save_basic_info(&state.fs, &state.player_components.basic)
            .map_err(|e| e.to_string())?;
    }
    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: target_role_id,
    });

    if transfer.formation_changed {
        let State {
            conn,
            fs,
            assets,
            player_components,
            scene: scene_opt,
            buff_timers,
            ..
        } = state;
        if let Some(scene) = scene_opt.as_mut() {
            role_helper::reset_roles(
                scene,
                fs,
                assets,
                &mut player_components.role,
                &player_components.weapon,
                &player_components.echo,
                conn,
                Some(&[target_role_id]),
                buff_timers,
                now_ms_val,
            )
            .map_err(|e| e.to_string())?;
            buff_timers.ensure_all_registered(scene, assets, now_ms_val);
            buff_timers.sync_all_left_durations(scene, now_ms_val);
            scene.save(fs).map_err(|e| e.to_string())?;
        }
        events.enqueue(Event::UpdateFormations {
            player_id: state.player_id,
            formation_id: 0,
            role_ids: Vec::new(),
        });
    }

    let Some(target_role) = state
        .player_components
        .role
        .role_map
        .get(&target_role_id)
        .cloned()
    else {
        return Ok(RoleElementChangeResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
        });
    };

    push_weapon_equip_notify(state, target_role_id, target_role.weapon);
    let client_role_info = role_stats::to_client_role_info(
        &state.assets,
        &mut state.player_components.role,
        target_role_id,
        &target_role,
        &state.player_components.weapon,
        &state.player_components.echo,
    )
    .map_err(|e| e.to_string())?;
    state.conn.push(
        RoleChangeNotify::MSG_ID,
        &RoleChangeNotify {
            SourceRoleId: source_role_id,
            RoleInfo: Some(client_role_info),
        },
    );
    push_phantom_equip_notify(state, target_role_id)?;

    Ok(RoleElementChangeResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn role_in_current_formation(scene: &Scene, role_id: i32) -> bool {
    if role_id <= 0 || scene.formation_info.cur_formation < 0 {
        return false;
    }
    let form_idx = scene.formation_info.cur_formation as usize;
    scene
        .formation_info
        .formations
        .get(form_idx)
        .is_some_and(|f| f.roles.iter().flatten().any(|r| r.role_id == role_id))
}

/// Switch to a role of the Echo (Phantom) group. After SwitchSceneGroupNotify {Group 2} the
/// client's TeamGoBattle sends SwitchRoleRequest for the group's current role
/// (SceneTeamModel.cs:465-468, SceneTeamController.SendSwitchRole :313-323); the Echo is not in
/// the battle formation, whose cur_role is kept for RestorePhantomFormation.
fn apply_phantom_role_switch(scene: &mut Scene, role_id: i32) -> bool {
    let Some(up_eid) = scene.phantom_role_entity_id(role_id) else {
        return false;
    };
    let down_eid = scene.current_role_entity_id();
    if down_eid == up_eid {
        return false;
    }
    scene.phantom_cur_role = role_id;
    if let Some(storage) = scene.entity_mut(down_eid) {
        storage.visible = false;
    }
    if let Some(storage) = scene.entity_mut(up_eid) {
        storage.visible = true;
    }
    true
}

fn apply_active_role_switch(scene: &mut Scene, role_id: i32) -> bool {
    if scene.phantom_role_entity_id(role_id).is_some() {
        return apply_phantom_role_switch(scene, role_id);
    }
    if !role_in_current_formation(scene, role_id) {
        return false;
    }
    let form_idx = scene.formation_info.cur_formation as usize;
    let formation = &mut scene.formation_info.formations[form_idx];
    let previous_role = formation.cur_role;
    if previous_role == role_id {
        return false;
    }
    formation.cur_role = role_id;
    for storage in &mut scene.entities {
        if storage.config.config_id == previous_role {
            storage.visible = false;
        } else if storage.config.config_id == role_id {
            storage.visible = true;
        }
    }
    true
}

pub fn handle_switch_role_request(
    state: &mut State,
    req: SwitchRoleRequest,
) -> SwitchRoleResponse {
    let now_ms_val = now_ms();
    let State {
        fs,
        assets,
        scene: scene_opt,
        buff_timers,
        ..
    } = state;
    let Some(scene) = scene_opt.as_mut() else {
        return SwitchRoleResponse {
            ErrorCode: Some(ErrorCode::SwitchRoleNotInCurrentFormation),
            RoleId: req.RoleId,
        };
    };
    if scene.phantom_role_entity_id(req.RoleId).is_none()
        && !role_in_current_formation(scene, req.RoleId)
    {
        return SwitchRoleResponse {
            ErrorCode: Some(ErrorCode::SwitchRoleNotInCurrentFormation),
            RoleId: req.RoleId,
        };
    }
    if !role_helper::is_role_entity_alive(scene, req.RoleId) {
        return SwitchRoleResponse {
            ErrorCode: Some(ErrorCode::ErrSwitchRoleIsDead),
            RoleId: req.RoleId,
        };
    }
    let _ = apply_active_role_switch(scene, req.RoleId);
    buff_timers.ensure_all_registered(scene, assets, now_ms_val);
    buff_timers.sync_all_left_durations(scene, now_ms_val);
    if let Err(err) = scene.save_persistent(fs) {
        tracing::error!("switch_role save_persistent failed: {err}");
    }
    SwitchRoleResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleId: req.RoleId,
    }
}

pub fn handle_client_current_role_report_push(
    state: &mut State,
    push: ClientCurrentRoleReportPush,
) {
    if push.CurrentRoleId <= 0 {
        return;
    }
    if let Some(scene) = state.scene.as_mut()
        && apply_active_role_switch(scene, push.CurrentRoleId)
        && let Err(err) = scene.save_persistent(&state.fs)
    {
        tracing::error!("client_current_role_report_push save_persistent failed: {err}");
    }
}

fn set_skill_level(role: &mut RoleInfo, skill_id: i32, level: i32) {
    if let Some(entry) = role.skills.iter_mut().find(|entry| entry.0 == skill_id) {
        entry.1 = level;
        return;
    }
    role.skills.push((skill_id, level));
}

fn get_skill_level(role: &RoleInfo, skill_id: i32) -> i32 {
    role.skills
        .iter()
        .find(|entry| entry.0 == skill_id)
        .map_or(0, |entry| entry.1)
}

fn set_skill_node_state(role: &mut RoleInfo, node_id: i32, active: bool, skill_id: i32) {
    if let Some(entry) = role
        .skill_node_state
        .iter_mut()
        .find(|entry| entry.node_id == node_id)
    {
        entry.active = active;
        entry.skill_id = skill_id;
        return;
    }
    role.skill_node_state.push(SkillNode {
        node_id,
        active,
        skill_id,
    });
}

fn get_max_resonant_chain_index(assets: &Assets, group_id: i32) -> i32 {
    assets
        .tables
        .resonant_chain
        .items
        .iter()
        .filter(|c| c.GroupId == group_id)
        .map(|c| c.GroupIndex)
        .max()
        .unwrap_or(0)
}

fn get_resonant_chain_by_index(
    assets: &Assets,
    group_id: i32,
    index: i32,
) -> Option<ResonantChain> {
    assets
        .tables
        .resonant_chain
        .items
        .iter()
        .find(|c| c.GroupId == group_id && c.GroupIndex == index)
        .cloned()
}

fn has_activate_consume_items(info: &InventoryInfo, chain: &ResonantChain) -> bool {
    chain
        .ActivateConsume
        .iter()
        .all(|(&item_id, &count)| info.normal_item_count(item_id) >= count)
}

fn consume_activate_items(info: &mut InventoryInfo, chain: &ResonantChain) -> Vec<NormalItem> {
    chain
        .ActivateConsume
        .iter()
        .map(|(&item_id, &count)| {
            let _ = info.consume_normal_item(item_id, count);
            NormalItem {
                Id: item_id,
                Count: info.normal_item_count(item_id),
                ExpireTime: 0,
            }
        })
        .collect()
}

pub fn handle_role_prop_request(
    state: &mut State,
    req: RolePropRequest,
) -> Result<RolePropResponse, String> {
    let mut prop_list = Vec::new();
    for role_id in req.RoleIds {
        if !state.player_components.role.role_map.contains_key(&role_id) {
            continue;
        }
        let snapshot = role_stats::build_snapshot(
            &state.assets,
            &mut state.player_components.role,
            &state.player_components.weapon,
            &state.player_components.echo,
            role_id,
        )
        .map_err(|e| e.to_string())?;
        prop_list.push(RolePropInfo {
            RoleId: role_id,
            BaseProp: snapshot
                .base_props
                .iter()
                .enumerate()
                .map(|(k, &v)| ArrayIntInt {
                    Key: k as i32,
                    Value: v,
                })
                .collect(),
            AddProp: snapshot
                .add_props
                .iter()
                .enumerate()
                .map(|(k, &v)| ArrayIntInt {
                    Key: k as i32,
                    Value: v,
                })
                .collect(),
            PhantomProp: None,
        });
    }
    Ok(RolePropResponse {
        Error: Some(ErrorCode::Success),
        RoleProps: prop_list,
    })
}

fn refresh_role_in_state(state: &mut State, role_id: i32) -> Result<(), String> {
    let State {
        conn,
        fs,
        assets,
        player_components,
        scene: scene_opt,
        ..
    } = state;
    if let Some(scene) = scene_opt.as_mut() {
        role_attribute_sync::refresh_role(
            conn,
            fs,
            scene,
            assets,
            &mut player_components.role,
            &player_components.weapon,
            &player_components.echo,
            role_id,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn handle_pb_up_level_skill_request(
    state: &mut State,
    events: &mut EventQueue,
    req: PbUpLevelSkillRequest,
) -> Result<PbUpLevelSkillResponse, String> {
    let Some(role_snap) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(PbUpLevelSkillResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(PbUpLevelSkillResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };
    let Some(skill) = state
        .assets
        .tables
        .skill
        .get_data_by_id(&req.SkillId)
        .map(|r| r.into_owned())
    else {
        return Ok(PbUpLevelSkillResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    };
    if skill.SkillGroupId != role_config.SkillId {
        return Ok(PbUpLevelSkillResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    }

    let current_level = get_skill_level(&role_snap, req.SkillId);
    if current_level <= 0 || current_level >= skill.MaxSkillLevel {
        return Ok(PbUpLevelSkillResponse {
            ErrorCode: Some(ErrorCode::ErrRoleLevelMax),
            ..Default::default()
        });
    }
    let next_level = current_level + 1;

    if let Some(lvl_cfg) =
        find_skill_level_config(&state.assets, skill.SkillLevelGroupId, next_level)
    {
        {
            let qctx = state.quest_context(events);
            let eval_ctx = qctx.make_eval_context_for_role(req.RoleId);
            if !condition::eval_condition_or_group(&eval_ctx, lvl_cfg.Condition) {
                return Ok(PbUpLevelSkillResponse {
                    ErrorCode: Some(ErrorCode::ErrRoleConditionNoEnough),
                    ..Default::default()
                });
            }
        }
        let mut costs = Vec::new();
        consume::append_int_map(&mut costs, &lvl_cfg.Consume);
        let ok = {
            let State {
                conn,
                fs,
                player_components,
                ..
            } = state;
            consume::consume_costs(
                conn,
                fs,
                &mut player_components.basic,
                &mut player_components.inventory,
                &costs,
            )
            .map_err(|e| e.to_string())?
        };
        if !ok {
            return Ok(PbUpLevelSkillResponse {
                ErrorCode: Some(ErrorCode::ErrItemNotEnough),
                ..Default::default()
            });
        }
    }

    {
        let role = state
            .player_components
            .role
            .role_map
            .get_mut(&req.RoleId)
            .expect("checked above");
        set_skill_level(role, req.SkillId, next_level);
        let r_clone = role.clone();
        save_role_immediate(
            &state.fs,
            state.player_components.role.player_id,
            req.RoleId,
            &r_clone,
        )?;
    }

    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: req.RoleId,
    });
    refresh_role_in_state(state, req.RoleId)?;
    {
        let mut qctx = state.quest_context(events);
        if let Err(err) = quest_engine::on_skill_unlock(&mut qctx, req.SkillId) {
            tracing::warn!("on_skill_unlock error: {err}");
        }
    }

    Ok(PbUpLevelSkillResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleId: req.RoleId,
        SkillInfo: Some(ArrayIntInt {
            Key: req.SkillId,
            Value: next_level,
        }),
    })
}

pub fn handle_role_activate_skill_request(
    state: &mut State,
    events: &mut EventQueue,
    req: RoleActivateSkillRequest,
) -> Result<RoleActivateSkillResponse, String> {
    let Some(role_snap) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(RoleActivateSkillResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleActivateSkillResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };

    let Some(node) = state
        .assets
        .tables
        .skill_tree
        .get_data_by_id(&req.SkillNodeId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleActivateSkillResponse {
            ErrorCode: Some(ErrorCode::ErrRolSkillNodeTypeUlock),
            ..Default::default()
        });
    };
    if node.NodeGroup != role_config.SkillTreeGroupId {
        return Ok(RoleActivateSkillResponse {
            ErrorCode: Some(ErrorCode::ErrRolSkillNodeTypeUlock),
            ..Default::default()
        });
    }
    if is_skill_node_active(&role_snap, req.SkillNodeId) {
        return Ok(RoleActivateSkillResponse {
            ErrorCode: Some(ErrorCode::ErrRolSkillNodeTypeActive),
            ..Default::default()
        });
    }

    {
        let qctx = state.quest_context(events);
        let eval_ctx = qctx.make_eval_context_for_role(req.RoleId);
        if !condition::eval_condition_or_group(&eval_ctx, node.UnLockCondition)
            || node
                .Condition
                .iter()
                .any(|&cond_id| !condition::eval_condition_or_group(&eval_ctx, cond_id))
        {
            return Ok(RoleActivateSkillResponse {
                ErrorCode: Some(ErrorCode::ErrRolSkillNodeTypeUlock),
                ..Default::default()
            });
        }
    }

    let mut costs = Vec::new();
    consume::append_int_map(&mut costs, &node.Consume);
    let ok = {
        let State {
            conn,
            fs,
            player_components,
            ..
        } = state;
        consume::consume_costs(
            conn,
            fs,
            &mut player_components.basic,
            &mut player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?
    };
    if !ok {
        return Ok(RoleActivateSkillResponse {
            ErrorCode: Some(ErrorCode::ErrItemNotEnough),
            ..Default::default()
        });
    }

    let node_states = {
        let role = state
            .player_components
            .role
            .role_map
            .get_mut(&req.RoleId)
            .expect("checked above");
        set_skill_node_state(role, req.SkillNodeId, true, node.SkillId);
        if node.SkillId > 0 && get_skill_level(role, node.SkillId) == 0 {
            set_skill_level(role, node.SkillId, 1);
        }
        let r_clone = role.clone();
        save_role_immediate(
            &state.fs,
            state.player_components.role.player_id,
            req.RoleId,
            &r_clone,
        )?;
        role.skill_node_state
            .iter()
            .map(|entry| ArraySkillNode {
                SkillNodeId: entry.node_id,
                IsActive: entry.active,
                SkillId: entry.skill_id,
            })
            .collect()
    };

    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: req.RoleId,
    });
    state.conn.push(
        RoleSkillNodeNotify::MSG_ID,
        &RoleSkillNodeNotify {
            RoleId: req.RoleId,
            SkillNodeState: node_states,
        },
    );
    refresh_role_in_state(state, req.RoleId)?;
    {
        let mut qctx = state.quest_context(events);
        if let Err(err) = quest_engine::on_skill_unlock(&mut qctx, node.SkillId) {
            tracing::warn!("on_skill_unlock error: {err}");
        }
    }

    Ok(RoleActivateSkillResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleId: req.RoleId,
        SkillInfo: Some(ArrayIntInt {
            Key: node.SkillId,
            Value: 1,
        }),
    })
}

pub fn handle_role_skill_level_up_view_request(
    state: &mut State,
    req: RoleSkillLevelUpViewRequest,
) -> Result<RoleSkillLevelUpViewResponse, String> {
    let Some(role) = state.player_components.role.role_map.get(&req.RoleId) else {
        return Ok(RoleSkillLevelUpViewResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state.assets.tables.role_info.get_data_by_id(&req.RoleId) else {
        return Ok(RoleSkillLevelUpViewResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };
    let Some(skill) = state.assets.tables.skill.get_data_by_id(&req.SkillId) else {
        return Ok(RoleSkillLevelUpViewResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    };
    if skill.SkillGroupId != role_config.SkillId {
        return Ok(RoleSkillLevelUpViewResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    }

    let mut cost_list = Vec::new();
    let next_level = get_skill_level(role, req.SkillId) + 1;
    if next_level <= skill.MaxSkillLevel
        && let Some(lvl_cfg) =
            find_skill_level_config(&state.assets, skill.SkillLevelGroupId, next_level)
    {
        for (&k, &v) in lvl_cfg.Consume.iter() {
            if k > 0 && v > 0 {
                cost_list.push(ArrayIntInt { Key: k, Value: v });
            }
        }
    }

    Ok(RoleSkillLevelUpViewResponse {
        ErrorCode: Some(ErrorCode::Success),
        CostList: cost_list,
        ..Default::default()
    })
}

pub fn handle_role_skill_view_request(
    state: &mut State,
    events: &mut EventQueue,
    req: RoleSkillViewRequest,
) -> Result<RoleSkillViewResponse, String> {
    let Some(role) = state.player_components.role.role_map.get(&req.RoleId) else {
        return Ok(RoleSkillViewResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleSkillViewResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };
    let Some(skill) = state
        .assets
        .tables
        .skill
        .get_data_by_id(&req.SkillId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleSkillViewResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    };
    if skill.SkillGroupId != role_config.SkillId {
        return Ok(RoleSkillViewResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    }

    let next_level = get_skill_level(role, req.SkillId) + 1;
    let cond_ok = if next_level <= skill.MaxSkillLevel {
        if let Some(lvl_cfg) =
            find_skill_level_config(&state.assets, skill.SkillLevelGroupId, next_level)
        {
            let qctx = state.quest_context(events);
            condition::eval_condition_or_group(
                &qctx.make_eval_context_for_role(req.RoleId),
                lvl_cfg.Condition,
            )
        } else {
            true
        }
    } else {
        false
    };

    Ok(RoleSkillViewResponse {
        ErrorCode: Some(ErrorCode::Success),
        IsConditionFinish: cond_ok,
        ..Default::default()
    })
}

pub fn handle_role_skill_quick_level_up_request(
    state: &mut State,
    events: &mut EventQueue,
    req: RoleSkillQuickLevelUpRequest,
) -> Result<RoleSkillQuickLevelUpResponse, String> {
    let Some(role_snap) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(RoleSkillQuickLevelUpResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleSkillQuickLevelUpResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };
    let Some(skill) = state
        .assets
        .tables
        .skill
        .get_data_by_id(&req.SkillId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleSkillQuickLevelUpResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    };
    if skill.SkillGroupId != role_config.SkillId {
        return Ok(RoleSkillQuickLevelUpResponse {
            ErrorCode: Some(ErrorCode::ErrSkillInfoParamError),
            ..Default::default()
        });
    }

    let current_level = get_skill_level(&role_snap, req.SkillId);
    if current_level <= 0 || current_level >= skill.MaxSkillLevel {
        return Ok(RoleSkillQuickLevelUpResponse {
            ErrorCode: Some(ErrorCode::ErrRoleLevelMax),
            ..Default::default()
        });
    }
    let requested_level = if req.TargetLevel > current_level {
        req.TargetLevel
    } else {
        current_level + 1
    };
    let next_level = requested_level.min(skill.MaxSkillLevel);

    let mut costs = Vec::new();
        for lv in (current_level + 1)..=next_level {
            let lvl_cfg = find_skill_level_config(&state.assets, skill.SkillLevelGroupId, lv);
            if let Some(lvl_cfg) = lvl_cfg {
                let qctx = state.quest_context(events);
                let eval_ctx = qctx.make_eval_context_for_role(req.RoleId);
                if !condition::eval_condition_or_group(&eval_ctx, lvl_cfg.Condition) {
                    return Ok(RoleSkillQuickLevelUpResponse {
                        ErrorCode: Some(ErrorCode::ErrRoleConditionNoEnough),
                        ..Default::default()
                    });
                }
                consume::append_int_map(&mut costs, &lvl_cfg.Consume);
            }
        }

    let ok = {
        let State {
            conn,
            fs,
            player_components,
            ..
        } = state;
        consume::consume_costs(
            conn,
            fs,
            &mut player_components.basic,
            &mut player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?
    };
    if !ok {
        return Ok(RoleSkillQuickLevelUpResponse {
            ErrorCode: Some(ErrorCode::ErrItemNotEnough),
            ..Default::default()
        });
    }

    let role_clone = {
        let role = state
            .player_components
            .role
            .role_map
            .get_mut(&req.RoleId)
            .expect("checked above");
        set_skill_level(role, req.SkillId, next_level);
        let r = role.clone();
        save_role_immediate(
            &state.fs,
            state.player_components.role.player_id,
            req.RoleId,
            &r,
        )?;
        r
    };

    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: req.RoleId,
    });
    refresh_role_in_state(state, req.RoleId)?;
    {
        let mut qctx = state.quest_context(events);
        if let Err(err) = quest_engine::on_skill_unlock(&mut qctx, req.SkillId) {
            tracing::warn!("on_skill_unlock error: {err}");
        }
    }

    let client_role = role_stats::to_client_role_info(
        &state.assets,
        &mut state.player_components.role,
        req.RoleId,
        &role_clone,
        &state.player_components.weapon,
        &state.player_components.echo,
    )
    .map_err(|e| e.to_string())?;
    Ok(RoleSkillQuickLevelUpResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleInfo: Some(client_role),
    })
}

pub fn handle_player_voice_language_request(
    state: &mut State,
    _req: PlayerVoiceLanguageRequest,
) -> Result<PlayerVoiceLanguageResponse, String> {
    let role_voices = state
        .player_components
        .role
        .role_map
        .iter()
        .filter(|(_, role)| role.voice_language != 0)
        .map(|(&role_id, role)| RoleVoice {
            RoleId: role_id,
            VoiceId: role.voice_language,
        })
        .collect();
    Ok(PlayerVoiceLanguageResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleVoices: role_voices,
    })
}

pub fn handle_player_role_voice_set_request(
    state: &mut State,
    events: &mut EventQueue,
    req: PlayerRoleVoiceSetRequest,
) -> Result<PlayerRoleVoiceSetResponse, String> {
    if req.RoleVoices.is_empty() {
        for (&role_id, role) in &mut state.player_components.role.role_map {
            if role.voice_language == 0 {
                continue;
            }
            role.voice_language = 0;
            events.enqueue(Event::RoleInfoModified {
                player_id: state.player_id,
                role_id,
            });
        }
        return Ok(PlayerRoleVoiceSetResponse {
            ErrorCode: Some(ErrorCode::Success),
        });
    }

    for voice in req.RoleVoices {
        if let Some(role) = state.player_components.role.role_map.get_mut(&voice.RoleId)
            && role.voice_language != voice.VoiceLanguage
        {
            role.voice_language = voice.VoiceLanguage;
            events.enqueue(Event::RoleInfoModified {
                player_id: state.player_id,
                role_id: voice.RoleId,
            });
        }
    }

    Ok(PlayerRoleVoiceSetResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_resonant_chain_unlock_request(
    state: &mut State,
    events: &mut EventQueue,
    req: ResonantChainUnlockRequest,
) -> Result<ResonantChainUnlockResponse, String> {
    let Some(role_snap) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(ResonantChainUnlockResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(ResonantChainUnlockResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };

    let max_index =
        get_max_resonant_chain_index(&state.assets, role_config.ResonantChainGroupId);
    if max_index == 0 {
        return Ok(ResonantChainUnlockResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    }
    if role_snap.resonant_chain_group_index >= max_index {
        return Ok(ResonantChainUnlockResponse {
            ErrorCode: Some(ErrorCode::Success),
            RoleId: req.RoleId,
            ResonantChainGroupIndex: role_snap.resonant_chain_group_index,
        });
    }

    let next_index = role_snap.resonant_chain_group_index + 1;
    let Some(chain) =
        get_resonant_chain_by_index(&state.assets, role_config.ResonantChainGroupId, next_index)
    else {
        return Ok(ResonantChainUnlockResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    if !has_activate_consume_items(&state.player_components.inventory.info, &chain) {
        return Ok(ResonantChainUnlockResponse {
            ErrorCode: Some(ErrorCode::ErrRoleItemListNoEnough),
            ..Default::default()
        });
    }

    let updated_items =
        consume_activate_items(&mut state.player_components.inventory.info, &chain);
    let new_chain_idx = (role_snap.resonant_chain_group_index + 1).min(max_index);
    {
        let role = state
            .player_components
            .role
            .role_map
            .get_mut(&req.RoleId)
            .expect("checked above");
        role.resonant_chain_group_index = new_chain_idx;
        let r_clone = role.clone();
        save_role_immediate(
            &state.fs,
            state.player_components.role.player_id,
            req.RoleId,
            &r_clone,
        )?;
    }

    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: req.RoleId,
    });

    if let Some(new_chain) =
        get_resonant_chain_by_index(&state.assets, role_config.ResonantChainGroupId, new_chain_idx)
        && !new_chain.BuffIds.is_empty()
        && let Some(scene) = state.scene.as_ref()
        && let Some(idx) = scene
            .entities
            .iter()
            .position(|s| s.config.config_id == req.RoleId)
    {
        let handle = EntityHandle {
            index: idx,
            net_id: scene.entities[idx].net_id,
        };
        let buffs: Vec<BuffAdditionEntry> = new_chain
            .BuffIds
            .iter()
            .map(|&buff_id| BuffAdditionEntry {
                id: buff_id,
                is_active: true,
                stack_count: 1,
                ..Default::default()
            })
            .collect();
        events.enqueue(Event::BuffAddition {
            target: handle,
            instigator: handle,
            buffs,
        });
    }

    comp_util::save_struct(
        &state.fs,
        &state.player_components.inventory.info,
        &format!(
            "player/{}/{}",
            state.player_components.inventory.player_id,
            InventoryInfo::DATA_PATH
        ),
    )
    .map_err(|e| e.to_string())?;
    state.conn.push(
        NormalItemUpdateNotify::MSG_ID,
        &NormalItemUpdateNotify {
            NormalItemList: updated_items,
            NoTips: true,
        },
    );
    refresh_role_in_state(state, req.RoleId)?;

    Ok(ResonantChainUnlockResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleId: req.RoleId,
        ResonantChainGroupIndex: new_chain_idx,
    })
}

pub fn handle_role_skill_branch_modify_request(
    state: &mut State,
    events: &mut EventQueue,
    req: RoleSkillBranchModifyRequest,
) -> Result<RoleSkillBranchModifyResponse, String> {
    let Some(role_snap) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(RoleSkillBranchModifyResponse {
            ErrorCode: Some(ErrorCode::ErrSkillBranchRoleValid),
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleSkillBranchModifyResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
        });
    };
    if !is_valid_skill_branch_for_role(&state.assets, &role_config, req.RoleId, req.SkillBranch) {
        return Ok(RoleSkillBranchModifyResponse {
            ErrorCode: Some(ErrorCode::ErrSkillBranchNotValid),
        });
    }

    let prev_skill_branch = role_snap.skill_branch;
    let entity_handle = state.scene.as_ref().and_then(|scene| {
        scene
            .entities
            .iter()
            .position(|s| s.config.config_id == req.RoleId)
            .map(|idx| EntityHandle {
                index: idx,
                net_id: scene.entities[idx].net_id,
            })
    });

    if let Some(handle) = entity_handle {
        if prev_skill_branch != 0
            && let Some(prev_branch_data) = state
                .assets
                .tables
                .skill_branch_buff
                .get_data_by_id(&prev_skill_branch)
        {
            for &buff_id in &prev_branch_data.Buffs {
                events.enqueue(Event::BuffRemovalById {
                    entity: handle,
                    buff_id,
                });
            }
        }
        if req.SkillBranch != 0
            && let Some(new_branch_data) = state
                .assets
                .tables
                .skill_branch_buff
                .get_data_by_id(&req.SkillBranch)
        {
            let add_buffs: Vec<BuffAdditionEntry> = new_branch_data
                .Buffs
                .iter()
                .map(|&buff_id| BuffAdditionEntry {
                    id: buff_id,
                    is_active: true,
                    stack_count: 1,
                    ..Default::default()
                })
                .collect();
            events.enqueue(Event::BuffAddition {
                target: handle,
                instigator: handle,
                buffs: add_buffs,
            });
        }
    }

    {
        let role = state
            .player_components
            .role
            .role_map
            .get_mut(&req.RoleId)
            .expect("checked above");
        role.skill_branch = req.SkillBranch;
        let r_clone = role.clone();
        save_role_immediate(
            &state.fs,
            state.player_components.role.player_id,
            req.RoleId,
            &r_clone,
        )?;
    }
    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: req.RoleId,
    });
    events.enqueue(Event::UpdateFormations {
        player_id: state.player_id,
        formation_id: 0,
        role_ids: Vec::new(),
    });

    Ok(RoleSkillBranchModifyResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn role_level_exp(assets: &Assets, consume_group_id: i32, level: i32) -> i32 {
    assets
        .tables
        .role_level_consume
        .items
        .iter()
        .find(|entry| entry.ConsumeGroupId == consume_group_id && entry.Level == level)
        .map_or(1000, |entry| {
            if entry.ExpCount > 0 {
                entry.ExpCount
            } else {
                1000
            }
        })
}

fn role_level_limit(assets: &Assets, breach_group_id: i32, breach: i32) -> i32 {
    if let Some(entry) = find_role_breach_entry(assets, breach_group_id, breach) {
        return entry.MaxLevel;
    }
    match breach {
        0 => 20,
        1 => 40,
        2 => 50,
        3 => 60,
        4 => 70,
        5 => 80,
        _ => 90,
    }
}

fn role_exp_item_value(assets: &Assets, item_id: i32) -> i32 {
    assets
        .tables
        .role_exp_item
        .get_data_by_id(&item_id)
        .map_or(0, |item| item.BasicExp)
}

pub fn handle_role_level_up_view_request(
    state: &mut State,
    req: RoleLevelUpViewRequest,
) -> Result<RoleLevelUpViewResponse, String> {
    let Some(role) = state.player_components.role.role_map.get(&req.RoleId) else {
        return Ok(RoleLevelUpViewResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state.assets.tables.role_info.get_data_by_id(&req.RoleId) else {
        return Ok(RoleLevelUpViewResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };

    let max_level = role_level_limit(&state.assets, role_config.BreachId, role.breakthrough);
    let level_exp_info: Vec<ArrayIntInt> = state
        .assets
        .tables
        .role_level_consume
        .items
        .iter()
        .filter(|e| e.ConsumeGroupId == role_config.LevelConsumeId)
        .map(|e| ArrayIntInt {
            Key: e.Level,
            Value: e.ExpCount,
        })
        .collect();

    let mut selected_items = Vec::new();
    let mut add_exp_i64 = 0i64;
    if !req.ItemList.is_empty() {
        let mut used_counts: HashMap<i32, i64> = HashMap::new();
        for item in &req.ItemList {
            if item.Key <= 0 || item.Value <= 0 {
                continue;
            }
            let exp_val = role_exp_item_value(&state.assets, item.Key);
            if exp_val <= 0 {
                continue;
            }
            let owned = i64::from(
                state
                    .player_components
                    .inventory
                    .info
                    .normal_item_count(item.Key),
            );
            let already = used_counts.get(&item.Key).copied().unwrap_or(0);
            let take_i64 = i64::from(item.Value).min((owned - already).max(0));
            if take_i64 <= 0 {
                continue;
            }
            used_counts.insert(item.Key, already + take_i64);
            selected_items.push(ArrayIntInt {
                Key: item.Key,
                Value: take_i64 as i32,
            });
            add_exp_i64 += i64::from(exp_val) * take_i64;
        }
    } else if req.MaxItemId > 0 && role.level < max_level {
        let mut needed_total = -i64::from(role.exp);
        for lv in role.level..max_level {
            needed_total +=
                i64::from(role_level_exp(&state.assets, role_config.LevelConsumeId, lv + 1));
        }
        let mut remaining = needed_total.max(0);

        for (idx, exp_item) in state.assets.tables.role_exp_item.items.iter().enumerate().rev() {
            if remaining <= 0 {
                break;
            }
            if exp_item.Id > req.MaxItemId || exp_item.BasicExp <= 0 {
                continue;
            }
            let owned = i64::from(
                state
                    .player_components
                    .inventory
                    .info
                    .normal_item_count(exp_item.Id),
            );
            if owned <= 0 {
                continue;
            }
            let b_exp = i64::from(exp_item.BasicExp);
            let take_raw = if idx == 0 {
                (remaining + b_exp - 1) / b_exp
            } else {
                remaining / b_exp
            };
            let count_i64 = owned.min(take_raw.max(0));
            if count_i64 <= 0 {
                continue;
            }
            selected_items.push(ArrayIntInt {
                Key: exp_item.Id,
                Value: count_i64 as i32,
            });
            let gain = count_i64 * b_exp;
            add_exp_i64 += gain;
            remaining -= gain;
        }
    }
    let add_exp = add_exp_i64.min(i64::from(i32::MAX)) as i32;

    let mut sim_level = role.level;
    let mut sim_exp = role.exp + add_exp;
    while sim_level < max_level {
        let next_req = role_level_exp(&state.assets, role_config.LevelConsumeId, sim_level + 1);
        if next_req <= 0 || sim_exp < next_req {
            break;
        }
        sim_exp -= next_req;
        sim_level += 1;
    }
    if sim_level >= max_level {
        sim_exp = 0;
    }

    let final_prop = build_preview_final_props(
        &state.assets,
        req.RoleId,
        role,
        sim_level,
        role.breakthrough,
        &state.player_components.weapon,
        &state.player_components.echo,
    );

    let cost_list = if add_exp > 0 {
        vec![ArrayIntInt {
            Key: 2,
            Value: ((add_exp * 35) / 100).max(1),
        }]
    } else {
        Vec::new()
    };

    Ok(RoleLevelUpViewResponse {
        ErrorCode: Some(ErrorCode::Success),
        Level: sim_level,
        LevelExpInfo: level_exp_info,
        Exp: sim_exp,
        AddExp: add_exp,
        FinalProp: final_prop,
        CostList: cost_list,
        OverflowList: Vec::new(),
        ItemList: selected_items,
    })
}

pub fn handle_pb_up_level_role_request(
    state: &mut State,
    events: &mut EventQueue,
    req: PbUpLevelRoleRequest,
) -> Result<PbUpLevelRoleResponse, String> {
    let Some(role_snap) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(PbUpLevelRoleResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(PbUpLevelRoleResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };

    let max_level = role_level_limit(&state.assets, role_config.BreachId, role_snap.breakthrough);
    if role_snap.level >= max_level {
        return Ok(PbUpLevelRoleResponse {
            ErrorCode: Some(ErrorCode::ErrRoleLevelMax),
            RoleId: req.RoleId,
            Level: role_snap.level,
            Exp: role_snap.exp,
            ..Default::default()
        });
    }

    let mut gained_exp_i64 = 0i64;
    let mut req_counts: HashMap<i32, i64> = HashMap::new();
    for item in &req.ItemList {
        if item.Key <= 0 || item.Value <= 0 {
            continue;
        }
        let exp_val = role_exp_item_value(&state.assets, item.Key);
        if exp_val <= 0 {
            continue;
        }
        let total = req_counts.entry(item.Key).or_insert(0);
        *total += i64::from(item.Value);
        if i64::from(
            state
                .player_components
                .inventory
                .info
                .normal_item_count(item.Key),
        ) < *total
        {
            return Ok(PbUpLevelRoleResponse {
                ErrorCode: Some(ErrorCode::ErrRoleItemListNoEnough),
                RoleId: req.RoleId,
                Level: role_snap.level,
                Exp: role_snap.exp,
                ..Default::default()
            });
        }
        let Some(add) = i64::from(exp_val).checked_mul(i64::from(item.Value)) else {
            return Ok(PbUpLevelRoleResponse {
                ErrorCode: Some(ErrorCode::ErrRoleItemExpError),
                RoleId: req.RoleId,
                Level: role_snap.level,
                Exp: role_snap.exp,
                ..Default::default()
            });
        };
        gained_exp_i64 += add;
    }

    if gained_exp_i64 <= 0 || gained_exp_i64 > i64::from(i32::MAX) {
        return Ok(PbUpLevelRoleResponse {
            ErrorCode: Some(ErrorCode::ErrRoleItemExpError),
            RoleId: req.RoleId,
            Level: role_snap.level,
            Exp: role_snap.exp,
            ..Default::default()
        });
    }
    let gained_exp = gained_exp_i64 as i32;

    let coin_cost_i64 = ((gained_exp_i64 * 35) / 100).max(1);
    if coin_cost_i64 > i64::from(i32::MAX)
        || state.player_components.basic.info.attributes.coin < coin_cost_i64 as i32
    {
        return Ok(PbUpLevelRoleResponse {
            ErrorCode: Some(ErrorCode::ErrItemNotEnough),
            RoleId: req.RoleId,
            Level: role_snap.level,
            Exp: role_snap.exp,
            ..Default::default()
        });
    }
    let coin_cost = coin_cost_i64 as i32;

    for (&pid, &cnt) in &req_counts {
        let _ = state
            .player_components
            .inventory
            .info
            .consume_normal_item(pid, cnt as i32);
    }

    let mut new_level = role_snap.level;
    let mut total_exp = i64::from(role_snap.exp) + i64::from(gained_exp);
    while new_level < max_level {
        let next_req = role_level_exp(&state.assets, role_config.LevelConsumeId, new_level + 1);
        if next_req <= 0 || total_exp < i64::from(next_req) {
            break;
        }
        total_exp -= i64::from(next_req);
        new_level += 1;
    }

    let mut touched_potions: Vec<i32> = req_counts.keys().copied().collect();
    let mut item_map = std::collections::BTreeMap::new();
    let new_exp = if new_level >= max_level {
        let mut excess = total_exp;
        const REFUND_POTS: [(i32, i64); 4] = [
            (43010004, 20000),
            (43010003, 8000),
            (43010002, 3000),
            (43010001, 1000),
        ];
        for (id, pot_exp) in REFUND_POTS {
            if excess >= pot_exp {
                let cnt = (excess / pot_exp).min(i64::from(i32::MAX)) as i32;
                if cnt > 0 {
                    excess -= i64::from(cnt) * pot_exp;
                    state
                        .player_components
                        .inventory
                        .info
                        .add_normal_item(id, cnt);
                    item_map.insert(id, cnt);
                    if !touched_potions.contains(&id) {
                        touched_potions.push(id);
                    }
                }
            }
        }
        0
    } else {
        total_exp as i32
    };

    let mut updated_items: Vec<NormalItem> = touched_potions
        .into_iter()
        .map(|pid| NormalItem {
            Id: pid,
            Count: state.player_components.inventory.info.normal_item_count(pid),
            ExpireTime: 0,
        })
        .collect();

    state.player_components.basic.info.attributes.coin -= coin_cost;
    let new_coin = state.player_components.basic.info.attributes.coin;
    updated_items.push(NormalItem {
        Id: 2,
        Count: new_coin,
        ExpireTime: 0,
    });

    comp_util::save_struct(
        &state.fs,
        &state.player_components.inventory.info,
        &format!(
            "player/{}/{}",
            state.player_components.inventory.player_id,
            InventoryInfo::DATA_PATH
        ),
    )
    .map_err(|e| e.to_string())?;
    role_helper::save_basic_info(&state.fs, &state.player_components.basic)
        .map_err(|e| e.to_string())?;

    if !updated_items.is_empty() {
        state.conn.push(
            NormalItemUpdateNotify::MSG_ID,
            &NormalItemUpdateNotify {
                NormalItemList: updated_items,
                NoTips: true,
            },
        );
    }
    state.conn.push(
        PlayerAttrNotify::MSG_ID,
        &PlayerAttrNotify {
            Attributes: vec![PlayerAttr {
                Key: Some(PlayerAttrKey::Coin),
                ValueType: Some(PlayerAttrType::Int32),
                Value: Some(PlayerAttrValueOneof::Int32Value(new_coin)),
            }],
        },
    );

    let assets_ref = state.assets.clone();
    {
        let role = state
            .player_components
            .role
            .role_map
            .get_mut(&req.RoleId)
            .expect("checked above");
        role.level = new_level;
        role.exp = new_exp;
        let _ = role.refresh_scaled_properties(&assets_ref, req.RoleId);
        let r_clone = role.clone();
        save_role_immediate(
            &state.fs,
            state.player_components.role.player_id,
            req.RoleId,
            &r_clone,
        )?;
    }

    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: req.RoleId,
    });
    events.enqueue(Event::UpdateFormations {
        player_id: state.player_id,
        formation_id: 0,
        role_ids: Vec::new(),
    });
    refresh_role_in_state(state, req.RoleId)?;

    let mut qctx = state.quest_context(events);
    quest_engine::on_condition_state_changed(&mut qctx).map_err(|e| e.to_string())?;
    Ok(PbUpLevelRoleResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleId: req.RoleId,
        Level: new_level,
        Exp: new_exp,
        ItemMap: item_map,
    })
}

pub fn handle_role_break_through_view_request(
    state: &mut State,
    events: &mut EventQueue,
    req: RoleBreakThroughViewRequest,
) -> Result<RoleBreakThroughViewResponse, String> {
    let Some(role) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(RoleBreakThroughViewResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(RoleBreakThroughViewResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };

    let target_breach = (role.breakthrough + 1).min(6);
    let mut cost_list = Vec::new();
    let mut level_limit = role_level_limit(&state.assets, role_config.BreachId, target_breach);
    let mut is_condition_finish = true;

    if let Some(entry) = find_role_breach_entry(&state.assets, role_config.BreachId, target_breach)
    {
        level_limit = entry.MaxLevel;
        {
            let qctx = state.quest_context(events);
            is_condition_finish = condition::eval_condition_or_group(
                &qctx.make_eval_context_for_role(req.RoleId),
                entry.ConditionId,
            );
        }
        for (&k, &v) in entry.BreachConsume.iter() {
            if k > 0 && v > 0 {
                cost_list.push(ArrayIntInt { Key: k, Value: v });
            }
        }
    }

    let final_prop = build_preview_final_props(
        &state.assets,
        req.RoleId,
        &role,
        role.level,
        target_breach,
        &state.player_components.weapon,
        &state.player_components.echo,
    );

    Ok(RoleBreakThroughViewResponse {
        ErrorCode: Some(ErrorCode::Success),
        LevelLimit: level_limit,
        CostList: cost_list,
        FinalProp: final_prop,
        IsConditionFinish: is_condition_finish,
        ..Default::default()
    })
}

pub fn handle_pb_over_role_request(
    state: &mut State,
    events: &mut EventQueue,
    req: PbOverRoleRequest,
) -> Result<PbOverRoleResponse, String> {
    let Some(role_snap) = state
        .player_components
        .role
        .role_map
        .get(&req.RoleId)
        .cloned()
    else {
        return Ok(PbOverRoleResponse {
            ErrorCode: Some(ErrorCode::RequestParamError),
            ..Default::default()
        });
    };
    let Some(role_config) = state
        .assets
        .tables
        .role_info
        .get_data_by_id(&req.RoleId)
        .map(|r| r.into_owned())
    else {
        return Ok(PbOverRoleResponse {
            ErrorCode: Some(ErrorCode::ErrRoleNoConfig),
            ..Default::default()
        });
    };

    if role_snap.breakthrough >= 6 {
        return Ok(PbOverRoleResponse {
            ErrorCode: Some(ErrorCode::ErrRoleOverNotEnough),
            RoleId: req.RoleId,
            Breakthrough: role_snap.breakthrough,
        });
    }

    let current_limit = role_level_limit(&state.assets, role_config.BreachId, role_snap.breakthrough);
    if role_snap.level < current_limit {
        return Ok(PbOverRoleResponse {
            ErrorCode: Some(ErrorCode::ErrRoleLevelNotEnough),
            RoleId: req.RoleId,
            Breakthrough: role_snap.breakthrough,
        });
    }

    let target_breach = role_snap.breakthrough + 1;
    let Some(breach_cfg) =
        find_role_breach_entry(&state.assets, role_config.BreachId, target_breach)
    else {
        return Ok(PbOverRoleResponse {
            ErrorCode: Some(ErrorCode::ErrRoleOverNotEnough),
            RoleId: req.RoleId,
            Breakthrough: role_snap.breakthrough,
        });
    };

    {
        let qctx = state.quest_context(events);
        if !condition::eval_condition_or_group(
            &qctx.make_eval_context_for_role(req.RoleId),
            breach_cfg.ConditionId,
        ) {
            return Ok(PbOverRoleResponse {
                ErrorCode: Some(ErrorCode::ErrRoleConditionNoEnough),
                RoleId: req.RoleId,
                Breakthrough: role_snap.breakthrough,
            });
        }
    }

    let mut costs = Vec::new();
    consume::append_int_map(&mut costs, &breach_cfg.BreachConsume);
    let ok = {
        let State {
            conn,
            fs,
            player_components,
            ..
        } = state;
        consume::consume_costs(
            conn,
            fs,
            &mut player_components.basic,
            &mut player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?
    };
    if !ok {
        return Ok(PbOverRoleResponse {
            ErrorCode: Some(ErrorCode::ErrItemNotEnough),
            RoleId: req.RoleId,
            Breakthrough: role_snap.breakthrough,
        });
    }

    if breach_cfg.BreachReward > 0 {
        let mut rctx = state.reward_context();
        reward::grant_drop_package(&mut rctx, breach_cfg.BreachReward, 1)
            .map_err(|e| e.to_string())?;
    }

    let assets_ref = state.assets.clone();
    {
        let role = state
            .player_components
            .role
            .role_map
            .get_mut(&req.RoleId)
            .expect("checked above");
        role.breakthrough = target_breach;
        let _ = role.refresh_scaled_properties(&assets_ref, req.RoleId);
        let r_clone = role.clone();
        save_role_immediate(
            &state.fs,
            state.player_components.role.player_id,
            req.RoleId,
            &r_clone,
        )?;
    }

    events.enqueue(Event::RoleInfoModified {
        player_id: state.player_id,
        role_id: req.RoleId,
    });
    events.enqueue(Event::UpdateFormations {
        player_id: state.player_id,
        formation_id: 0,
        role_ids: Vec::new(),
    });
    refresh_role_in_state(state, req.RoleId)?;

    Ok(PbOverRoleResponse {
        ErrorCode: Some(ErrorCode::Success),
        RoleId: req.RoleId,
        Breakthrough: target_breach,
    })
}

pub fn handle_pb_get_role_list_request(
    state: &mut State,
    _req: PbGetRoleListRequest,
) -> Result<PbGetRoleListResponse, String> {
    let selected_main_role = role_helper::selected_main_role_id(
        &state.assets,
        &state.player_components.role,
        state.player_components.basic.info.attributes.sex,
        0,
    );
    let role_list = state
        .player_components
        .role
        .role_map
        .iter()
        .filter_map(|(&rid, info)| {
            if !(1000..=1999).contains(&rid) {
                return None;
            }
            if selected_main_role
                .is_some_and(|selected| role_helper::is_main_role(&state.assets, rid) && rid != selected)
            {
                return None;
            }
            Some(roleInfo {
                RoleId: rid,
                Level: info.level,
                Exp: info.exp,
                Breakthrough: info.breakthrough,
                ..Default::default()
            })
        })
        .collect();
    Ok(PbGetRoleListResponse {
        Code: Some(ErrorCode::Success),
        RoleList: role_list,
    })
}

pub fn handle_character_death_request(
    _state: &mut State,
    _req: CharacterDeathRequest,
) -> Result<CharacterDeathResponse, String> {
    Ok(CharacterDeathResponse {})
}
