use std::collections::HashSet;
use std::io;

use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::asset_indexes::{
    self, CellRange, entity_world_pos, entity_world_rot, is_spawnable_blueprint_and_template,
};
use sonoro_gamesv_data::tables::blueprint_config::{EntityLogic, EntityType as BpEntityType};
use sonoro_gamesv_data::tables::entity_components::components::gravity_flip_from_default;
use sonoro_gamesv_data::tables::entity_components::extra::{ExtraEntry, ExtraKind};
use sonoro_gamesv_data::tables::entity_components::interact::InteractComponent as InteractConfig;
use sonoro_gamesv_data::tables::level_entity_config::LevelEntityConfig;
use sonoro_gamesv_fs::formation_info::FormationRole;
use sonoro_gamesv_fs::progress_info::ProgressInfo;
use sonoro_gamesv_fs::quest_info::QuestInfo;
use sonoro_gamesv_fs::scene_instance::ScenePlayer;
use sonoro_proto::enums::{
    EAttributeType, ERemoveEntityType, GravityFlipType, InfrStatusPb, MoveStatus, SceneMode,
};
use sonoro_proto::pb::{
    AfterJoinSceneNotify, CombatReceivePackNotify, ElevatorMoveNotify, EntityRemoveInfo,
    EntityRemoveNotify, ExploreSkillRoulette, ExploreSkillRouletteUpdateNotify,
    ExploreToolAllNotify, FightFormationNotifyInfo, FightRoleInfo, FightRoleInfos, FlowStartNotify,
    FormationAttr, FormationAttrNotify, FormationRoleInfo, GravityFlipUpdateTypeNotify,
    InfrOneRoad, InfrRoadPb, InfrRoadUpdateNotify, JSPatchNotify, JoinSceneNotify,
    MovePackageNotify, MoveReplaySample, MoveSplineStatusNotify, MovingEntityData,
    PlayerFightFormations, PlayerSceneAoiData, Rotator, SceneAreaState, SceneInformation,
    SceneItemMoveTargetInfo, SceneItemMoveTargetNotify, SceneItemSplineRuntimeData,
    ScenePlayerInformation, SceneRoadSyncNotify, SwitchBattleModeNotify, TimeCheckNotify,
    TransitionOptionPb, UpdateFormationNotify, Vector, VisionExploreSkillNotify,
};
use sonoro_proto::wire::PbEnum;

use crate::commands::entity as entity_cmd;
use crate::component::entity::small::{
    ConfigType, EntityType, LiftComponent, MoveSplineComponent, MoveToPointComponent,
    SceneItemComponent,
};
use crate::component::entity::{EntityComponentStorage, InteractStateComponent};
use crate::component::player::{
    PlayerBasicComponent, PlayerEchoComponent, PlayerGuideComponent, PlayerInventoryComponent,
    PlayerMailComponent, PlayerMapComponent, PlayerMotorComponent, PlayerProgressComponent,
    PlayerQuestComponent, PlayerRoleComponent, PlayerSceneComponent, PlayerWeaponComponent,
};
use crate::event_queue::EventQueue;
use crate::events::{Event, PendingFlow};
use crate::helpers::condition::EvalContext;
use crate::helpers::level_play::{self, LevelPlayContext};
use crate::helpers::quest_types::{ActivityNotifyFn, QuestContext};
use crate::helpers::{
    autopilot, condition, quest_actions, quest_engine, role as role_helper, spline_state,
    trial_roles,
};
use crate::scene::{DeferredJoinFlow, Scene, now_millis};
use crate::schedulers::{BuffTimerScheduler, DirtySaveQueue};
use crate::sink::PushSink;
use crate::templates::{player_entity_templates, role_entity_templates};

const LAHAI_ROI_DUNGEON_ID: i32 = 906;
const ROULETTE_SLOT_COUNT: usize = 8;
/// Most entities the world spawner keeps in a scene.
const SCENE_ENTITY_CAP: usize = 240;

fn ensure_valid_formation(
    formation_info: &mut sonoro_gamesv_fs::formation_info::FormationInfo,
    basic_comp: &PlayerBasicComponent,
) {
    let cur = &mut formation_info.formations[0];
    let is_empty = cur.roles.iter().all(|r| r.is_none());
    if is_empty {
        let rover_id = if basic_comp.info.attributes.sex == 1 {
            1501
        } else {
            1502
        };
        let starter_roles = [rover_id, 1402, 1202];
        cur.cur_role = starter_roles[0];
        for (i, &role_id) in starter_roles.iter().enumerate() {
            cur.roles[i] = Some(FormationRole {
                role_id,
                entity_id: -1,
                on_stage_without_control: false,
            });
        }
        formation_info.cur_formation = 0;
    } else if formation_info.cur_formation < 0 || formation_info.cur_formation >= 20 {
        formation_info.cur_formation = 0;
    }
}

fn roulette_skill_ids(ids: &[i32]) -> Vec<i32> {
    let mut out = vec![0; ROULETTE_SLOT_COUNT];
    let count = ids.len().min(ROULETTE_SLOT_COUNT);
    out[..count].copy_from_slice(&ids[..count]);
    out
}

fn notify_transport_roadways<S: PushSink + ?Sized>(
    fs: &FileSystem,
    scene: &Scene,
    sink: &mut S,
    assets: &Assets,
) {
    let Some(instance_dungeon) = assets
        .tables
        .instance_dungeon
        .get_data_by_id(&scene.instance_id)
    else {
        return;
    };
    let Ok(roads) = autopilot::road_ids_for_map(assets, fs, instance_dungeon.MapConfigId) else {
        return;
    };
    if roads.is_empty() {
        return;
    }
    sink.push(
        SceneRoadSyncNotify::MSG_ID,
        &SceneRoadSyncNotify {
            InstanceId: scene.instance_id,
            EnabledRoads: roads,
            ..Default::default()
        },
    );
}

fn notify_infrastructure_road_data<S: PushSink + ?Sized>(sink: &mut S, assets: &Assets) {
    let roads: Vec<InfrOneRoad> = assets
        .tables
        .infr_road_build
        .items
        .iter()
        .filter(|cfg| cfg.DungeonId == LAHAI_ROI_DUNGEON_ID)
        .map(|cfg| InfrOneRoad {
            RoadId: cfg.Id,
            status: Some(InfrStatusPb::InfrStatusComplete),
            CompleteTime: 0,
            TotalGiftCount: 0,
            LastGiftTime: 0,
        })
        .collect();
    sink.push(
        InfrRoadUpdateNotify::MSG_ID,
        &InfrRoadUpdateNotify {
            RoadInfo: Some(InfrRoadPb {
                Roads: roads,
                ..Default::default()
            }),
        },
    );
}

fn opt_key_int(info: &ProgressInfo, key: &str) -> Option<i64> {
    info.dynamic.ints.iter().find(|e| e.key == key).map(|e| e.val)
}

fn stored_world_float(info: &ProgressInfo, config_id: i32, field: &str) -> Option<f32> {
    let key = format!("world_pos:{config_id}:{field}");
    let raw = opt_key_int(info, &key)?;
    let bits = u32::try_from(raw).ok()?;
    let value = f32::from_bits(bits);
    value.is_finite().then_some(value)
}

fn stored_world_location(info: &ProgressInfo, config_id: i32) -> Option<[f32; 3]> {
    Some([
        stored_world_float(info, config_id, "x")?,
        stored_world_float(info, config_id, "y")?,
        stored_world_float(info, config_id, "z")?,
    ])
}

fn stored_progress_float(info: &ProgressInfo, key: &str) -> Option<f32> {
    let raw = opt_key_int(info, key)?;
    let bits = u32::try_from(raw).ok()?;
    let value = f32::from_bits(bits);
    value.is_finite().then_some(value)
}

fn push_stored_world_transform<S: PushSink + ?Sized>(
    scene: &Scene,
    sink: &mut S,
    progress_info: &ProgressInfo,
    config_id: i32,
    net_id: i64,
) {
    if stored_world_float(progress_info, config_id, "roll").is_none()
        && stored_world_float(progress_info, config_id, "pitch").is_none()
        && stored_world_float(progress_info, config_id, "yaw").is_none()
    {
        return;
    }
    let Some(position) = scene.entity(net_id).and_then(|s| s.position) else {
        return;
    };
    sink.push(
        MovePackageNotify::MSG_ID,
        &MovePackageNotify {
            MovingEntities: vec![MovingEntityData {
                EntityId: net_id,
                Originator: net_id,
                MoveInfos: vec![MoveReplaySample {
                    Location: Some(Vector {
                        X: position.location[0],
                        Y: position.location[1],
                        Z: position.location[2],
                    }),
                    Rotation: Some(Rotator {
                        Roll: position.rotation[0],
                        Pitch: position.rotation[1],
                        Yaw: position.rotation[2],
                    }),
                    ..Default::default()
                }],
                ForcePush: true,
            }],
        },
    );
}

fn push_stored_elevator_state<S: PushSink + ?Sized>(
    sink: &mut S,
    progress_info: &ProgressInfo,
    config_id: i32,
    net_id: i64,
) {
    if config_id <= 0 {
        return;
    }
    let key = format!("elevator:{config_id}");
    let Some(raw) = opt_key_int(progress_info, &key) else {
        return;
    };
    let Ok(floor) = i32::try_from(raw) else {
        return;
    };
    sink.push(
        ElevatorMoveNotify::MSG_ID,
        &ElevatorMoveNotify {
            EntityId: net_id,
            Location: floor,
            MoveType: None,
        },
    );
}

fn entity_interact_config(
    assets: &Assets,
    scene: &Scene,
    config_id: i32,
) -> Option<InteractConfig> {
    let config = assets.find_level_entity(scene.instance_id, i64::from(config_id))?;
    let mut components = config.ComponentsData.clone();
    if let Some(tpl) = assets.find_template_config(&config.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut components);
    }
    components.InteractComponent
}

fn apply_stored_world_entity_overrides(
    scene: &mut Scene,
    assets: &Assets,
    fs: &FileSystem,
    progress_comp: &PlayerProgressComponent,
    config_id: i32,
    net_id: i64,
) -> io::Result<Option<GravityFlipType>> {
    if config_id <= 0 || scene.entity(net_id).is_none() {
        return Ok(None);
    }
    let info = &progress_comp.info;
    let mut changed = false;

    if let (Some(x), Some(y), Some(z)) = (
        stored_world_float(info, config_id, "x"),
        stored_world_float(info, config_id, "y"),
        stored_world_float(info, config_id, "z"),
    ) && let Some(storage) = scene.entity_mut(net_id)
    {
        let mut position = storage.position.unwrap_or_default();
        position.location = [x, y, z];
        if let Some(roll) = stored_world_float(info, config_id, "roll") {
            position.rotation[0] = roll;
        }
        if let Some(pitch) = stored_world_float(info, config_id, "pitch") {
            position.rotation[1] = pitch;
        }
        if let Some(yaw) = stored_world_float(info, config_id, "yaw") {
            position.rotation[2] = yaw;
        }
        storage.position = Some(position);
        changed = true;
    }

    if let Some(raw_camp) = opt_key_int(info, &format!("world_camp:{config_id}"))
        && let Ok(camp32) = i32::try_from(raw_camp)
        && let Some(storage) = scene.entity_mut(net_id)
    {
        storage.config.camp = camp32;
        changed = true;
    }

    if let Some(raw_lift) = opt_key_int(info, &format!("lift_target:{config_id}"))
        && let Ok(lift_loc) = i32::try_from(raw_lift)
        && let Some(storage) = scene.entity_mut(net_id)
    {
        storage.world_item.lift = Some(LiftComponent { location: lift_loc });
        changed = true;
    }

    let mut gravity_type = None;
    if let Some(raw) = opt_key_int(info, &format!("gravity:{config_id}")) {
        if (0..=3).contains(&raw) {
            gravity_type = Some(GravityFlipType::from_i32_or_default(raw as i32));
            if let Some(storage) = scene.entity_mut(net_id) {
                storage.world_item.gravity_flip = Some(raw as i32);
            }
            changed = true;
        }
    } else if let Some(lcfg) = assets.find_level_entity(scene.instance_id, i64::from(config_id)) {
        let mut merged_grav = lcfg.ComponentsData.clone();
        if let Some(tpl) = assets.find_template_config(&lcfg.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut merged_grav);
        }
        if let Some(dg) = merged_grav
            .GravityFlipComponent
            .as_ref()
            .and_then(|gfc| gfc.DefaultGravity)
            && let Some(storage) = scene.entity_mut(net_id)
        {
            storage.world_item.gravity_flip = gravity_flip_from_default(dg);
        }
    }

    if let Some(lcfg) = assets.find_level_entity(scene.instance_id, i64::from(config_id)) {
        let mut merged = lcfg.ComponentsData.clone();
        if let Some(tpl) = assets.find_template_config(&lcfg.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut merged);
        }
        let mut fan_turns_val = None;
        let mut hook_disabled_val = None;
        for extra_slice in [&merged.extra, &merged.template_extra] {
            for entry in extra_slice {
                match entry {
                    ExtraEntry::FanComponent(fc) => {
                        let stored = opt_key_int(info, &format!("fan_turns:{net_id}"))
                            .or_else(|| opt_key_int(info, &format!("fan_turns:{config_id}")));
                        let turns = stored
                            .and_then(|v| i32::try_from(v).ok())
                            .or_else(|| fc.InitCircle.and_then(|ic| i32::try_from(ic).ok()))
                            .unwrap_or(0);
                        fan_turns_val = Some(turns);
                    }
                    ExtraEntry::HookLockPoint(hlp) => {
                        let is_active = opt_key_int(info, &format!("hook_point:active:{net_id}"))
                            .or_else(|| opt_key_int(info, &format!("hook_point:active:{config_id}")))
                            == Some(1);
                        let default_disabled =
                            hlp.DisableAtCreateStage.unwrap_or(false) || hlp.Disabled.unwrap_or(false);
                        hook_disabled_val = Some(if is_active { false } else { default_disabled });
                    }
                    _ => {}
                }
            }
        }
        if let Some(storage) = scene.entity_mut(net_id) {
            if let Some(t) = fan_turns_val {
                storage.world_item.fan_turns = Some(t);
            }
            if let Some(hd) = hook_disabled_val {
                storage.world_item.hook_disabled = Some(hd);
            }
            storage.world_item.be_controlled_present = merged.TeleControl2.is_some();
        }
    }

    let pdir_opt = opt_key_int(info, &format!("patrol:dir:{net_id}"))
        .or_else(|| opt_key_int(info, &format!("patrol:dir:{config_id}")))
        .map(|v| v != 0);
    if let Some(pdir) = pdir_opt
        && let Some(storage) = scene.entity_mut(net_id)
    {
        storage.world_item.patrol_dir = Some(pdir);
    }

    let bc_active = opt_key_int(info, &format!("be_ctrl:active:{net_id}"))
        .or_else(|| opt_key_int(info, &format!("be_ctrl:active:{config_id}")))
        .is_some_and(|v| v != 0);
    let bc_show = opt_key_int(info, &format!("be_ctrl:show:{net_id}"))
        .or_else(|| opt_key_int(info, &format!("be_ctrl:show:{config_id}")))
        .is_some_and(|v| v != 0);
    if bc_active || bc_show {
        let cur_eid = scene.current_role_entity_id();
        if let Some(storage) = scene.entity_mut(net_id) {
            storage.world_item.be_controlled_player_id = bc_active.then_some(cur_eid);
            storage.world_item.be_controlled_show = bc_show;
        }
    }

    let mut spline_entity_id = 0i32;
    let mut spline_active = false;
    let stored_status = info.get_key_int(&format!("scene_item_spline_status:{config_id}"));
    if info.get_key_int(&format!("scene_item_spline_active:{config_id}")) == 1
        && (stored_status == i64::from(MoveStatus::Moving.to_i32())
            || stored_status == i64::from(MoveStatus::Switch.to_i32()))
    {
        spline_entity_id =
            i32::try_from(info.get_key_int(&format!("scene_item_spline_cfg:{config_id}")))
                .unwrap_or(0);
        spline_active = spline_entity_id > 0;
    }
    if !spline_active && info.get_key_int(&format!("quest_spline_active:{config_id}")) == 1 {
        spline_entity_id =
            i32::try_from(info.get_key_int(&format!("quest_spline:{config_id}"))).unwrap_or(0);
        spline_active = spline_entity_id > 0;
    }
    if !spline_active && info.get_key_int(&format!("vehicle:spline_active:{config_id}")) == 1 {
        spline_entity_id =
            i32::try_from(info.get_key_int(&format!("vehicle:spline:{config_id}"))).unwrap_or(0);
        spline_active = spline_entity_id > 0;
    }

    let target_fields = [
        "x",
        "y",
        "z",
        "move_time",
        "stay_time",
        "max_speed",
        "acceleration",
    ];
    let mut target_values = [None; 7];
    for (i, field) in target_fields.iter().enumerate() {
        target_values[i] = stored_progress_float(
            info,
            &format!("scene_item_move_target:{config_id}:{field}"),
        );
    }
    let move_target_active = info.get_key_int(&format!(
        "scene_item_move_target_active:{config_id}"
    )) == 1;

    let interact_cfg = entity_interact_config(assets, scene, config_id);
    let has_batch_bullet = assets
        .find_level_entity(scene.instance_id, i64::from(config_id))
        .is_some_and(|lvl_cfg| {
            let mut merged_bbc = lvl_cfg.ComponentsData.clone();
            if let Some(tpl) = assets.find_template_config(&lvl_cfg.BlueprintType) {
                tpl.ComponentsData.merge_into(&mut merged_bbc);
            }
            merged_bbc.has_extra(ExtraKind::BatchBulletCasterComponent)
        });

    let player_id = scene.player_id;
    let instance_id = scene.instance_id;
    if let Some(storage) = scene.entity_mut(net_id) {
        storage.world_item.move_spline = if spline_active {
            let pos = storage.position.unwrap_or_default();
            Some(MoveSplineComponent {
                move_status: Some(MoveStatus::Moving),
                spline_entity_id,
                runtime_data: Some(SceneItemSplineRuntimeData {
                    CurPos: Some(Vector {
                        X: pos.location[0],
                        Y: pos.location[1],
                        Z: pos.location[2],
                    }),
                    ..Default::default()
                }),
                move_spline_config: spline_state::restore_move_spline_config(info, config_id),
            })
        } else {
            None
        };

        storage.world_item.move_to_point = if move_target_active
            && let (Some(tx), Some(ty), Some(tz)) =
                (target_values[0], target_values[1], target_values[2])
        {
            Some(MoveToPointComponent {
                target_pos: [tx, ty, tz],
                move_type: 0,
                is_exact: false,
                move_time: target_values[3].unwrap_or(0.0),
                stay_time: target_values[4].unwrap_or(0.0),
                max_speed: target_values[5].unwrap_or(0.0),
                acceleration: target_values[6].unwrap_or(0.0),
            })
        } else {
            None
        };

        storage.world_item.scene_item = if storage.config.entity_type == EntityType::SceneItem {
            Some(SceneItemComponent::from_progress(info, player_id, config_id))
        } else {
            None
        };

        storage.world_item.batch_bullet_cast_present = has_batch_bullet;
        storage.world_item.interact = interact_cfg
            .filter(|ic| !ic.Disabled.unwrap_or(false))
            .map(|ic| InteractStateComponent::from_config(&ic, player_id, instance_id, config_id));
    }

    if changed {
        scene.save_entity_by_net_id(fs, net_id)?;
    }
    Ok(gravity_type)
}

fn append_unique_i32(items: &mut Vec<i32>, value: i32) {
    if !items.contains(&value) {
        items.push(value);
    }
}

fn append_completed_road_data_layers(scene_info: &mut SceneInformation, assets: &Assets) {
    let mut repaired_layers = Vec::new();
    let mut unrepaired_layers = Vec::new();
    for cfg in &assets.tables.infr_road_build.items {
        if cfg.DungeonId != LAHAI_ROI_DUNGEON_ID {
            continue;
        }
        for &layer in &cfg.LoadDataLayers {
            append_unique_i32(&mut repaired_layers, layer);
        }
        for &layer in &cfg.UnloadDataLayers {
            append_unique_i32(&mut unrepaired_layers, layer);
        }
    }

    let mut data_layers = Vec::new();
    for &layer in &scene_info.DataLayers {
        if !unrepaired_layers.contains(&layer) {
            append_unique_i32(&mut data_layers, layer);
        }
    }
    for layer in repaired_layers {
        append_unique_i32(&mut data_layers, layer);
    }
    scene_info.DataLayers = data_layers;
}

pub fn handle_scene_pending_updates<S: PushSink + ?Sized>(
    now_ms: i64,
    scene: &mut Scene,
    fs: &FileSystem,
    sink: &mut S,
) {
    let mut data = Vec::new();
    let _ = scene.append_battle_state_notify(&mut data);
    let combine_detaches = std::mem::take(&mut scene.pending_combine_detaches);
    for detach in &combine_detaches {
        data.push(Scene::remove_combine_notify(*detach));
    }
    if !data.is_empty() {
        sink.push(
            CombatReceivePackNotify::MSG_ID,
            &CombatReceivePackNotify { Data: data },
        );
        for detach in &combine_detaches {
            scene.signal_fsm_dissolve_combine(detach.combine_entity_id);
        }
    }

    let mut remove_infos = Vec::new();
    scene.flush_due_killed_removals(fs, now_ms, &mut remove_infos);
    if !remove_infos.is_empty() {
        sink.push(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                IsRemove: true,
                RemoveInfos: remove_infos,
            },
        );
    }
}

pub fn explore_skill_notify<S: PushSink + ?Sized>(scene: &Scene, sink: &mut S) {
    let roulette_info = vec![
        ExploreSkillRoulette {
            SkillIds: roulette_skill_ids(&scene.explore_tools_info.roulette),
            ExtraItemId: scene.explore_tools_info.explore_extra_item_id,
            ExploreSkill: scene.explore_tools_info.active_explore_skill,
        },
        ExploreSkillRoulette {
            SkillIds: roulette_skill_ids(&scene.explore_tools_info.function_roulette),
            ExtraItemId: scene.explore_tools_info.function_extra_item_id,
            ExploreSkill: scene.explore_tools_info.active_function_skill,
        },
        ExploreSkillRoulette::default(),
        ExploreSkillRoulette {
            SkillIds: roulette_skill_ids(&scene.explore_tools_info.motorcycle_roulette),
            ExtraItemId: scene.explore_tools_info.motorcycle_extra_item_id,
            ExploreSkill: scene.explore_tools_info.active_motorcycle_skill,
        },
    ];

    sink.push(
        ExploreToolAllNotify::MSG_ID,
        &ExploreToolAllNotify {
            ExploreSkill: scene.explore_tools_info.active_explore_skill,
            SkillList: scene.explore_tools_info.unlocked_explore_skills.clone(),
            ..Default::default()
        },
    );
    sink.push(
        ExploreSkillRouletteUpdateNotify::MSG_ID,
        &ExploreSkillRouletteUpdateNotify {
            RouletteInfo: roulette_info,
        },
    );
    sink.push(
        VisionExploreSkillNotify::MSG_ID,
        &VisionExploreSkillNotify { ExploreSkill: 1001 },
    );
}

fn has_scene_data(fs: &FileSystem, player_id: i32) -> bool {
    matches!(
        fs.read_dir(&format!("player/{player_id}/scene")),
        Ok(Some(entries)) if entries.iter().any(|e| e.is_dir)
    )
}

fn apply_save_block_pos_rollback(
    scene: &mut Scene,
    assets: &Assets,
    rollback: crate::helpers::save_block::PosRollback,
) {
    if scene.instance.players.is_empty() {
        return;
    }
    let player_loc = scene.instance.players[0].location;
    let Some(range_cfg) =
        assets.find_level_entity(scene.instance_id, i64::from(rollback.range_entity_id))
    else {
        return;
    };
    let Some(center) = entity_world_pos(&range_cfg) else {
        return;
    };
    let Some(shape) = range_cfg
        .ComponentsData
        .RangeComponent
        .as_ref()
        .and_then(|r| r.Shape.as_ref())
    else {
        return;
    };
    let inside = if shape.Type.as_deref().is_some_and(|t| !t.is_empty()) {
        crate::helpers::condition::is_pos_inside_range_shape(
            player_loc,
            center,
            entity_world_rot(&range_cfg),
            shape,
            0.0,
            0.0,
        )
    } else if let Some(radius) = shape.Radius {
        let dx = player_loc[0] - center[0];
        let dy = player_loc[1] - center[1];
        let dz = player_loc[2] - center[2];
        dx * dx + dy * dy + dz * dz <= radius * radius
    } else {
        false
    };
    if !inside {
        return;
    }

    let Some(position_cfg) =
        assets.find_level_entity(scene.instance_id, i64::from(rollback.position_entity_id))
    else {
        return;
    };
    let Some(target) = entity_world_pos(&position_cfg) else {
        return;
    };
    tracing::info!(
        target: "initial_scene_join",
        "login restore: moving player to save block position entity {}",
        rollback.position_entity_id
    );
    scene.instance.players[0].location = target;
    scene.instance.players[0].rotation = entity_world_rot(&position_cfg);
}

#[allow(clippy::too_many_arguments)]
pub fn on_initial_scene_join(
    target_instance_id: Option<i32>,
    location: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    pending_flow: Option<PendingFlow>,
    events: &mut EventQueue,
    fs: &FileSystem,
    player_id: i32,
    assets: &Assets,
    scene_comp: &mut PlayerSceneComponent,
    basic_comp: &PlayerBasicComponent,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    quest_comp: &PlayerQuestComponent,
    cur_scene: &mut Option<Scene>,
    buff_timers: &mut BuffTimerScheduler,
    dirty_saves: &mut DirtySaveQueue,
) -> io::Result<()> {
    let no_scene_data = !has_scene_data(fs, scene_comp.player_id);
    let now_ms = now_millis();

    if let Some(active_scene) = cur_scene.as_mut() {
        DirtySaveQueue::save_all_buffs(fs, active_scene, buff_timers, assets, now_ms)?;
    }
    dirty_saves.flush(
        fs,
        role_comp,
        weapon_comp,
        cur_scene.as_mut(),
        buff_timers,
        assets,
        now_ms,
    )?;

    let preserve_client_black_screen = cur_scene
        .as_ref()
        .is_some_and(|scene| scene.client_black_screen_open);
    if let Some(scene) = cur_scene.take() {
        scene.save_persistent(fs)?;
    }
    buff_timers.reset();

    let requested_inst_id = target_instance_id.unwrap_or(scene_comp.last_scene_info.instance_id);
    let instance_dungeon = match assets
        .tables
        .instance_dungeon
        .get_data_by_id(&requested_inst_id)
    {
        Some(d) => d,
        None => {
            tracing::error!(
                "player({player_id}) last scene instance id {requested_inst_id} doesn't exist; falling back to 8"
            );
            scene_comp.last_scene_info.instance_id = 8;
            let Some(d8) = assets.tables.instance_dungeon.get_data_by_id(&8) else {
                return Ok(());
            };
            d8
        }
    };
    let sync_last_scene = scene_comp.last_scene_info.instance_id != instance_dungeon.Id;
    if sync_last_scene {
        scene_comp.last_scene_info.instance_id = instance_dungeon.Id;
    }

    let mut scene = Scene::init(fs, player_id, instance_dungeon.Id)?;
    scene.client_black_screen_open = preserve_client_black_screen;
    if sync_last_scene {
        scene.save_last_scene(scene_comp, fs)?;
    }

    if scene.instance.players.is_empty() {
        // Official Gorges of Spirits Prologue Spawn for Map 8 (matches 139000025_23 ServerSetPlayerPos)
        let default_born_loc = if instance_dungeon.Id == 8 {
            [-139333.3, 66243.8, 7550.0]
        } else {
            [
                instance_dungeon.BornPosition[0] as f32,
                instance_dungeon.BornPosition[1] as f32,
                instance_dungeon.BornPosition[2] as f32,
            ]
        };
        let default_born_rot = if instance_dungeon.Id == 8 {
            [0.0, 0.0, 30.0]
        } else {
            [
                instance_dungeon.BornRotation[0] as f32,
                instance_dungeon.BornRotation[1] as f32,
                instance_dungeon.BornRotation[2] as f32,
            ]
        };
        scene.instance.players.push(ScenePlayer {
            id: player_id,
            group_type: 1,
            location: location.unwrap_or(default_born_loc),
            rotation: rotation.unwrap_or(default_born_rot),
        });
    } else if let Some(loc) = location {
        scene.instance.players[0] = ScenePlayer {
            id: player_id,
            group_type: scene.instance.players[0].group_type,
            location: loc,
            rotation: rotation.unwrap_or([
                instance_dungeon.BornRotation[0] as f32,
                instance_dungeon.BornRotation[1] as f32,
                instance_dungeon.BornRotation[2] as f32,
            ]),
        };
    }

    if location.is_none()
        && let Some(rollback) = quest_comp.login_restore.pos_rollback
    {
        apply_save_block_pos_rollback(&mut scene, assets, rollback);
    }

    if no_scene_data {
        scene.explore_tools_info.unlocked_explore_skills = assets
            .tables
            .explore_tools
            .items
            .iter()
            .filter(|tool| {
                !(6000..7000).contains(&tool.PhantomSkillId)
                    && (tool.Authorization.is_empty() || tool.PhantomSkillId == 1015)
            })
            .map(|tool| tool.PhantomSkillId)
            .collect();

        let mut roulette = vec![1001, 1003, 1004, 1007];
        for tool in &assets.tables.explore_tools.items {
            if tool.PhantomSkillId == 1009 && !roulette.contains(&1009) {
                roulette.push(1009);
            }
        }
        roulette.resize(ROULETTE_SLOT_COUNT, 0);
        scene.explore_tools_info.roulette = roulette;
        scene.explore_tools_info.active_explore_skill = 1001;
        scene.explore_tools_info.active_function_skill = 0;
        scene.explore_tools_info.motorcycle_roulette = vec![0; ROULETTE_SLOT_COUNT];
        scene.explore_tools_info.active_motorcycle_skill = 0;
    }

    if !scene
        .explore_tools_info
        .unlocked_explore_skills
        .contains(&1015)
    {
        scene.explore_tools_info.unlocked_explore_skills.push(1015);
    }

    trial_roles::materialize_active_trial_roles(
        assets,
        role_comp,
        weapon_comp,
        echo_comp,
        &quest_comp.info.env.trial_roles,
    );
    trial_roles::sanitize_formation_for_login(
        &mut scene.formation_info,
        role_comp,
        basic_comp,
        &quest_comp.info.env.trial_roles,
    );
    ensure_valid_formation(&mut scene.formation_info, basic_comp);

    let new_now_ms = now_millis();
    buff_timers.ensure_all_registered(&scene, assets, new_now_ms);
    buff_timers.sync_all_left_durations(&mut scene, new_now_ms);
    scene.save_persistent(fs)?;

    let p0 = &scene.instance.players[0];
    events.enqueue(Event::SceneSwitch {
        player_id,
        instance_id: instance_dungeon.Id,
        x: p0.location[0],
        y: p0.location[1],
        z: p0.location[2],
        yaw: p0.rotation[2],
        keep_team: true,
        pending_flow,
    });
    *cur_scene = Some(scene);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn notify_join_scene<S: PushSink + ?Sized>(
    pending_flow: Option<PendingFlow>,
    events: &mut EventQueue,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    player_id: i32,
    scene_comp: &mut PlayerSceneComponent,
    basic_comp: &PlayerBasicComponent,
    role_comp: &mut PlayerRoleComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    motor_comp: &PlayerMotorComponent,
    echo_comp: &mut PlayerEchoComponent,
    inventory_comp: &PlayerInventoryComponent,
    map_comp: &PlayerMapComponent,
    quest_comp: &PlayerQuestComponent,
    progress_comp: &PlayerProgressComponent,
    scene: &mut Scene,
    buff_timers: &mut BuffTimerScheduler,
) -> io::Result<()> {
    explore_skill_notify(scene, sink);

    let env_info = &quest_comp.info.env;
    let mut scene_info = SceneInformation {
        InstanceId: scene.instance_id,
        OwnerId: scene.instance.owner_id,
        CurContextId: i64::from(player_id),
        Mode: Some(SceneMode::Single),
        SceneBulletOwnerId: 0,
        TimeInfo: Some(sonoro_proto::pb::SceneTimeInfo {
            Hour: if env_info.time_locked {
                env_info.time_lock_hour
            } else {
                scene.instance.map_time.hour
            },
            Minute: if env_info.time_locked {
                env_info.time_lock_minute
            } else {
                scene.instance.map_time.minute
            },
            OwnerTimeClockTimeSpan: scene.instance.map_time.owner_clock_time_span,
        }),
        ..Default::default()
    };

    let instance_dungeon = match assets
        .tables
        .instance_dungeon
        .get_data_by_id(&scene_comp.last_scene_info.instance_id)
    {
        Some(d) => d.into_owned(),
        None => {
            tracing::error!(
                "player({player_id}) last scene instance id {} doesn't exist; falling back to 8",
                scene_comp.last_scene_info.instance_id
            );
            scene_comp.last_scene_info.instance_id = 8;
            let Some(d8) = assets.tables.instance_dungeon.get_data_by_id(&8) else {
                return Ok(());
            };
            d8.into_owned()
        }
    };

    let join_map_id = assets.resolve_map_config_id(instance_dungeon.Id);
    for dl_cfg in &assets.tables.data_layer_config.items {
        if dl_cfg.InitLoad
            && !dl_cfg.DataLayer.is_empty()
            && (dl_cfg.LevelId == instance_dungeon.Id || dl_cfg.LevelId == join_map_id)
            && !quest_comp.info.world.unloaded_datalayers.contains(&dl_cfg.Id)
        {
            append_unique_i32(&mut scene_info.DataLayers, dl_cfg.Id);
        }
    }
    for &dl in &quest_comp.info.world.loaded_datalayers {
        if dl > 0
            && !quest_comp.info.world.unloaded_datalayers.contains(&dl)
            && let Some(dl_cfg) = assets.tables.data_layer_config.get_data_by_id(&dl)
            && !dl_cfg.DataLayer.is_empty()
            && (dl_cfg.LevelId == 0
                || dl_cfg.LevelId == instance_dungeon.Id
                || dl_cfg.LevelId == join_map_id)
        {
            append_unique_i32(&mut scene_info.DataLayers, dl);
        }
    }
    if instance_dungeon.Id == LAHAI_ROI_DUNGEON_ID {
        append_completed_road_data_layers(&mut scene_info, assets);
    }
    scene_info.LoadedSubLevels = quest_comp
        .info
        .world
        .loaded_sublevels
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
    scene_info.HideSubLevels = quest_comp
        .info
        .world
        .hidden_sublevels
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
    scene_info.AreaStates = env_info
        .area_states
        .iter()
        .map(|a| SceneAreaState {
            AreaId: a.area_id,
            State: a.state,
        })
        .collect();

    let player_entry = scene
        .instance
        .players
        .iter()
        .find(|p| p.id == player_id)
        .cloned()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "PlayerNotFoundInScene"))?;

    let motorcycle_entity =
        player_entity_templates::create_motorcycle_entity(scene, player_id, assets, motor_comp)
            .map_err(io::Error::other)?;
    let player_scene_entity = player_entity_templates::create_player_scene_entity(
        scene,
        player_id,
        assets,
        motorcycle_entity.net_id,
    );
    let _ = player_entity_templates::ensure_motorcycle_companion_entity(
        scene,
        player_id,
        assets,
        motorcycle_entity,
        player_scene_entity,
    )
    .map_err(io::Error::other)?;
    let scene_battle_entity =
        player_entity_templates::create_scene_battle_entity(scene, player_id, assets);
    scene_info.SceneBulletOwnerId = scene_battle_entity.net_id;

    let form_idx = usize::try_from(scene.formation_info.cur_formation).unwrap_or(0);
    let slot_role_ids: Vec<(usize, i32, bool)> = scene
        .formation_info
        .formations
        .get(form_idx)
        .map(|f| {
            f.roles
                .iter()
                .enumerate()
                .filter_map(|(slot_idx, r)| {
                    r.as_ref()
                        .map(|fr| (slot_idx, fr.role_id, fr.on_stage_without_control))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut fight_role_infos_vec = Vec::with_capacity(slot_role_ids.len());
    for (slot_idx, role_id, on_stage_without_control) in slot_role_ids {
        let entity = role_entity_templates::create_role_entity(
            fs,
            scene,
            player_id,
            assets,
            role_comp,
            weapon_comp,
            echo_comp,
            &instance_dungeon,
            role_id,
        )
        .map_err(io::Error::other)?;
        if let Some(slot) = scene.formation_info.formations[form_idx].roles[slot_idx].as_mut() {
            slot.entity_id = entity.net_id;
        }
        fight_role_infos_vec.push(FightRoleInfo {
            RoleId: role_id,
            EntityId: entity.net_id,
            OnStageWithoutControl: on_stage_without_control,
        });
    }
    let _ = role_helper::ensure_alive_cur_role(scene, form_idx);
    let formation_snap = scene.formation_info.formations[form_idx].clone();
    let infos = FightRoleInfos {
        GroupType: player_entry.group_type,
        CurRole: formation_snap.cur_role,
        LivingStatus: Some(role_helper::formation_living_status(scene, &formation_snap)),
        FightRoleInfos: fight_role_infos_vec,
        IsFixedLocation: false,
    };

    scene_info.PlayerInfos.push(ScenePlayerInformation {
        PlayerId: player_id,
        PlayerName: basic_comp.info.attributes.name.as_bytes().to_vec(),
        Level: basic_comp.info.attributes.level,
        GroupType: player_entry.group_type,
        Location: Some(Vector {
            X: player_entry.location[0],
            Y: player_entry.location[1],
            Z: player_entry.location[2],
        }),
        Rotation: Some(Rotator {
            Roll: player_entry.rotation[0],
            Pitch: player_entry.rotation[1],
            Yaw: player_entry.rotation[2],
        }),
        FightRoleInfos: vec![infos],
        CurRole: formation_snap.cur_role,
        VehiclePlayerData: Some(Default::default()),
        PlayerPrefix: 1,
        PlayerGEIncHandle: 0,
        Gravity: Some(Vector::default()),
        ..Default::default()
    });

    let now_ms = now_millis();
    buff_timers.ensure_all_registered(scene, assets, now_ms);
    buff_timers.sync_all_left_durations(scene, now_ms);
    let _ = scene.init_fsm_runtimes(now_ms);
    scene.save(fs)?;

    let mut active_quest_targets = HashSet::new();
    collect_active_target_entity_ids(assets, quest_comp, Some(progress_comp), &mut active_quest_targets);

    let entity_snap: Vec<(i64, i32, EntityType)> = scene
        .entities
        .iter()
        .map(|s| (s.net_id, s.config.config_id, s.config.entity_type))
        .collect();

    let map_id = assets.resolve_map_config_id(scene.instance_id);
    for &(net_id, config_id, entity_type) in &entity_snap {
        if entity_type != EntityType::Player && entity_type != EntityType::PlayerEntity {
            let _ = apply_stored_world_entity_overrides(
                scene,
                assets,
                fs,
                progress_comp,
                config_id,
                net_id,
            )?;
            if config_id > 0
                && let Some(state_name) = saved_spawn_state(
                    assets,
                    quest_comp,
                    map_comp,
                    map_id,
                    i64::from(config_id),
                )
            {
                let _ = scene.update_entity_state_tag(
                    &assets.tables.gameplay_tag_remap,
                    net_id,
                    state_name,
                );
            }
        }
    }

    let mut join_eval_ctx =
        EvalContext::new(basic_comp, echo_comp, inventory_comp, map_comp, quest_comp);
    join_eval_ctx.assets = Some(assets);
    join_eval_ctx.role_comp = Some(role_comp);
    join_eval_ctx.weapon_comp = Some(weapon_comp);
    join_eval_ctx.progress_comp = Some(progress_comp);
    join_eval_ctx.scene = Some(scene);

    let mut stale_net_ids = Vec::new();
    let mut force_hidden_ids = HashSet::new();

    for &(net_id, config_id, entity_type) in &entity_snap {
        if entity_type != EntityType::Player && entity_type != EntityType::PlayerEntity {
            let is_quest_target = active_quest_targets.contains(&config_id);
            let explicitly_shown =
                config_id > 0 && quest_comp.info.world.shown_entities.contains(&config_id);
            let mut should_remove =
                quest_engine::should_hide_world_entity(&quest_comp.info, &active_quest_targets, config_id);
            if !should_remove
                && let Some(lcfg) = assets.find_level_entity(scene.instance_id, i64::from(config_id))
                && (is_blocked_world_entity_config(
                    assets,
                    &quest_comp.info,
                    scene.instance_id,
                    &lcfg,
                    is_quest_target,
                    explicitly_shown,
                ) || !passes_entity_visible_conditions(
                    assets,
                    &join_eval_ctx,
                    &lcfg,
                    is_quest_target,
                    explicitly_shown,
                ))
            {
                should_remove = true;
            }
            if !should_remove && config_id > 0 && !is_quest_target {
                if quest_comp.info.has_opened_chest(config_id)
                    || scene.instance.is_on_respawn_cooldown(config_id, now_ms)
                {
                    should_remove = true;
                } else if !explicitly_shown
                    && let Some(lcfg) =
                        assets.find_level_entity(scene.instance_id, i64::from(config_id))
                    && (lcfg.InSleep || lcfg.IsHidden)
                {
                    force_hidden_ids.insert(net_id);
                }
            }
            if should_remove {
                stale_net_ids.push(net_id);
            }
        }
    }

    for &nid in &force_hidden_ids {
        if let Some(storage) = scene.entity_mut(nid) {
            storage.visible = false;
            storage.actor_visible = false;
        }
    }

    for &(net_id, config_id, entity_type) in &entity_snap {
        if entity_type != EntityType::Player
            && entity_type != EntityType::PlayerEntity
            && config_id > 0
            && !stale_net_ids.contains(&net_id)
        {
            let _ = quest_engine::register_dynamic_interact_options_for_entity(
                scene,
                assets,
                &quest_comp.info,
                Some(progress_comp),
                i64::from(config_id),
            );
        }
    }

    let mut aoi_entities = Vec::with_capacity(scene.entities.len());
    for storage in &scene.entities {
        if stale_net_ids.contains(&storage.net_id) {
            continue;
        }
        let mut pb_ent = storage.to_proto(assets, now_ms);
        entity_cmd::attach_dynamic_interact_infos(scene, &mut pb_ent);
        if force_hidden_ids.contains(&storage.net_id) {
            pb_ent.IsVisible = false;
            pb_ent.IsActorVisible = false;
        }
        aoi_entities.push(pb_ent);
    }

    for nid in stale_net_ids {
        if let Err(err) = scene.remove(fs, nid) {
            tracing::warn!(target: "scene", "failed to remove stale net_id {nid}: {err}");
        }
    }

    scene_info.AoiData = Some(PlayerSceneAoiData {
        Entities: aoi_entities,
        ..Default::default()
    });

    sink.push(
        JoinSceneNotify::MSG_ID,
        &JoinSceneNotify {
            MaxEntityId: 0,
            SceneInfo: Some(scene_info),
            TransitionOption: Some(TransitionOptionPb::default()),
        },
    );
    scene.join_scene_pushed = true;
    scene.scene_loading_finished = false;
    scene.deferred_join_sync_pending = true;
    scene.clear_deferred_join_flow();

    events.enqueue(Event::AfterSceneJoin {
        player_id,
        instance_id: scene.instance_id,
        reason: String::new(),
        pending_flow,
        suppress_area_events: false,
    });
    Ok(())
}

pub fn formation_update_notify<S: PushSink + ?Sized>(
    sink: &mut S,
    scene: &Scene,
    role_comp: &PlayerRoleComponent,
    weapon_comp: &PlayerWeaponComponent,
) {
    let Some(main_scene_player) = scene.instance.players.first() else {
        return;
    };
    let mut formations = Vec::new();
    let life_max_idx = EAttributeType::LifeMax.to_i32() as usize;
    let life_idx = EAttributeType::Life.to_i32() as usize;
    let lv_idx = EAttributeType::Lv.to_i32() as usize;

    for (i, formation) in scene.formation_info.formations.iter().enumerate() {
        if formation.roles.iter().all(|r| r.is_none()) {
            continue;
        }
        let id = (i + 1) as i32;
        let mut role_infos = Vec::new();
        for role in formation.roles.iter().flatten() {
            let Some(role_info) = role_comp.role_map.get(&role.role_id) else {
                tracing::warn!(target: "update_formations", "role {} not found, skipping", role.role_id);
                continue;
            };
            let Some(weapon_info) = weapon_comp.weapon_map.get(&role_info.weapon) else {
                tracing::warn!(
                    target: "update_formations",
                    "weapon {} not found for role {}, skipping",
                    role_info.weapon,
                    role.role_id
                );
                continue;
            };
            let dress_list: Vec<i32> = role_info
                .ornaments
                .iter()
                .filter(|o| o.role_skin_id == role_info.role_skin_id)
                .map(|o| o.ornament_id)
                .collect();

            if role.entity_id != -1
                && let Some(attr) = scene
                    .entity(role.entity_id)
                    .and_then(|s| s.attribute.as_ref())
            {
                role_infos.push(FormationRoleInfo {
                    roleId: role.role_id,
                    MaxHp: attr.attributes[life_max_idx].current,
                    CurHp: attr.attributes[life_idx].current,
                    Level: attr.attributes[lv_idx].current,
                    SkillBranchId: role_info.skill_branch,
                    RoleSkinId: role_info.role_skin_id,
                    WeaponBreachLevel: weapon_info.breach,
                    WeaponId: weapon_info.id,
                    WeaponSkinId: role_info.weapon_skin_id,
                    DressList: dress_list,
                });
                continue;
            }

            role_infos.push(FormationRoleInfo {
                roleId: role.role_id,
                MaxHp: role_info.base_prop.get(life_max_idx).copied().unwrap_or(0),
                CurHp: role_info.base_prop.get(life_idx).copied().unwrap_or(0),
                Level: role_info.level,
                SkillBranchId: role_info.skill_branch,
                RoleSkinId: role_info.role_skin_id,
                WeaponBreachLevel: weapon_info.breach,
                WeaponId: weapon_info.id,
                WeaponSkinId: role_info.weapon_skin_id,
                DressList: dress_list,
            });
        }

        formations.push(FightFormationNotifyInfo {
            FormationId: id,
            Name: formation
                .name
                .as_deref()
                .unwrap_or("")
                .as_bytes()
                .to_vec(),
            CurRole: formation.cur_role,
            RoleInfos: role_infos,
            IsCurrent: usize::try_from(scene.formation_info.cur_formation).ok() == Some(i),
        });
    }

    sink.push(
        UpdateFormationNotify::MSG_ID,
        &UpdateFormationNotify {
            PlayersFormations: vec![PlayerFightFormations {
                Formations: formations,
                PlayerId: main_scene_player.id,
            }],
        },
    );
}

pub fn after_scene_join<S: PushSink + ?Sized>(
    pending_flow: Option<&PendingFlow>,
    events: &mut EventQueue,
    fs: &FileSystem,
    qctx: &mut QuestContext<'_, S>,
    player_id: i32,
) -> io::Result<()> {
    qctx.sink
        .push(AfterJoinSceneNotify::MSG_ID, &AfterJoinSceneNotify::default());
    if let Some(progress) = qctx.progress_comp.as_deref()
        && let Some(scene) = qctx.scene.as_deref()
    {
        for storage in &scene.entities {
            if storage.config.entity_type == EntityType::SceneItem {
                push_stored_elevator_state(
                    qctx.sink,
                    &progress.info,
                    storage.config.config_id,
                    storage.net_id,
                );
            }
        }
    }
    qctx.sink.push(
        SwitchBattleModeNotify::MSG_ID,
        &SwitchBattleModeNotify::default(),
    );
    if let Some(scene) = qctx.scene.as_deref() {
        notify_transport_roadways(fs, scene, qctx.sink, qctx.assets);
    }
    notify_infrastructure_road_data(qctx.sink, qctx.assets);
    quest_engine::triggers::push_ready_quest_list(qctx);

    let loading_finished = qctx
        .scene
        .as_deref()
        .is_some_and(|s| s.scene_loading_finished);
    if loading_finished {
        if let Some(scene) = qctx.scene.as_deref_mut() {
            scene.deferred_join_sync_pending = false;
        }
        quest_actions::sync_world_state_on_join(qctx)?;
        if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(qctx) {
            level_play::sync_on_login(&mut lp_ctx)?;
        }
    } else if let Some(scene) = qctx.scene.as_deref_mut() {
        scene.deferred_join_sync_pending = true;
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let monotonic_now = now_millis();

    let flow_ts = if let Some(scene) = qctx.scene.as_deref_mut() {
        scene.scene_time.reset(now_ms, monotonic_now);
        let _ = scene.init_fsm_runtimes(monotonic_now);
        for storage in &scene.entities {
            if let Some(move_to) = storage.world_item.move_to_point {
                qctx.sink.push(
                    SceneItemMoveTargetNotify::MSG_ID,
                    &SceneItemMoveTargetNotify {
                        MoveInfo: Some(SceneItemMoveTargetInfo {
                            EntityId: storage.net_id,
                            Location: Some(Vector {
                                X: move_to.target_pos[0],
                                Y: move_to.target_pos[1],
                                Z: move_to.target_pos[2],
                            }),
                            MoveTime: move_to.move_time,
                            StayTime: move_to.stay_time,
                            MaxSpeed: move_to.max_speed,
                            Acceleration: move_to.acceleration,
                        }),
                    },
                );
            }
        }
        scene.scene_time.current_flow_timestamp(monotonic_now)
    } else {
        0
    };

    qctx.sink.push(
        TimeCheckNotify::MSG_ID,
        &TimeCheckNotify {
            ClientTime: 0,
            ServerTime: now_ms,
            ServerCombatTime: now_ms,
            ServerStopTime: now_ms,
            ServerFlowTimestamp: flow_ts,
        },
    );
    events.enqueue(Event::UpdateFormations {
        player_id,
        formation_id: 0,
        role_ids: Vec::new(),
    });

    qctx.sink.push(
        FormationAttrNotify::MSG_ID,
        &FormationAttrNotify {
            Duration: 1_534_854_458,
            FormationAttrs: vec![
                FormationAttr {
                    AttrId: 1,
                    Ratio: 2400,
                    BaseMaxValue: 24000,
                    MaxValue: 24000,
                    CurrentValue: 22000,
                },
                FormationAttr {
                    AttrId: 10,
                    Ratio: 2400,
                    BaseMaxValue: 7000,
                    MaxValue: 15000,
                    CurrentValue: 7000,
                },
                FormationAttr {
                    AttrId: 14,
                    Ratio: 0,
                    BaseMaxValue: 10000,
                    MaxValue: 10000,
                    CurrentValue: 10000,
                },
            ],
        },
    );

    if let Some(raw_wm) = fs.read_file("assets/scripts/join_scene_patches/uid_watermark.js")? {
        let content = String::from_utf8_lossy(&raw_wm).replace("{PLR_UID}", &player_id.to_string());
        qctx.sink.push(
            JSPatchNotify::MSG_ID,
            &JSPatchNotify {
                Content: content.into(),
            },
        );
    }

    for path in [
        "assets/scripts/join_scene_patches/disable_watermask.js",
        "assets/scripts/join_scene_patches/unlock_soar_flight.js",
        "assets/scripts/join_scene_patches/dismount_vehicle_on_teleport.js",
        "assets/scripts/join_scene_patches/entity_source_map_sync.js",
        "assets/scripts/join_scene_patches/special_energy_bar_lili.js",
    ] {
        if let Some(content) = fs.read_file(path)? {
            qctx.sink.push(
                JSPatchNotify::MSG_ID,
                &JSPatchNotify {
                    Content: String::from_utf8_lossy(&content).into_owned().into(),
                },
            );
        }
    }

    if let Some(scene) = qctx.scene.as_deref_mut() {
        if let Some(p0) = scene.instance.players.first() {
            scene.last_spatial_check_loc = p0.location;
        }
        if let Some(flow) = pending_flow {
            if scene.scene_loading_finished {
                qctx.sink.push(
                    FlowStartNotify::MSG_ID,
                    &FlowStartNotify {
                        FlowIncId: i64::from(flow.start_action_id),
                        FlowListName: flow.flow_list_name.as_bytes().to_vec(),
                        FlowId: flow.flow_id,
                        StateId: flow.state_id,
                        ..Default::default()
                    },
                );
            } else {
                scene.deferred_join_flow = Some(DeferredJoinFlow {
                    inc_id: flow.start_action_id,
                    namespace: flow.flow_list_name.clone(),
                    id: flow.flow_id,
                    state: flow.state_id,
                });
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn handle_spatial_spawns_check<S: PushSink + ?Sized>(
    loc: [f32; 3],
    events: &mut EventQueue,
    scene: &mut Scene,
    scene_comp: Option<&mut PlayerSceneComponent>,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    basic_comp: &mut PlayerBasicComponent,
    inventory_comp: &mut PlayerInventoryComponent,
    weapon_comp: &mut PlayerWeaponComponent,
    echo_comp: &mut PlayerEchoComponent,
    map_comp: &mut PlayerMapComponent,
    quest_comp: &mut PlayerQuestComponent,
    guide_comp: Option<&mut PlayerGuideComponent>,
    mail_comp: Option<&mut PlayerMailComponent>,
    progress_comp: &mut PlayerProgressComponent,
    role_comp: &mut PlayerRoleComponent,
    activity_notify: Option<ActivityNotifyFn<S>>,
) -> io::Result<()> {
    {
        let mut lp_ctx = LevelPlayContext {
            assets,
            fs,
            sink,
            scene,
            scene_comp,
            events,
            basic_comp,
            inventory_comp,
            weapon_comp,
            echo_comp,
            map_comp,
            quest_comp,
            role_comp: Some(role_comp),
            guide_comp,
            mail_comp,
            progress_comp: Some(progress_comp),
            activity_notify,
        };
        level_play::on_player_move(&mut lp_ctx, loc[0], loc[1], loc[2])?;
    }

    spawn_nearby_world_entities(
        events,
        scene,
        fs,
        assets,
        sink,
        basic_comp,
        inventory_comp,
        weapon_comp,
        echo_comp,
        map_comp,
        quest_comp,
        progress_comp,
        role_comp,
        loc,
        20000.0,
        64,
        now_millis(),
    )
}

#[allow(clippy::too_many_arguments)]
fn evict_distant_ambient_entities<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    quest_info: &QuestInfo,
    active_quest_targets: &HashSet<i32>,
    player_loc: [f32; 3],
    evict_radius: f32,
    skip_priority: bool,
) {
    let evict_r2 = evict_radius * evict_radius;
    let mut victim_ids = Vec::new();
    for s in &scene.entities {
        if is_kept_world_entity(s, scene, assets, quest_info, active_quest_targets) {
            continue;
        }
        if skip_priority
            && s.config.config_id > 0
            && let Some(raw_cfg) =
                assets.find_level_entity(scene.instance_id, i64::from(s.config.config_id))
            && is_priority_world_entity(assets, &raw_cfg)
        {
            continue;
        }

        let Some(pos) = s.position else {
            continue;
        };
        let dx = pos.location[0] - player_loc[0];
        let dy = pos.location[1] - player_loc[1];
        let dz = pos.location[2] - player_loc[2];
        if dx * dx + dy * dy + dz * dz <= evict_r2 {
            continue;
        }
        victim_ids.push(s.net_id);
    }
    remove_evicted_entities(scene, fs, sink, victim_ids);
}

/// Entities that eviction never removes: players, followers, vehicles, summons, battle and quest
/// targets, and level entities that are shown, have a saved state or belong to an entered level
/// play.
fn is_kept_world_entity(
    s: &EntityComponentStorage,
    scene: &Scene,
    assets: &Assets,
    quest_info: &QuestInfo,
    active_quest_targets: &HashSet<i32>,
) -> bool {
    if s.player_id.is_some()
        || s.follower.is_some()
        || s.vehicle.is_some()
        || s.summoner.is_some()
        || s.concomitant.is_some()
    {
        return true;
    }
    if !matches!(
        s.config.entity_type,
        EntityType::Monster
            | EntityType::Animal
            | EntityType::Npc
            | EntityType::SceneItem
            | EntityType::Vision
    ) {
        return true;
    }
    if scene.active_battle_entities.contains(&s.net_id)
        || active_quest_targets.contains(&s.config.config_id)
    {
        return true;
    }
    s.config.config_id > 0
        && (quest_info.world.shown_entities.contains(&s.config.config_id)
            || quest_info.get_entity_state(i64::from(s.config.config_id)).is_some()
            || assets
                .indexes
                .level_play_indices_for_ref_entity(s.config.config_id)
                .iter()
                .any(|&lp_idx| {
                    let lpid = assets.tables.level_play_data.items[lp_idx as usize].LevelPlayId;
                    quest_info.progress.entered_level_plays.contains(&lpid)
                }))
}

fn remove_evicted_entities<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    sink: &mut S,
    net_ids: Vec<i64>,
) {
    let mut remove_infos = Vec::new();
    for net_id in net_ids {
        if scene.remove(fs, net_id).is_ok() {
            remove_infos.push(EntityRemoveInfo {
                EntityId: net_id,
                Type: Some(ERemoveEntityType::RemoveTypeNormal),
            });
        }
    }
    if !remove_infos.is_empty() {
        sink.push(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                RemoveInfos: remove_infos,
                IsRemove: true,
            },
        );
    }
}

/// Client `IComponentDefine.AoiXyLayerValues`, keyed by BaseInfoComponent.AoiLayer
/// (Aki.TdInterface.dll, `IComponentDefine..cctor`). No BinData or assets.db table has them.
const AOI_XY_LAYER_VALUES: [(i32, f32); 8] = [
    (0, 6000.0),
    (1, 24000.0),
    (2, 99000.0),
    (3, 11000.0),
    (6, 3000.0),
    (7, 9000.0),
    (8, 14000.0),
    (9, 49000.0),
];
/// Client `IComponentDefine.AoizLayerValues`, keyed by BaseInfoComponent.AoiZRadius (same
/// source); -1 means no Z limit.
const AOI_Z_LAYER_VALUES: [(i32, f32); 5] = [
    (0, -1.0),
    (1, 24000.0),
    (2, 6000.0),
    (3, 3000.0),
    (4, 50000.0),
];

/// Value of an AOI layer key; a missing or unknown key uses the Default (0) entry.
fn aoi_layer_value(table: &[(i32, f32)], key: Option<i32>) -> f32 {
    let key = key.unwrap_or(0);
    table
        .iter()
        .find(|e| e.0 == key)
        .or_else(|| table.first())
        .map_or(0.0, |e| e.1)
}

/// A level entity's client AOI radii: (XY, Z), from its own BaseInfoComponent or its template's.
/// Z is `None` when the client has no Z limit.
fn client_aoi_radii(assets: &Assets, cfg: &LevelEntityConfig) -> (f32, Option<f32>) {
    let own_info = cfg.ComponentsData.BaseInfoComponent.as_ref();
    let mut layer = own_info.and_then(|bi| bi.AoiLayer);
    let mut z_layer = own_info.and_then(|bi| bi.AoiZRadius);
    if (layer.is_none() || z_layer.is_none())
        && let Some(tpl) = assets.find_template_config(&cfg.BlueprintType)
        && let Some(tpl_info) = tpl.ComponentsData.BaseInfoComponent.as_ref()
    {
        layer = layer.or(tpl_info.AoiLayer);
        z_layer = z_layer.or(tpl_info.AoiZRadius);
    }
    let z = aoi_layer_value(&AOI_Z_LAYER_VALUES, z_layer);
    (
        aoi_layer_value(&AOI_XY_LAYER_VALUES, layer),
        (z >= 0.0).then_some(z),
    )
}

/// The Z half of the client's AOI test: no limit, or |dz| below it.
fn within_client_aoi_z(z_radius: Option<f32>, dz: f32) -> bool {
    z_radius.is_none_or(|z| dz.abs() < z)
}

/// Whether a level entity at `pos` is in the player's AOI the way the client computes it
/// (`FlowActionUtils.CheckEntityInAoi`): 2D distance below the XY value and, unless the Z value
/// is -1, Z distance below it.
fn in_client_aoi(
    assets: &Assets,
    cfg: &LevelEntityConfig,
    pos: [f32; 3],
    player_loc: [f32; 3],
) -> bool {
    let (xy, z) = client_aoi_radii(assets, cfg);
    let dx = pos[0] - player_loc[0];
    let dy = pos[1] - player_loc[1];
    dx * dx + dy * dy < xy * xy && within_client_aoi_z(z, pos[2] - player_loc[2])
}

/// Full scene: in spawn order, a candidate inside its client AOI takes the slot of an evictable
/// level entity outside its own AOI (farthest first) or, when none is left, of a farther
/// non-priority one. Priority entities inside their AOI keep their slot. Level plays and quests
/// spawn past the cap, so the first slot also costs the entities over it. Candidates that get no
/// slot are dropped from the pools; the evicted entities are removed with EntityRemoveNotify.
#[allow(clippy::too_many_arguments)]
fn make_room_for_candidates<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    quest_info: &QuestInfo,
    active_quest_targets: &HashSet<i32>,
    player_loc: [f32; 3],
    pools: [&mut Vec<SpawnCandidate>; 2],
) {
    let mut free = SCENE_ENTITY_CAP.saturating_sub(scene.entities.len());
    let mut excess = scene.entities.len().saturating_sub(SCENE_ENTITY_CAP);
    let mut victims = Vec::new();
    for s in &scene.entities {
        if s.config.config_type != ConfigType::Level
            || s.config.config_id <= 0
            || is_kept_world_entity(s, scene, assets, quest_info, active_quest_targets)
        {
            continue;
        }
        let Some(pos) = s.position else {
            continue;
        };
        let Some(cfg) = assets.find_level_entity(scene.instance_id, i64::from(s.config.config_id))
        else {
            continue;
        };
        let in_aoi = in_client_aoi(assets, &cfg, pos.location, player_loc);
        if in_aoi && is_priority_world_entity(assets, &cfg) {
            continue;
        }
        let dx = pos.location[0] - player_loc[0];
        let dy = pos.location[1] - player_loc[1];
        let dz = pos.location[2] - player_loc[2];
        victims.push((s.net_id, dx * dx + dy * dy + dz * dz, in_aoi));
    }
    // Outside the AOI first, then farthest first.
    victims.sort_by(|a, b| a.2.cmp(&b.2).then(b.1.total_cmp(&a.1)));

    let mut evicted = Vec::new();
    for pool in pools {
        pool.retain(|cand| {
            if free > 0 {
                free -= 1;
                return true;
            }
            // Victims are ordered, so the last one needed decides.
            let need = excess + 1;
            if victims
                .get(need - 1)
                .is_none_or(|&(_, dist2, in_aoi)| in_aoi && dist2 <= cand.dist2)
            {
                return false;
            }
            let Some(cfg) = assets.get_level_entity_by_idx(cand.cfg_idx) else {
                return false;
            };
            if !in_client_aoi(assets, &cfg, [cand.x, cand.y, cand.z], player_loc) {
                return false;
            }
            evicted.extend(victims.drain(..need).map(|(net_id, ..)| net_id));
            excess = 0;
            true
        });
    }
    remove_evicted_entities(scene, fs, sink, evicted);
}

#[derive(Clone, Copy)]
struct SpawnCandidate {
    cfg_idx: u32,
    x: f32,
    y: f32,
    z: f32,
    dist2: f32,
}

fn is_priority_world_entity(assets: &Assets, cfg: &LevelEntityConfig) -> bool {
    if cfg.BlueprintType.contains("Treasure") || cfg.BlueprintType.contains("Teleport") {
        return true;
    }
    let Some(bp) = assets.indexes.find_blueprint_config(&assets.tables, &cfg.BlueprintType) else {
        return false;
    };
    if bp.EntityLogic == EntityLogic::Vehicle || bp.EntityLogic == EntityLogic::Npc {
        return true;
    }
    matches!(
        bp.EntityType,
        BpEntityType::AirWallEffectTrigger
            | BpEntityType::AirPassage
            | BpEntityType::ChallengeInteract
            | BpEntityType::DungeonEntry
            | BpEntityType::DungeonEntryWithTrigger
            | BpEntityType::ElementalNpc
            | BpEntityType::ExploreSkillGroupInteractor
            | BpEntityType::ExploreSkillInteractor
            | BpEntityType::GameplayInteractGear
            | BpEntityType::GravityFlip
            | BpEntityType::HookLockPoint
            | BpEntityType::HookTreasureBox
            | BpEntityType::HookWithRange
            | BpEntityType::InteractGear
            | BpEntityType::InteractGear2
            | BpEntityType::InteractGearCanHit
            | BpEntityType::InteractGearTrigger
            | BpEntityType::KiteHook
            | BpEntityType::CableWayHook
            | BpEntityType::Lift
            | BpEntityType::Lift2
            | BpEntityType::Npc
            | BpEntityType::Npc2
            | BpEntityType::NpcMonster
            | BpEntityType::PasserbyNpc
            | BpEntityType::RewardNpc
            | BpEntityType::SequenceLift
            | BpEntityType::Portal
            | BpEntityType::NoRenderPortal
            | BpEntityType::NoRenderPortalWithInteract
            | BpEntityType::MoveNoRenderPortal
            | BpEntityType::SceneAura
            | BpEntityType::SimpleInteract
            | BpEntityType::SpawnPasserbyNpc
            | BpEntityType::StateSceneItem
            | BpEntityType::TargetGear
            | BpEntityType::TargetGear2
            | BpEntityType::TeleControl
            | BpEntityType::Teleporter
            | BpEntityType::TreasureBox
            | BpEntityType::TuanziNpc
            | BpEntityType::VehicleNpc
            | BpEntityType::VehicleSceneItem
            | BpEntityType::VisionTreasureBox
            | BpEntityType::WaterSpout
    )
}

fn insert_spawn_candidate(buf: &mut Vec<SpawnCandidate>, cap: usize, cand: SpawnCandidate) {
    if cap == 0 {
        return;
    }
    if buf.len() < cap {
        buf.push(cand);
    } else if let Some(last) = buf.last_mut()
        && cand.dist2 < last.dist2
    {
        *last = cand;
    } else {
        return;
    }
    let mut j = buf.len() - 1;
    while j > 0 && buf[j].dist2 < buf[j - 1].dist2 {
        buf.swap(j, j - 1);
        j -= 1;
    }
}

fn passes_entity_visible_conditions(
    assets: &Assets,
    eval_ctx: &EvalContext<'_>,
    cfg: &LevelEntityConfig,
    is_quest_target: bool,
    explicitly_shown: bool,
) -> bool {
    if is_quest_target || explicitly_shown {
        return true;
    }
    let mut comps = cfg.ComponentsData.clone();
    if let Some(tpl) = assets.find_template_config(&cfg.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut comps);
    }
    let mut vis_ctx = eval_ctx.clone();
    vis_ctx.self_entity_config_id = i32::try_from(cfg.EntityId).unwrap_or(0);
    condition::eval_entity_visible_component(&vis_ctx, &comps)
}

#[allow(clippy::too_many_arguments)]
fn consider_spawn_candidates(
    assets: &Assets,
    scene: &Scene,
    quest_info: &QuestInfo,
    eval_ctx: &EvalContext<'_>,
    active_quest_targets: &HashSet<i32>,
    now_ms: i64,
    player_loc: [f32; 3],
    r2: f32,
    indices: &[u32],
    priority_buf: &mut Vec<SpawnCandidate>,
    best_buf: &mut Vec<SpawnCandidate>,
    cap: usize,
) {
    for &cfg_idx in indices {
        let Some(cfg) = assets.get_level_entity_by_idx(cfg_idx) else {
            continue;
        };
        if !quest_actions::entity_data_layers_active(assets, quest_info, scene.instance_id, &cfg) {
            continue;
        }
        let Some(pos) = entity_world_pos(&cfg) else {
            continue;
        };
        let dx = pos[0] - player_loc[0];
        let dy = pos[1] - player_loc[1];
        let dz = pos[2] - player_loc[2];
        let dist2 = dx * dx + dy * dy + dz * dz;
        if dist2 > r2 {
            continue;
        }
        let cfg_id32 = i32::try_from(cfg.EntityId).unwrap_or(0);
        let is_quest_target = cfg_id32 > 0 && active_quest_targets.contains(&cfg_id32);
        let explicitly_shown =
            cfg_id32 > 0 && quest_info.world.shown_entities.contains(&cfg_id32);
        if !is_quest_target
            && !explicitly_shown
            && !within_client_aoi_z(client_aoi_radii(assets, &cfg).1, dz)
        {
            continue;
        }

        let is_priority = is_quest_target || is_priority_world_entity(assets, &cfg);
        if is_priority {
            if priority_buf.len() >= 96 && priority_buf.last().is_some_and(|l| dist2 >= l.dist2) {
                continue;
            }
        } else if cap > 0
            && best_buf.len() >= cap
            && best_buf.last().is_some_and(|l| dist2 >= l.dist2)
        {
            continue;
        }
        if scene.debug_spawn_route(cfg.EntityId).is_some() {
            continue;
        }
        if cfg_id32 > 0
            && (scene.find_entity_net_id_by_config_id(cfg_id32).is_some()
                || (!is_quest_target
                    && (quest_info.has_opened_chest(cfg_id32)
                        || scene.instance.is_on_respawn_cooldown(cfg_id32, now_ms))))
        {
            continue;
        }
        if is_blocked_world_entity_config(
            assets,
            quest_info,
            scene.instance_id,
            &cfg,
            is_quest_target,
            explicitly_shown,
        ) {
            continue;
        }
        if !passes_entity_visible_conditions(assets, eval_ctx, &cfg, is_quest_target, explicitly_shown)
        {
            continue;
        }
        if (cfg.InSleep || cfg.IsHidden) && !explicitly_shown && !is_quest_target {
            continue;
        }
        if quest_engine::should_hide_world_entity(quest_info, active_quest_targets, cfg_id32) {
            continue;
        }

        let cand = SpawnCandidate {
            cfg_idx,
            x: pos[0],
            y: pos[1],
            z: pos[2],
            dist2,
        };
        if is_priority {
            insert_spawn_candidate(priority_buf, 96, cand);
        } else {
            insert_spawn_candidate(best_buf, cap, cand);
        }
    }
}

pub fn is_blocked_world_entity_config(
    assets: &Assets,
    quest_info: &QuestInfo,
    instance_id: i32,
    lcfg: &LevelEntityConfig,
    is_quest_target: bool,
    explicitly_shown: bool,
) -> bool {
    if !quest_actions::entity_data_layers_active(assets, quest_info, instance_id, lcfg) {
        return true;
    }
    if is_quest_target || explicitly_shown {
        return false;
    }
    if lcfg.BlueprintType.starts_with("Editor") {
        return true;
    }

    let tpl_opt = assets.find_template_config(&lcfg.BlueprintType);
    if let Some(bp) = assets
        .indexes
        .find_blueprint_config(&assets.tables, &lcfg.BlueprintType)
    {
        if !is_spawnable_blueprint_and_template(lcfg, bp, tpl_opt.as_deref()) {
            return true;
        }
        if matches!(
            bp.EntityType,
            BpEntityType::TriggerConditionListener
                | BpEntityType::HitConditionListener
                | BpEntityType::InteractiveConditionListener
                | BpEntityType::TargetGearWithConditionListener
        ) {
            let cl_disabled = lcfg
                .ComponentsData
                .ConditionListenerComponent
                .as_ref()
                .and_then(|cl| cl.Disabled)
                .or_else(|| {
                    tpl_opt
                        .as_ref()
                        .and_then(|tpl| tpl.ComponentsData.ConditionListenerComponent.as_ref())
                        .and_then(|tcl| tcl.Disabled)
                });
            if cl_disabled.unwrap_or(false) {
                return true;
            }
        }
    }

    let base_disabled = lcfg
        .ComponentsData
        .BaseInfoComponent
        .as_ref()
        .and_then(|bi| bi.Disabled)
        .or_else(|| {
            tpl_opt
                .as_ref()
                .and_then(|tpl| tpl.ComponentsData.BaseInfoComponent.as_ref())
                .and_then(|tbi| tbi.Disabled)
        });
    if base_disabled.unwrap_or(false) {
        return true;
    }

    if let Ok(cfg_id32) = i32::try_from(lcfg.EntityId)
        && cfg_id32 > 0
    {
        let scene_map = assets.resolve_map_config_id(instance_id);
        for &lp_idx in assets.indexes.level_play_indices_for_reward_entity(cfg_id32) {
            let lp = &assets.tables.level_play_data.items[lp_idx as usize];
            let lp_map = assets.resolve_map_config_id(if lp.Data.InstanceId != 0 {
                lp.Data.InstanceId
            } else {
                lp.Data.LevelId
            });
            if lp.Data.InstanceId != instance_id && lp.Data.LevelId != instance_id && lp_map != scene_map
            {
                continue;
            }
            if !quest_info.completed_level_plays.contains(&lp.LevelPlayId) {
                return true;
            }
        }
        for &lp_idx in assets.indexes.level_play_indices_for_ref_entity(cfg_id32) {
            let lp = &assets.tables.level_play_data.items[lp_idx as usize];
            let lp_map = assets.resolve_map_config_id(if lp.Data.InstanceId != 0 {
                lp.Data.InstanceId
            } else {
                lp.Data.LevelId
            });
            if lp.Data.InstanceId != instance_id && lp.Data.LevelId != instance_id && lp_map != scene_map
            {
                continue;
            }
            if lp.Data.LevelPlayEntityId == cfg_id32 {
                continue;
            }
            if matches!(lp.Data.Type.as_str(), "Quest" | "Dungeon" | "LordGym")
                && !quest_info.progress.entered_level_plays.contains(&lp.LevelPlayId)
                && !quest_info.completed_level_plays.contains(&lp.LevelPlayId)
            {
                return true;
            }
        }
    }
    false
}

fn saved_spawn_state<'a>(
    assets: &Assets,
    quest_comp: &'a PlayerQuestComponent,
    map_comp: &PlayerMapComponent,
    map_id: i32,
    entity_config_id: i64,
) -> Option<&'a str> {
    if let Some(saved) = quest_comp.info.get_entity_state(entity_config_id) {
        return (!saved.is_empty()).then_some(saved);
    }
    is_unlocked_teleporter_entity(assets, map_comp, map_id, entity_config_id)
        .then_some("关卡.Common.状态.激活")
}

fn is_unlocked_teleporter_entity(
    assets: &Assets,
    map_comp: &PlayerMapComponent,
    map_id: i32,
    entity_config_id: i64,
) -> bool {
    if entity_config_id <= 0 {
        return false;
    }
    for &tp_idx in assets.indexes.teleporter_indices_for_map(map_id) {
        let tp = &assets.tables.teleporter.items[tp_idx as usize];
        if tp.TeleportEntityConfigId == entity_config_id
            && map_comp.info.unlocked_teleporters.contains(&tp.Id)
        {
            return true;
        }
    }
    false
}

fn collect_active_target_entity_ids(
    assets: &Assets,
    quest_comp: &PlayerQuestComponent,
    progress_comp: Option<&PlayerProgressComponent>,
    out: &mut HashSet<i32>,
) {
    for aq in &quest_comp.info.active_quests {
        let Ok(nodes) = assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            if let Some(node) = nodes.iter().find(|n| n.id == nid) {
                for eid in quest_engine::extract_node_target_entity_ids(node) {
                    out.insert(eid);
                }
            }
        }
    }
    crate::helpers::level_play::collect_active_target_entity_ids(
        assets,
        quest_comp,
        progress_comp,
        out,
    );
}

struct PendingWorldSpawn {
    handle: crate::events::EntityHandle,
    entity_id: i64,
    source_map_id: i32,
    cfg_id: Option<i32>,
    net_id: Option<i64>,
    gravity_type: Option<GravityFlipType>,
    state_tag_upd: Option<crate::scene::StateTagUpdate>,
    lift_target_floor: Option<i32>,
    is_lift: bool,
}

#[allow(clippy::too_many_arguments)]
fn spawn_nearby_world_entities<S: PushSink + ?Sized>(
    events: &mut EventQueue,
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    basic_comp: &PlayerBasicComponent,
    inventory_comp: &PlayerInventoryComponent,
    weapon_comp: &PlayerWeaponComponent,
    echo_comp: &PlayerEchoComponent,
    map_comp: &PlayerMapComponent,
    quest_comp: &PlayerQuestComponent,
    progress_comp: &PlayerProgressComponent,
    role_comp: &PlayerRoleComponent,
    player_loc: [f32; 3],
    radius: f32,
    max_count: usize,
    now_ms: i64,
) -> io::Result<()> {
    let need_evict = scene.entities.len() >= 200;

    let mut active_quest_targets = HashSet::new();
    collect_active_target_entity_ids(assets, quest_comp, Some(progress_comp), &mut active_quest_targets);

    if need_evict {
        evict_distant_ambient_entities(
            scene,
            fs,
            assets,
            sink,
            &quest_comp.info,
            &active_quest_targets,
            player_loc,
            24000.0,
            true,
        );
        if scene.entities.len() >= 215 {
            evict_distant_ambient_entities(
                scene,
                fs,
                assets,
                sink,
                &quest_comp.info,
                &active_quest_targets,
                player_loc,
                21000.0,
                true,
            );
        }
        if scene.entities.len() >= 230 {
            evict_distant_ambient_entities(
                scene,
                fs,
                assets,
                sink,
                &quest_comp.info,
                &active_quest_targets,
                player_loc,
                32000.0,
                false,
            );
        }
    }

    let cap = max_count.min(40);
    let mut priority_buf = Vec::new();
    let mut best_buf = Vec::new();
    let r2 = radius * radius;
    let map_id = assets.resolve_map_config_id(scene.instance_id);

    let mut eval_ctx =
        EvalContext::new(basic_comp, echo_comp, inventory_comp, map_comp, quest_comp);
    eval_ctx.assets = Some(assets);
    eval_ctx.role_comp = Some(role_comp);
    eval_ctx.weapon_comp = Some(weapon_comp);
    eval_ctx.progress_comp = Some(progress_comp);
    eval_ctx.scene = Some(scene);

    if let Some(CellRange {
        min_x,
        max_x,
        min_y,
        max_y,
    }) = asset_indexes::spatial_cell_range(player_loc[0], player_loc[1], radius)
    {
        for cx in min_x..=max_x {
            for cy in min_y..=max_y {
                let cell_indices = assets.indexes.spawnable_indices_for_map_cell(map_id, cx, cy);
                if cell_indices.is_empty() {
                    continue;
                }
                consider_spawn_candidates(
                    assets,
                    scene,
                    &quest_comp.info,
                    &eval_ctx,
                    &active_quest_targets,
                    now_ms,
                    player_loc,
                    r2,
                    cell_indices,
                    &mut priority_buf,
                    &mut best_buf,
                    cap,
                );
            }
        }
    } else {
        consider_spawn_candidates(
            assets,
            scene,
            &quest_comp.info,
            &eval_ctx,
            &active_quest_targets,
            now_ms,
            player_loc,
            r2,
            assets.indexes.spawnable_indices_for_map(map_id),
            &mut priority_buf,
            &mut best_buf,
            cap,
        );
    }

    if priority_buf.len() + best_buf.len() > SCENE_ENTITY_CAP.saturating_sub(scene.entities.len()) {
        make_room_for_candidates(
            scene,
            fs,
            assets,
            sink,
            &quest_comp.info,
            &active_quest_targets,
            player_loc,
            [&mut priority_buf, &mut best_buf],
        );
    }

    let mut pending = Vec::new();
    for pool in [priority_buf, best_buf] {
        for cand in pool {
            if scene.entities.len() >= SCENE_ENTITY_CAP {
                break;
            }
            let Some(cfg) = assets.get_level_entity_by_idx(cand.cfg_idx) else {
                continue;
            };
            let cfg_id32_hint = i32::try_from(cfg.EntityId).unwrap_or(0);
            let is_qt = cfg_id32_hint > 0 && active_quest_targets.contains(&cfg_id32_hint);
            let exp_shown = cfg_id32_hint > 0
                && quest_comp.info.world.shown_entities.contains(&cfg_id32_hint);
            let mut merged_comps = cfg.ComponentsData.clone();
            if let Some(tpl) = assets.find_template_config(&cfg.BlueprintType) {
                tpl.ComponentsData.merge_into(&mut merged_comps);
            }
            let active_lift = merged_comps
                .LiftComponent
                .as_ref()
                .filter(|lc| !lc.Disabled.unwrap_or(false));
            let is_lift = active_lift.is_some();
            let lift_target_floor = active_lift.and_then(|lc| {
                let floor = opt_key_int(&progress_comp.info, &format!("lift_target:{}", cfg.EntityId))
                    .filter(|&v| v > 0)
                    .or_else(|| {
                        opt_key_int(&progress_comp.info, &format!("elevator:{}", cfg.EntityId))
                            .filter(|&v| v > 0)
                    })
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or_else(|| lc.InitialFloor.unwrap_or(1));
                (floor > 1).then_some(floor)
            });
            let saved_location = if is_lift {
                None
            } else if cfg_id32_hint > 0 {
                stored_world_location(&progress_comp.info, cfg_id32_hint)
            } else {
                None
            };
            let initial_location = saved_location.unwrap_or([cand.x, cand.y, cand.z]);
            let Ok(Some(spawned_ent)) = entity_cmd::spawn_with_location_and_progress(
                events,
                scene,
                fs,
                assets,
                sink,
                basic_comp,
                cfg.EntityId,
                Some(false),
                Some(false),
                Some(cfg.MapId),
                Some(initial_location),
                true,
                &progress_comp.info,
                !(exp_shown || is_qt),
                true,
            ) else {
                continue;
            };

            if let Ok(cfg_id) = i32::try_from(cfg.EntityId)
                && let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id)
            {
                let gravity_type = apply_stored_world_entity_overrides(
                    scene,
                    assets,
                    fs,
                    progress_comp,
                    cfg_id,
                    net_id,
                )?;
                if lift_target_floor.is_some()
                    && let Some(storage) = scene.entity_mut(net_id)
                {
                    storage.world_item.lift = Some(LiftComponent { location: 1 });
                }
                let mut state_tag_upd = None;
                if let Some(state_name) =
                    saved_spawn_state(assets, quest_comp, map_comp, map_id, cfg.EntityId)
                    && let Some(upd) = scene.update_entity_state_tag(
                        &assets.tables.gameplay_tag_remap,
                        net_id,
                        state_name,
                    )
                    && upd.old_tag_id != upd.new_tag_id
                {
                    state_tag_upd = Some(upd);
                }
                quest_engine::register_dynamic_interact_options_for_entity(
                    scene,
                    assets,
                    &quest_comp.info,
                    Some(progress_comp),
                    cfg.EntityId,
                )?;
                pending.push(PendingWorldSpawn {
                    handle: spawned_ent,
                    entity_id: cfg.EntityId,
                    source_map_id: cfg.MapId,
                    cfg_id: Some(cfg_id),
                    net_id: Some(net_id),
                    gravity_type,
                    state_tag_upd,
                    lift_target_floor,
                    is_lift,
                });
            } else {
                pending.push(PendingWorldSpawn {
                    handle: spawned_ent,
                    entity_id: cfg.EntityId,
                    source_map_id: cfg.MapId,
                    cfg_id: None,
                    net_id: None,
                    gravity_type: None,
                    state_tag_upd: None,
                    lift_target_floor,
                    is_lift,
                });
            }
        }
    }

    let batch_items: Vec<(crate::events::EntityHandle, i64, i32)> = pending
        .iter()
        .map(|p| (p.handle, p.entity_id, p.source_map_id))
        .collect();
    let published_handles: HashSet<i64> = entity_cmd::publish_world_spawn_batch(
        scene,
        fs,
        assets,
        sink,
        &batch_items,
    )
    .into_iter()
    .map(|h| h.net_id)
    .collect();

    for item in pending {
        if !published_handles.contains(&item.handle.net_id) {
            continue;
        }
        if let (Some(cfg_id), Some(net_id)) = (item.cfg_id, item.net_id) {
            if !item.is_lift {
                push_stored_world_transform(scene, sink, &progress_comp.info, cfg_id, net_id);
            }
            if let Some(location) = item.lift_target_floor {
                if let Some(storage) = scene.entity_mut(net_id) {
                    storage.world_item.lift = Some(LiftComponent { location });
                }
                sink.push(
                    ElevatorMoveNotify::MSG_ID,
                    &ElevatorMoveNotify {
                        EntityId: net_id,
                        Location: location,
                        MoveType: Some(sonoro_proto::pb::LiftMoveTypePb {
                            Type: Some(sonoro_proto::enums::LiftMoveType::CustomSpeed),
                            CustomSpeed: Some(sonoro_proto::pb::LiftMoveTypeCustomSpeedPb {
                                TotalTime: 0.01,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                    },
                );
            } else if !item.is_lift
                && let Some(location) = opt_key_int(&progress_comp.info, &format!("lift_target:{cfg_id}"))
                    .and_then(|v| i32::try_from(v).ok())
            {
                sink.push(
                    ElevatorMoveNotify::MSG_ID,
                    &ElevatorMoveNotify {
                        EntityId: net_id,
                        Location: location,
                        MoveType: None,
                    },
                );
            }
            push_stored_elevator_state(sink, &progress_comp.info, cfg_id, net_id);
            if let Some(flip_type) = item.gravity_type {
                sink.push(
                    GravityFlipUpdateTypeNotify::MSG_ID,
                    &GravityFlipUpdateTypeNotify {
                        NewGravityFlipType: Some(flip_type),
                        EntityId: net_id,
                    },
                );
            }
            if let Some(storage) = scene.entity(net_id) {
                if let Some(move_spline) = &storage.world_item.move_spline {
                    sink.push(
                        MoveSplineStatusNotify::MSG_ID,
                        &MoveSplineStatusNotify {
                            SceneItemRuntimeData: move_spline.runtime_data.clone(),
                            MoveStatus: move_spline.move_status,
                            EntityId: net_id,
                            SplineEntityId: move_spline.spline_entity_id,
                            MoveSplineConfig: move_spline.move_spline_config.clone(),
                        },
                    );
                }
                if let Some(move_to) = storage.world_item.move_to_point {
                    sink.push(
                        SceneItemMoveTargetNotify::MSG_ID,
                        &SceneItemMoveTargetNotify {
                            MoveInfo: Some(SceneItemMoveTargetInfo {
                                EntityId: net_id,
                                Location: Some(Vector {
                                    X: move_to.target_pos[0],
                                    Y: move_to.target_pos[1],
                                    Z: move_to.target_pos[2],
                                }),
                                MoveTime: move_to.move_time,
                                StayTime: move_to.stay_time,
                                MaxSpeed: move_to.max_speed,
                                Acceleration: move_to.acceleration,
                            }),
                        },
                    );
                }
            }
            quest_actions::sync_persisted_vehicle_npc_links(
                scene,
                progress_comp,
                sink,
                Some(cfg_id),
            );
            if let Some(upd) = item.state_tag_upd {
                quest_actions::emit_entity_state_tag_update_checked(
                    scene,
                    Some(assets),
                    sink,
                    net_id,
                    upd,
                );
            }
        }
    }
    Ok(())
}
