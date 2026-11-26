use std::collections::BTreeMap;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::asset_indexes;
use sonoro_gamesv_data::tables::entity_components::range::ShapeConfig;
use sonoro_gamesv_data::tables::level_entity_config::LevelEntityConfig;
use sonoro_gamesv_fs::inventory_info::InventoryInfo;
use sonoro_gamesv_fs::last_scene_info::LastSceneInfo;
use sonoro_gamesv_fs::quest_info::{QuestInfo, append_unique_int, append_unique_str, remove_int_value, remove_str_value};
use sonoro_proto::enums::{
    BtType, ChangeReason, ERemoveEntityType, ExitVehicleType, GameCtxType, GravityFlipType,
    IllustratedType, MoveStatus, SuspendType, TeleportReason,
};
use sonoro_proto::pb::{
    AddGuestNotify, AlertArea, AlertAreaNotify, AudioStatePb, AudioStateUpdateNotify,
    BehaviorTreeCtxPb, BtSuspendNotify, BtVarUpdateNotify, CommonTagData, ElevatorMoveNotify,
    EnableNearbyTrackingNotify, EntityAddBubbleNotify, EntityAddDynamicInteractNotify, EntityCommonTagNotify, EntityCtxPb,
    EntityPosResetNotify, EntityRemoveBubbleNotify, EntityRemoveDynamicInteractNotify,
    EntityRemoveInfo, EntityRemoveNotify, EntityStateReadyNotify, FlowActionCtxPb,
    FlowShowEntityNotify, GameCtxPb, GameCtxPbCtxInfoOneof, GameplayCueNotify,
    GravityFlipUpdateTypeNotify, IllustratedEntry, IllustratedUnlockNotify, InstanceTimer,
    InstanceTimerNotify, InstResultNotify, InstSettleNotify, LeaveSceneNotify,
    LevelPlayStateNotify, MailInfosNotify, MapMarkShowIdInfoUpdateNotify, ModifyEntityCampNotify,
    MovePackageNotify, MoveReplaySample, MoveSplineConfig, MoveSplineConfigEndPointOneof,
    MoveSplineConfigLookDirOneof, MoveSplineConfigStartPointOneof, MoveSplineStatusNotify,
    MovingEntityData, NormalItem, NormalItemUpdateNotify, OccupationPbInfo,
    OperationRestrictionNotify, PlayerReviveNotify, PlayerVarNotify, RemoveGuestNotify,
    RoleTrialCloseNotify, RoleTrialOpenNotify, Rotator, RoverRogueRoomInfoNotify, SceneAreaState,
    SceneAreaStateNotify, SceneChangeDataLayerNotify, SceneItemMoveTargetInfo,
    SceneItemMoveTargetNotify, SceneItemSplineRuntimeData, SceneSubLevelsChangedNotify,
    SceneTimeInfo, ShortMessageInfo, ShortMessageUpdateNotify, SummonEntityNotify,
    SyncSceneTimeNotify, TeleportNotify, TeleportUpdateNotify, TemporaryTeleportAllInfoNotify,
    TimeLockInfoNotify, TrialRoleItem, VarDefinePb, VarDefinePbValueOneof, Vector,
    VehicleUpdateEntityNotify, VehicleUpdateNotify, WeatherNotify,
};
use sonoro_proto::wire::PbEnum;

use crate::commands::js_patches;
use crate::component::comp_util;
use crate::component::entity::small::{EntityState, EntityType, LiftComponent, MoveSplineComponent};
use crate::component::entity::tag::TagComponent;
use crate::component::player::PlayerProgressComponent;
use crate::events::{BuffAdditionEntry, Event};
use crate::fsm::types::wake_reason;
use crate::handlers::tags as tags_handler;
use crate::helpers::condition::{
    is_pos_inside_range_shape, push_func_open_notify,
};
use crate::helpers::quest_types::{
    ActionCallbacks, DeferredBtVar, QuestContext, json_bool, json_float, json_i64, json_int,
    json_str, parse_int_list, parse_str_list, resolve_var_int_for_owner, resolve_var_name,
    scoped_var_key, scoped_var_key_with_owner,
};
use crate::helpers::reward::{self, RewardItem};
use crate::helpers::{gameplay_tags, gravity, map as map_helper, morale, prologue_quest, spline_state, trial_roles};
use crate::scene::{DynamicInteractKind, Scene, StateTagUpdate, now_millis};
use crate::sink::PushSink;

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub fn execute_action_list<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &mut ActionCallbacks<'_, S>,
    quest_id: i32,
    node_id: i32,
    actions: &[Value],
) -> io::Result<()> {
    for act in actions {
        execute_action(ctx, callbacks, quest_id, node_id, act)?;
    }
    Ok(())
}

pub fn execute_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &mut ActionCallbacks<'_, S>,
    quest_id: i32,
    node_id: i32,
    act: &Value,
) -> io::Result<()> {
    let Some(act_obj) = act.as_object() else {
        return Ok(());
    };
    let Some(name) = json_str(act_obj.get("Name")) else {
        return Ok(());
    };
    let empty_map = Map::new();
    let params = act_obj
        .get("Params")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty_map);

    match name {
        "AwakeEntity" => apply_awake_entity(ctx, params)?,
        "DestroyEntity" => apply_destroy_entity(ctx, params)?,
        "DestroySelf" => apply_destroy_self(ctx)?,
        "ChangeEntityState" => apply_change_entity_state(ctx, callbacks, params)?,
        "ChangeSelfEntityState" => apply_change_self_entity_state(ctx, callbacks, params)?,
        "SetEntityVisible" => apply_set_entity_visible(ctx, params)?,
        "HideSpecificEntities" => apply_entity_visibility_list(ctx, params.get("EntityIds"), false)?,
        "ShowSpecificEntities" => apply_entity_visibility_list(ctx, params.get("EntityIds"), true)?,
        "HideGroup" => apply_hide_group(ctx, params)?,
        "ShowHidedGroup" => apply_show_hided_group(ctx, params)?,
        "HideTargetRange" => apply_hide_target_range(ctx, params)?,
        "ShowTargetRange" => apply_show_target_range(ctx, params)?,
        "HideByRangeInFlow" => apply_hide_by_range_in_flow(ctx, params)?,
        "ShowAllHidedGroupInFlow" => restore_all_hidden_groups(ctx)?,
        "LockEntity" => apply_lock_entities(ctx, params.get("EntityIds"), true),
        "UnlockEntity" => apply_lock_entities(ctx, params.get("EntityIds"), false),
        "SwitchDataLayers" => {
            let load_layers = parse_int_list(params.get("LoadDataLayers"));
            let unload_layers = parse_int_list(params.get("UnloadDataLayers"));
            apply_data_layer_change(ctx, &load_layers, &unload_layers)?;
        }
        "SwitchSubLevels" => {
            let switch_ty = json_str(params.get("Type")).unwrap_or("Directly");
            if switch_ty != "Preload" {
                let loads = parse_str_list(params.get("LoadLevels"));
                let unloads = parse_str_list(params.get("UnloadLevels"));
                let hides = parse_str_list(params.get("HideLevels"));
                let tp_eid = json_int(params.get("TeleportEntityId")).unwrap_or(0);
                apply_sub_level_change(ctx, &loads, &unloads, &hides, tp_eid)?;
            }
        }
        "AddTrialCharacter" => apply_add_trial_character(ctx, params)?,
        "RemoveTrialCharacter" => apply_remove_trial_character(ctx, params)?,
        "AddBuffToPlayer" | "AddBuffToTriggeredEntity" => {
            apply_quest_buff_action(ctx, params, true, false);
        }
        "RemoveBuffFromPlayer"
        | "RemoveBuffFromTriggeredEntity"
        | "RemoveBuffToTriggeredEntity" => {
            apply_quest_buff_action(ctx, params, true, true);
        }
        "AddBuffToEntity" => apply_quest_buff_action(ctx, params, false, false),
        "RemoveBuffFromEntity" => apply_quest_buff_action(ctx, params, false, true),
        "PlayFlow" => {
            if let Some(cb) = callbacks.start_flow {
                let act_async = json_bool(act_obj.get("Async"));
                cb(ctx, quest_id, node_id, params, true, act_async)?;
            }
        }
        "ChangeState" => {
            if let Some(sid) = json_int(params.get("StateId"))
                && let Some(scene) = ctx.scene.as_deref_mut()
            {
                if callbacks.defer_flow_transition {
                    scene.flow_next_state_id = sid;
                } else if let Some(cb) = callbacks.start_flow_state
                    && !scene.active_flow_list_name.is_empty()
                {
                    let qid = scene.active_flow_quest_id;
                    let nid = scene.active_flow_node_id;
                    let fln = scene.active_flow_list_name.clone();
                    let fid = scene.active_flow_id;
                    scene.clear_active_flow();
                    cb(ctx, qid, nid, &fln, fid, sid, false)?;
                }
            }
        }
        "FinishState" => {
            if let Some(cb) = callbacks.on_finish_flow_state {
                cb(ctx)?;
            }
        }
        "AccpetCurrentQuest" => {
            if let Some(cb) = callbacks.on_accept_focus_wait {
                cb(ctx)?;
            }
        }
        "TraceCurrentQuest" => {
            if let Some(cb) = callbacks.on_trace_current_quest {
                cb(ctx, quest_id)?;
            }
        }
        "SetPlayerOperationRestriction" => {
            let is_enable_all = json_str(params.get("Type")) == Some("EnableAll");
            let encoded_params = if is_enable_all {
                Vec::new()
            } else {
                serde_json::to_vec(&Value::Object(params.clone())).unwrap_or_default()
            };
            ctx.sink.push(
                OperationRestrictionNotify::MSG_ID,
                &OperationRestrictionNotify {
                    IsNull: is_enable_all,
                    Params: encoded_params,
                },
            );
        }
        "DestroyQuest" => {
            if let Some(target_qid) = json_int(params.get("QuestId"))
                && let Some(cb) = callbacks.on_destroy_quest
            {
                cb(ctx, target_qid)?;
            }
        }
        "GetItem" => grant_action_items(ctx, callbacks, params.get("Items"))?,
        "DestroyQuestItem" => destroy_quest_items(ctx, params)?,
        "UnlockTeleportTrigger" => {
            if let Some(tp_id) = json_int(params.get("TeleportId")) {
                unlock_teleporter(ctx, tp_id)?;
            }
        }
        "OpenSystemFunction" => {
            if let Some(func_id) = json_int(params.get("FunctionId"))
                && ctx.quest_comp.info.unlock_system(func_id)
            {
                push_func_open(ctx);
            }
        }
        "EnableSystem" => apply_enable_system_action(ctx, params),
        "EnableFunction" => apply_enable_function_action(ctx, params)?,
        "UnlockSystemItem" => apply_unlock_system_item(ctx, params),
        "EnableLevelPlay" => apply_enable_level_play(ctx, callbacks, params)?,
        "SetWeather" => {
            if let Some(weather_id) = json_int(params.get("WeatherId")) {
                apply_weather_change(ctx, callbacks, weather_id)?;
            }
        }
        "SetWeatherLockState" => {
            let lock_str = json_str(params.get("LockState")).unwrap_or("");
            ctx.quest_comp.info.env.weather_locked = lock_str == "Lock";
        }
        "SetAreaState" => {
            let Some(area_id) = json_int(params.get("AreaId")) else {
                return Ok(());
            };
            let state_val = json_int(params.get("State"))
                .unwrap_or_else(|| i32::from(json_bool(params.get("State")).unwrap_or(false)));
            apply_area_state_change(ctx, area_id, state_val != 0)?;
        }
        "SetTime" => {
            let hour = json_int(params.get("Hour")).unwrap_or(8);
            let min = json_int(params.get("Min")).unwrap_or(0);
            let show_ui = json_bool(params.get("ShowUi")).unwrap_or(false);
            apply_set_time(ctx, callbacks, hour, min, show_ui)?;
        }
        "SetTimeLockState" => {
            let lock_str = json_str(params.get("LockState")).unwrap_or("Unlock");
            let locked = lock_str == "Lock";
            apply_time_lock_state(
                ctx,
                locked,
                json_int(params.get("Hour")),
                json_int(params.get("Min")),
            )?;
        }
        "SetAreaTimeState" => apply_area_time_state(ctx, params)?,
        "ToggleMapMarkState" => {
            if let Some(mark_id) = json_int(params.get("MarkId")) {
                let action_type = json_str(params.get("Type")).unwrap_or("Show");
                toggle_map_mark(ctx, mark_id, action_type);
            }
        }
        "PhoneSystemInteract" => apply_phone_system_interact(ctx, callbacks, params)?,
        "SendNpcMail" => {
            if let Some(mail_id) = json_int(params.get("MailId")) {
                apply_send_npc_mail(ctx, callbacks, mail_id)?;
            }
        }
        "CompleteGuide" => {
            if let Some(guide_id) = json_int(params.get("GuideId"))
                && guide_id > 0
                && let Some(cb) = callbacks.complete_guide
            {
                cb(ctx, guide_id)?;
            }
        }
        "SetPlayerPos" | "ServerSetPlayerPos" => {
            let action_id = json_int(act_obj.get("ActionId")).unwrap_or(0);
            teleport_player_from_action(ctx, params, action_id, callbacks.is_flow_action);
        }
        "SetVar" => apply_set_var(ctx, callbacks, quest_id, params)?,
        "CalculateVar" => apply_calculate_var(ctx, callbacks, quest_id, params)?,
        "TeleportDungeon" => {
            if let Some(did) = json_int(params.get("DungeonId")) {
                let loc_eid = json_i64(params.get("LocationEntityId"));
                resolve_teleport_dungeon_action(ctx, did, loc_eid)?;
            }
        }
        "TeleportDungeonPos" => {
            let Some(did) = json_int(params.get("DungeonId")) else {
                return Ok(());
            };
            let pos = parse_vector_pos(params.get("TeleportPos"));
            transition_to_scene(ctx, did, pos, None)?;
        }
        "ExitDungeon" => leave_current_dungeon(ctx, 0)?,
        "SetAudioState" => apply_set_audio_state(ctx, quest_id, params),
        "AddPlayBubble" => {
            let guid = json_str(act_obj.get("ActionGuid")).unwrap_or("");
            apply_add_play_bubble(ctx, params, guid);
        }
        "ClearPlayBubble" => apply_clear_play_bubble(ctx, params),
        "AddFlowInteractOption" => apply_add_flow_interact_option(ctx, quest_id, node_id, params),
        "RemoveFlowInteractOption" => apply_remove_flow_interact_option(ctx, params),
        "SetEntityPos" => apply_set_entity_pos(ctx, params)?,
        "ResetEntityPos" => apply_reset_entity_pos(ctx, params)?,
        "ChangeEntityCamp" => apply_change_entity_camp(ctx, params)?,
        "AddGuestCharacter" => apply_guest_character(ctx, params, true),
        "RemoveGuestCharacter" => apply_guest_character(ctx, params, false),
        "UpdateTrialCharacter" => apply_update_trial_character(ctx, params)?,
        "RestoreTrialCharacter" => remove_trial_roles(ctx, &[])?,
        "FinishCondition" => {
            if let Some(cb) = callbacks.on_finish_condition {
                cb(ctx, quest_id, node_id)?;
            }
        }
        "RunActions" => {
            if let Some(Value::Array(list)) = params.get("ActionList") {
                execute_action_list(ctx, callbacks, quest_id, node_id, list)?;
            }
        }
        "RandomVar" => apply_random_var(ctx, callbacks, quest_id, params)?,
        "EnableTemporaryTeleport" => {
            if let Some(eid) = json_int(params.get("RangeEntity"))
                && let Some(pc) = ctx.progress_comp.as_deref_mut()
            {
                let en = json_bool(params.get("Enable")).unwrap_or(true);
                pc.info
                    .set_key_int(&format!("temp_tp_area:{eid}"), i64::from(en));
                pc.save(ctx.fs)?;
            }
        }
        "AcceptFishingEntrust" => {
            if let Some(eid) = json_int(params.get("EntrustId")) {
                record_progress_int(ctx, "fish:cur_entrust", i64::from(eid), true)?;
            }
        }
        "DestroyFishingBoat" => record_progress_int(ctx, "fish:boat_active", 0, false)?,
        "SheriffUnlockClue" => {
            if let Some(cid) = json_int(params.get("ClueId")) {
                record_progress_claim(ctx, &format!("sheriff_clue:{cid}"))?;
            }
        }
        "SetWuYinQuState" => {
            let wname = json_str(params.get("WuYinQuName")).unwrap_or("");
            let st = json_int(params.get("State")).unwrap_or(1);
            if !wname.is_empty() {
                record_progress_int(ctx, &format!("wuyinqu:{wname}"), i64::from(st), false)?;
            }
        }
        "BvbSkipChallenge" => {
            if let Some(cid) = json_int(params.get("ChallengeId")) {
                append_unique_int(&mut ctx.quest_comp.info.progress.completed_bvb, cid);
            }
        }
        "InfrActivityUpgradeInfrastructure" => {
            if let Some(pc) = ctx.progress_comp.as_deref_mut() {
                let lv = i64::from(json_int(params.get("Level")).unwrap_or(1));
                let cur = pc.info.get_key_int("infr:lv");
                pc.info.set_key_int("infr:lv", cur.max(lv));
                pc.save(ctx.fs)?;
            }
        }
        "HonamiStorySetFormation" => {
            if let Some(pc) = ctx.progress_comp.as_deref_mut() {
                let aid = i64::from(json_int(params.get("ActivityId")).unwrap_or(0));
                pc.info.set_key_int("honami:act_id", aid);
                pc.save(ctx.fs)?;
            }
        }
        "SetupMoraleSystem" => {
            let mpid = json_int(params.get("MoralePlayId")).unwrap_or(1);
            let is_on = json_bool(params.get("IsOn")).unwrap_or(true);
            morale::sync_from_inventory(
                Some(ctx.sink),
                ctx.fs,
                ctx.assets,
                ctx.basic_comp,
                ctx.inventory_comp,
                ctx.progress_comp.as_deref_mut(),
                Some(mpid),
                Some(is_on),
                true,
            )?;
        }
        "AddTrialFollowShooter" => {
            let sid = i64::from(json_int(params.get("Id")).unwrap_or(0));
            record_progress_int(ctx, "trial:follow_shooter", sid, false)?;
        }
        "RemoveTrialFollowShooter" => record_progress_int(ctx, "trial:follow_shooter", 0, false)?,
        "ChangeTeamPosition" => {
            if let Some(pid) = json_int(params.get("PositionId")) {
                record_progress_int(ctx, "team:pos_id", i64::from(pid), false)?;
            }
        }
        "ResetEntity" => apply_reset_entity_action(ctx, params),
        "ChangeOtherState" => apply_change_other_state(ctx, callbacks, quest_id, node_id, params)?,
        "SetRegionConfig" => {
            if let Some(rid) = json_int(params.get("RegionMpcId")) {
                record_progress_int(ctx, &format!("region_cfg:{rid}"), 1, false)?;
                record_progress_int(ctx, "region:mpc_id", i64::from(rid), false)?;
            }
        }
        "GuideTrigger" => apply_guide_trigger(ctx, callbacks, params)?,
        "FinishDungeon" => apply_finish_dungeon(ctx, params)?,
        "SettlementDungeon" => apply_settlement_dungeon(ctx)?,
        "RogueActivatePortal" => record_progress_int(ctx, "rogue:portal_active", 1, false)?,
        "RogueGotoNextFloor" => {
            record_progress_add(ctx, "rogue:floor", 1)?;
        }
        "DangoAbyssActivatePortal" => record_progress_int(ctx, "abyss:portal_active", 1, false)?,
        "DangoAbyssGotoNextFloor" => {
            record_progress_add(ctx, "abyss:floor", 1)?;
        }
        "DangoAbyssCreateRewardTreasureBox" => {
            record_progress_add(ctx, "abyss:reward_box", 1)?;
        }
        "ChangeTimer" | "ToggleTimerPauseState" => apply_timer_action(ctx, name, params)?,
        "RecordDungeonEvent" => {
            let eid = json_int(params.get("EventConfig")).unwrap_or(0);
            record_progress_claim(ctx, &format!("inst:event:{eid}"))?;
        }
        "BossPilingRandomBuff" => {
            let pool_id = i64::from(json_int(params.get("BuffPoolId")).unwrap_or(0));
            record_progress_int(ctx, "bp:last_buff_pool", pool_id, false)?;
        }
        "RoverRogueRoomFinish" => {
            let layer = record_progress_add(ctx, "rover_rogue:layer", 1)?;
            record_progress_add(ctx, "rover_rogue:room_finish", 1)?;
            ctx.sink.push(
                RoverRogueRoomInfoNotify::MSG_ID,
                &RoverRogueRoomInfoNotify {
                    CurLayer: layer as i32,
                    RoomTypeId: 1,
                    MaxLayer: 5,
                    ..Default::default()
                },
            );
        }
        "RoverRogueCreateRoomReward" => {
            let rw_ty = i64::from(json_int(params.get("RewardType")).unwrap_or(1));
            record_progress_int(ctx, "rover_rogue:room_rw_type", rw_ty, false)?;
        }
        "RoleTowerGotoNextFloor" => {
            let floor = record_progress_add(ctx, "role_tower:stage", 1)?;
            record_progress_add(ctx, "role_tower:floor", 1)?;
            record_progress_int(ctx, "role_tower:stage", floor, false)?;
        }
        "SheriffUnlockProgress" => {
            if let Some(pid) = json_int(params.get("ProgressId")) {
                record_progress_claim(ctx, &format!("sheriff_prog:{pid}"))?;
                record_progress_int(ctx, "sheriff:progress_id", i64::from(pid), false)?;
            }
        }
        "SheriffChangeCriminalState" => {
            let cid = json_int(params.get("CriminalId")).unwrap_or(0);
            let st = json_int(params.get("CriminalState")).unwrap_or(0);
            if cid > 0 {
                record_progress_int(ctx, &format!("sheriff_crim_st:{cid}"), i64::from(st), false)?;
            }
        }
        "SheriffChangeCriminalIdentity" => {
            let cid = json_int(params.get("CriminalId")).unwrap_or(0);
            let iid = json_int(params.get("IdentityId")).unwrap_or(0);
            if cid > 0 {
                record_progress_int(ctx, &format!("sheriff_crim_id:{cid}"), i64::from(iid), false)?;
            }
        }
        "SheriffUnlockEnding" => {
            if let Some(eid) = json_int(params.get("EndingId")) {
                record_progress_claim(ctx, &format!("sheriff_end:{eid}"))?;
            }
        }
        "ExecRiskHarvestEffect" => {
            if let Some(eid) = json_int(params.get("Id")) {
                record_progress_claim(ctx, &format!("rh:effect:{eid}"))?;
            }
        }
        "HonamiStoryGetItem" => {
            let iid = json_int(params.get("ItemId")).unwrap_or(0);
            record_progress_add(ctx, "honami:pickup", 1)?;
            if iid > 0 {
                record_progress_add(ctx, &format!("honami:item:{iid}"), 1)?;
            }
        }
        "HonamiStoryInteractPickUp" => {
            record_progress_add(ctx, "honami:pickup", 1)?;
            apply_destroy_self(ctx)?;
        }
        "HonamiStoryReceiveCorruptedChestReward" => {
            record_progress_add(ctx, "honami:corrupted_chest_cnt", 1)?;
            if let Some(sc) = ctx.scene.as_deref()
                && sc.active_flow_owner_entity_id != 0
                && let Some(cid) = sc.find_config_id_by_net_id(sc.active_flow_owner_entity_id)
                && cid > 0
            {
                ctx.quest_comp.info.mark_chest_opened(cid);
            }
        }
        "DrinksRollRoleRequirement" => {
            let rid = json_int(params.get("RoleId")).unwrap_or(0);
            let req_id = json_int(params.get("RequirementId")).unwrap_or(1);
            if rid > 0 {
                record_progress_int(ctx, &format!("drinks:req:{rid}"), i64::from(req_id), false)?;
            }
        }
        "ChangeFightTeam" => {
            record_progress_add(ctx, "team:fight_change", 1)?;
        }
        "ExecResurrection" => apply_exec_resurrection(ctx, params)?,
        "ChangePhantomFormation" => {
            let fid = i64::from(json_int(params.get("FormationId")).unwrap_or(0));
            record_progress_int(ctx, "phantom:formation_id", fid, false)?;
        }
        "RestorePhantomFormation" => record_progress_int(ctx, "phantom:formation_id", 0, false)?,
        "SetReviveRegion" => {
            if let Some(rid) = json_int(params.get("ReviveId")) {
                record_progress_int(ctx, "region:revive_id", i64::from(rid), false)?;
            }
        }
        "SetBattleState" => apply_set_battle_state_action(ctx, params),
        "ExecBattleAction" => apply_exec_battle_action(ctx, params)?,
        "ExecAlertSystemAction" => apply_exec_alert_system_action(ctx, params)?,
        "ModifySceneItemAttributeTag" => apply_modify_scene_tag_action(ctx, params)?,
        "SetElementalBoxToTarget" | "ChangeElementalBoxState" => {
            apply_elemental_box_action(ctx, name, params)?;
        }
        "ToggleEntityElementalAbility" => apply_toggle_elemental_ability(ctx, params)?,
        "Collect"
        | "CollectSunSpirit"
        | "GetRewardByInteract"
        | "ClaimLevelPlayReward"
        | "ClaimDynamicEntityReward" => {
            apply_interact_collect_or_reward(ctx, callbacks, name, params)?;
        }
        "SetInteractionLockState" => {
            let locked = json_bool(params.get("IsLock"))
                .or_else(|| json_bool(params.get("IsLocked")))
                .unwrap_or(true);
            if let Some(eid) = resolve_action_entity_cfg_id(ctx.scene.as_deref(), params) {
                set_entity_locked(ctx, eid, locked);
                record_progress_int(ctx, &format!("interact_lock:{eid}"), i64::from(locked), false)?;
            }
        }
        "ActivateResetPoint" | "TeleportToLatestResetPoint" => {
            apply_reset_point_action(ctx, name == "ActivateResetPoint")?;
        }
        "ResetLevelPlay" => apply_reset_level_play_action(ctx, params)?,
        "MowingTowerGotoNextFloor" => {
            record_progress_add(ctx, "mow:floor", 1)?;
        }
        "SlashAndTowerGotoNextFloor" => {
            record_progress_add(ctx, "slash:floor", 1)?;
        }
        "BossPilingEnterNextInst" => {
            record_progress_add(ctx, "bp:inst_step", 1)?;
        }
        "DangoAbyssReceiveReward" => {
            record_progress_add(ctx, "abyss:reward_cnt", 1)?;
        }
        "RogueSelectRoom" => {
            record_progress_add(ctx, "rogue:room_sel", 1)?;
        }
        "RogueReceiveReward" => {
            record_progress_add(ctx, "rogue:reward_cnt", 1)?;
        }
        "RollBlockGetReward" => {
            record_progress_add(ctx, "rb_blk:rw", 1)?;
        }
        "HonamiStoryActivateSafeEvacuation" => {
            record_progress_int(ctx, "honami:safe_leave", 1, false)?;
        }
        "RoverRogueActivate" => {
            record_progress_add(ctx, "rover_rogue:activate", 1)?;
        }
        "RecoverDurability" => {
            if let Some(eid) = resolve_action_entity_cfg_id(ctx.scene.as_deref(), params) {
                record_progress_int(ctx, &format!("durability:{eid}"), 100, false)?;
            }
        }
        "ScanEnrichmentArea" => {
            let eid = json_int(params.get("EnrichmentAreaEntityId"))
                .or_else(|| resolve_action_entity_cfg_id(ctx.scene.as_deref(), params))
                .unwrap_or(0);
            if eid > 0 {
                record_progress_int(ctx, &format!("enrichment:scan:{eid}"), 1, true)?;
            }
        }
        "SendAiEvent" => {
            if let Some(ev_ty) = json_str(params.get("EventType"))
                && let Some(eid) = resolve_action_entity_cfg_id(ctx.scene.as_deref(), params)
                && let Some(pc) = ctx.progress_comp.as_deref_mut()
            {
                pc.info.set_key_str(&format!("ai_state:{eid}"), ev_ty);
                pc.save(ctx.fs)?;
            }
        }
        "SendClientEvent" => {
            if let Some(ev_name) = json_str(params.get("EventName")) {
                record_progress_int(ctx, &format!("client_ev:{ev_name}"), 1, true)?;
            }
        }
        "TriggerDeadeyeMode" => record_progress_int(ctx, "deadeye:active", 1, false)?,
        "VehicleSetSpeedImmediately" => record_progress_int(ctx, "vehicle:speed", 25, false)?,
        "SummonEntity" => apply_summon_entity_action(ctx, params)?,
        "DestroySummonedEntities" | "DestroyDynamicEntities" => {
            apply_destroy_summoned_action(ctx, name, params)?;
        }
        "SetEntityClientVisibleSave" | "SetEntityClientVisible" => {
            let vis = json_bool(params.get("Visible")).unwrap_or(true);
            apply_entity_visibility_list(ctx, params.get("EntityIds"), vis)?;
        }
        "SetForceLock" => {
            if let Some(eid) = json_int(params.get("EntityId")) {
                let locked = json_bool(params.get("IsLocked")).unwrap_or(true);
                set_entity_locked(ctx, eid, locked);
            }
        }
        "ChangeEntityPrefabPerformance" => {
            if let Some(eid) = json_int(params.get("EntityId"))
                && let Some(perf_tag) = json_str(params.get("PerformanceTag"))
            {
                apply_entity_state_change(ctx, callbacks, eid, perf_tag)?;
            }
        }
        "StopSceneItemMove" => apply_stop_scene_item_move(ctx, params)?,
        "StopMotorCruise" => record_progress_int(ctx, "motor:cruise", 0, false)?,
        "SetTeleControl" => {
            if let Some(cfg) = params.get("Config").and_then(|v| v.as_object()) {
                let ty = json_str(cfg.get("Type")).unwrap_or("");
                if ty == "SetResetPosition" {
                    if let Some(eid) = json_int(cfg.get("EntityId"))
                        && let Some(pos_eid) = json_int(cfg.get("PositionEntityId"))
                    {
                        record_progress_int(
                            ctx,
                            &format!("tele_reset:{eid}"),
                            i64::from(pos_eid),
                            false,
                        )?;
                    }
                } else if let Some(eid) = json_int(cfg.get("EntityId")) {
                    record_progress_int(ctx, &format!("tele_control:{eid}"), 1, false)?;
                }
            }
        }
        "ResetPlayerPosWithPresentation" => apply_reset_player_pos_presentation(ctx, params)?,
        "GiantToggleModel" => {
            let en = json_bool(params.get("IsEnabled")).unwrap_or(true);
            record_progress_int(ctx, "giant:model_enabled", i64::from(en), false)?;
        }
        "PerformSunSpiritGearRoundTrip" => {
            if let Some(gid) = json_int(params.get("SunSpiritGearId")) {
                record_progress_claim(ctx, &format!("sun_spirit_gear:{gid}"))?;
            }
        }
        "TriggerSystemRandomPlot" => {
            if let Some(pid) = json_int(params.get("Id")) {
                record_progress_claim(ctx, &format!("random_plot:{pid}"))?;
            }
        }
        "CreatePrefab" | "DestroyPrefab" => {
            apply_prefab_action(ctx, name, params)?;
        }
        _ if is_tracked_or_client_action(name) => return Ok(()),
        "ClearEntityVisibleTag" => apply_entity_visibility_list(ctx, params.get("EntityIds"), true)?,
        "MoveSceneItem"
        | "MoveWithSpline"
        | "NewMoveWithSpline"
        | "ChangeLiftTarget"
        | "NpcMoveToPoint"
        | "PlayerMoveToPoint"
        | "CharacterMoveToPoint" => apply_server_move_action(ctx, name, params)?,
        "VehicleEnter"
        | "VehicleExitPlayer"
        | "VehicleExitNpc"
        | "VehicleTeleport"
        | "TeleportToAndEnterVehicle"
        | "VehicleMoveWithPathLine"
        | "StopNewMoveWithSpline" => apply_vehicle_action(ctx, name, params)?,
        "EnableHostility" | "EnableAI" | "OverrideAiBehaviorTree" => {
            apply_ai_control_action(ctx, name, params)?;
        }
        "ManualOccupations" | "ForceOccupations" => {
            apply_occupation_action(ctx, quest_id, node_id, name == "ForceOccupations", params)?;
        }
        "SetNpcGroupPerform" => apply_set_npc_group_perform(ctx, params)?,
        "RemoveNpcGroupPerform" => {
            if let Some(key) = json_str(params.get("Key")) {
                record_progress_int(ctx, &format!("npc:group_perform:{key}"), 0, false)?;
            }
            record_progress_int(ctx, "npc:supported_walking", 0, false)?;
        }
        "PlayBubble" => {
            let guid = json_str(act_obj.get("ActionGuid")).unwrap_or("");
            apply_add_play_bubble(ctx, params, guid);
        }
        "ChangeNpcPerformState" => {
            let eid = json_int(params.get("EntityId")).unwrap_or(0);
            let st = json_str(params.get("State")).unwrap_or("Default");
            if eid > 0 {
                record_progress_claim(ctx, &format!("npc_perform:{eid}:{st}"))?;
            }
        }
        "ToggleDragActorPlay" => {
            let enabled = params
                .get("Config")
                .and_then(Value::as_object)
                .and_then(|c| json_str(c.get("Type")))
                .is_none_or(|t| t != "Disable");
            record_progress_int(ctx, "drag_actor:in_play", i64::from(enabled), false)?;
        }
        "EnableSplineMoveModel" => {
            if let Some(cfg) = params.get("Config").and_then(Value::as_object) {
                let sid = json_int(cfg.get("SplineEntityId")).unwrap_or(0);
                let on = json_str(cfg.get("Type")).is_none_or(|t| t != "Close");
                record_progress_int(ctx, &format!("spline_move:{sid}"), i64::from(on), false)?;
            }
        }
        "EnableNearbyTracking" => apply_enable_nearby_tracking(ctx, quest_id, params),
        "FocusOnMapMark" => {
            if let Some(mt) = params.get("MapMarkType").and_then(Value::as_object) {
                let qid = json_int(mt.get("QuestId")).unwrap_or(0);
                if qid > 0 {
                    record_progress_int(ctx, "map:focus_quest", i64::from(qid), false)?;
                }
            }
        }
        "LeisureInteract" | "NpcLeisureInteract" => {
            let eid = json_int(params.get("SceneEntity"))
                .or_else(|| json_int(params.get("EntityId")))
                .or_else(|| {
                    params
                        .get("Option")
                        .and_then(Value::as_object)
                        .and_then(|o| json_int(o.get("EntityId")))
                })
                .unwrap_or(0);
            if eid > 0 {
                record_progress_int(ctx, &format!("leisure:entity:{eid}"), 1, false)?;
                if let Some(scene) = ctx.scene.as_deref_mut() {
                    wake_entity_fsm(scene, eid, wake_reason::EVENT);
                }
            }
        }
        "RestoreAiBehaviorTree" => {
            let eids = parse_int_list(params.get("EntityIds"));
            for cfg_id in eids {
                if cfg_id <= 0 {
                    continue;
                }
                record_progress_int(ctx, &format!("ai_bt_cur:{cfg_id}"), 0, false)?;
                if let Some(scene) = ctx.scene.as_deref_mut() {
                    wake_entity_fsm(scene, cfg_id, wake_reason::INITIAL | wake_reason::BLACKBOARD);
                }
            }
        }
        "StartRoadNetworkNavigation" => {
            let tid = params
                .get("Target")
                .and_then(Value::as_object)
                .and_then(|t| json_int(t.get("TargetId")))
                .unwrap_or(0);
            record_progress_int(ctx, &format!("road_nav:{tid}"), 1, false)?;
        }
        "GiantTriggerResponse" => {
            record_progress_claim(ctx, "giant_trigger:active")?;
        }
        "OpenSystemBoard" | "OpenSystemBoardWithReturn" | "OpenSimpleGameplay" => {
            if let Some(st) = params.get("SystemType").and_then(Value::as_object) {
                if let Some(bid) = json_int(st.get("Id")) {
                    record_progress_int(ctx, &format!("system_board:{bid}"), 1, false)?;
                }
                if let Some(rv) = st.get("ReturnVar").and_then(Value::as_object)
                    && let Some(vname) = json_str(rv.get("Name"))
                {
                    ctx.quest_comp.info.set_var_int(vname, 1);
                    ctx.quest_comp.save(ctx.fs)?;
                }
            } else if let Some(bid) = json_int(params.get("BoardId")) {
                record_progress_int(ctx, &format!("system_board:{bid}"), 1, false)?;
            } else {
                let st = params
                    .get("GameplayConfig")
                    .and_then(Value::as_object)
                    .and_then(|g| json_str(g.get("Type")))
                    .or_else(|| json_str(params.get("SystemType")))
                    .unwrap_or("default");
                record_progress_claim(ctx, &format!("simple_gameplay:{st}"))?;
            }
        }
        "PlayDynamicSettlement" => {
            let dtype = params
                .get("DynamicSettlementConfig")
                .and_then(Value::as_object)
                .and_then(|d| json_str(d.get("Type")))
                .unwrap_or("Battle");
            record_progress_claim(ctx, &format!("dynamic_settlement:{dtype}"))?;
        }
        "DriveMotorInSpecConfig" => {
            record_progress_int(ctx, "motor:spec_cfg", 1, false)?;
        }
        "ExecClientBattleAction" => {
            if let Some(opt) = params.get("ClientBattleOption").and_then(Value::as_object) {
                if let Some(msid) = json_int(opt.get("MotorSkillId"))
                    && msid > 0
                {
                    record_progress_claim(ctx, &format!("skill:used:{msid}"))?;
                }
                let atype = json_str(opt.get("Type")).unwrap_or("default");
                record_progress_claim(ctx, &format!("client_battle_action:{atype}"))?;
            }
        }
        "SlideRailEnd" => {
            record_progress_int(ctx, "slide_rail:active", 0, false)?;
        }
        "TakePlotPhoto" => {
            record_progress_claim(ctx, "photo:submitted")?;
        }
        "WaitBattleCondition" => {
            let eid = params
                .get("StateOption")
                .and_then(Value::as_object)
                .and_then(|o| json_int(o.get("EntityId")))
                .unwrap_or(0);
            if eid > 0 {
                record_progress_int(ctx, &format!("battle_cond:{eid}"), 1, false)?;
            }
        }
        "WaitUntilLevelSequenceReachMark" => {
            let eid = json_int(params.get("EntityId")).unwrap_or(0);
            let mark = json_str(params.get("Mark")).unwrap_or("default");
            record_progress_claim(ctx, &format!("lvl_seq_mark:{eid}:{mark}"))?;
        }
        "PromptQuestChapterUI" => {
            let ch_state = json_int(params.get("ChapterState")).unwrap_or(1);
            record_progress_int(ctx, "quest:chapter_ui", i64::from(ch_state), false)?;
        }
        _ => {
            tracing::debug!("unhandled quest action: {name}");
        }
    }
    Ok(())
}

fn apply_unlock_system_item<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) {
    let Some(opt_val) = params.get("SystemOption").and_then(|v| v.as_object()) else {
        return;
    };
    let sys_type = json_str(opt_val.get("Type")).unwrap_or("");
    let uopt_obj = opt_val.get("UnlockOption").and_then(|v| v.as_object());
    let item_id = uopt_obj
        .and_then(|u| json_int(u.get("Id")))
        .or_else(|| json_int(opt_val.get("Id")));
    let Some(id) = item_id else {
        return;
    };
    if id <= 0 {
        return;
    }
    ctx.quest_comp.info.unlock_system(id);
    push_func_open(ctx);
    if sys_type == "AtlasSystem" {
        let utype = uopt_obj
            .and_then(|u| json_str(u.get("Type")))
            .unwrap_or("");
        let ill_type = match utype {
            "PlotPhoto" => Some(IllustratedType::Photograph),
            "Noun" => Some(IllustratedType::Noun),
            _ => None,
        };
        if let Some(it) = ill_type {
            ctx.sink.push(
                IllustratedUnlockNotify::MSG_ID,
                &IllustratedUnlockNotify {
                    IsNew: true,
                    Type: Some(it),
                    Entry: Some(IllustratedEntry {
                        Id: id,
                        Num: 1,
                        IsRead: false,
                        ..Default::default()
                    }),
                },
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn execute_flow_state_action_by_id<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &mut ActionCallbacks<'_, S>,
    flow_name: &str,
    flow_id: i32,
    state_id: i32,
    quest_id: i32,
    node_id: i32,
    action_id: i32,
) -> io::Result<()> {
    let Some(state_val) = ctx
        .assets
        .flow_states
        .get_flow_state(flow_name, flow_id, state_id)?
    else {
        return Ok(());
    };
    let Some(action_val) =
        sonoro_gamesv_data::lazy_flow_store::find_action_by_id(&state_val, action_id)
    else {
        return Ok(());
    };
    execute_action(ctx, callbacks, quest_id, node_id, action_val)
}

pub fn push_func_open<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    push_func_open_notify(
        ctx.sink,
        ctx.assets,
        ctx.basic_comp,
        ctx.echo_comp,
        ctx.inventory_comp,
        ctx.map_comp,
        ctx.quest_comp,
        ctx.role_comp.as_deref(),
    );
    if let (Some(notify_fn), Some(prog)) = (ctx.activity_notify, ctx.progress_comp.as_deref()) {
        let player_id = ctx.basic_comp.player_id;
        notify_fn(
            ctx.sink,
            ctx.assets,
            prog,
            ctx.basic_comp,
            ctx.echo_comp,
            ctx.inventory_comp,
            ctx.map_comp,
            ctx.quest_comp,
            ctx.role_comp.as_deref(),
            player_id,
        );
    }
}

pub fn unlock_teleporter<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    tp_id: i32,
) -> io::Result<()> {
    if ctx.map_comp.info.unlock_teleporter(tp_id) {
        if let Some(tp) = ctx.assets.tables.teleporter.get_data_by_id(&tp_id) {
            if tp.Type == 1 && tp.FogId > 0 {
                let _ = map_helper::unlock_fog_and_siblings(
                    ctx.sink,
                    ctx.assets,
                    ctx.map_comp,
                    tp.FogId,
                );
            }
            if let Ok(cfg_id32) = i32::try_from(tp.TeleportEntityConfigId)
                && cfg_id32 > 0
            {
                ctx.quest_comp
                    .info
                    .set_entity_state(tp.TeleportEntityConfigId, "关卡.Common.状态.激活");
                ctx.quest_comp.save(ctx.fs)?;
                if let Some(scene) = ctx.scene.as_deref_mut()
                    && let Some(nid) = scene.find_entity_net_id_by_config_id(cfg_id32)
                    && let Some(upd) = scene.update_entity_state_tag(
                        &ctx.assets.tables.gameplay_tag_remap,
                        nid,
                        "关卡.Common.状态.激活",
                    )
                {
                    emit_entity_state_tags(ctx, nid, upd);
                }
            }
        }
        ctx.map_comp.save(ctx.fs)?;
        ctx.sink.push(
            TeleportUpdateNotify::MSG_ID,
            &TeleportUpdateNotify { Ids: vec![tp_id] },
        );
    }
    Ok(())
}

pub fn apply_data_layer_change<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    load_layers: &[i32],
    unload_layers: &[i32],
) -> io::Result<()> {
    if load_layers.is_empty() && unload_layers.is_empty() {
        return Ok(());
    }

    let mut notify = SceneChangeDataLayerNotify::default();
    for &dl in load_layers {
        if dl <= 0 {
            continue;
        }
        append_unique_int(&mut ctx.quest_comp.info.world.loaded_datalayers, dl);
        remove_int_value(&mut ctx.quest_comp.info.world.unloaded_datalayers, dl);
        notify.LoadDataLayers.push(dl);
    }
    for &dl in unload_layers {
        if dl <= 0 {
            continue;
        }
        remove_int_value(&mut ctx.quest_comp.info.world.loaded_datalayers, dl);
        append_unique_int(&mut ctx.quest_comp.info.world.unloaded_datalayers, dl);
        notify.UnloadDataLayers.push(dl);
    }
    ctx.sink.push(SceneChangeDataLayerNotify::MSG_ID, &notify);
    ctx.quest_comp.save(ctx.fs)?;

    if let Some(scene) = ctx.scene.as_deref_mut() {
        let mut remove_ids = Vec::new();
        for storage in &scene.entities {
            if matches!(
                storage.config.entity_type,
                EntityType::Player | EntityType::PlayerEntity
            ) || storage.config.config_id <= 0
            {
                continue;
            }
            let Some(entity_cfg) = ctx
                .assets
                .find_level_entity(scene.instance_id, i64::from(storage.config.config_id))
            else {
                continue;
            };
            if !entity_data_layers_active(
                ctx.assets,
                &ctx.quest_comp.info,
                scene.instance_id,
                &entity_cfg,
            ) {
                remove_ids.push(storage.net_id);
            }
        }
        if !remove_ids.is_empty() {
            let mut remove_infos = Vec::with_capacity(remove_ids.len());
            for net_id in remove_ids {
                remove_infos.push(EntityRemoveInfo {
                    EntityId: net_id,
                    Type: Some(ERemoveEntityType::RemoveTypeNormal),
                });
                let _ = scene.remove(ctx.fs, net_id);
            }
            ctx.sink.push(
                EntityRemoveNotify::MSG_ID,
                &EntityRemoveNotify {
                    RemoveInfos: remove_infos,
                    IsRemove: true,
                },
            );
        }
        if !scene.instance.players.is_empty() {
            ctx.events.enqueue(Event::SpatialSpawnsCheck {
                player_id: scene.player_id,
            });
        }
    }
    Ok(())
}

pub fn entity_data_layers_active(
    assets: &Assets,
    quest_info: &QuestInfo,
    instance_id: i32,
    entity_cfg: &LevelEntityConfig,
) -> bool {
    let Some(base) = &entity_cfg.ComponentsData.BaseInfoComponent else {
        return true;
    };
    let Some(layers) = &base.DataLayers else {
        return true;
    };
    if layers.is_empty() {
        return true;
    }

    let map_id = assets.resolve_map_config_id(instance_id);
    for &layer_id in layers {
        if layer_id <= 0 || quest_info.world.unloaded_datalayers.contains(&layer_id) {
            return false;
        }
        let Some(layer_cfg) = assets.tables.data_layer_config.get_data_by_id(&layer_id) else {
            return false;
        };
        if layer_cfg.DataLayer.is_empty() {
            return false;
        }
        if layer_cfg.LevelId != 0
            && layer_cfg.LevelId != instance_id
            && layer_cfg.LevelId != map_id
        {
            return false;
        }
        if !layer_cfg.InitLoad && !quest_info.world.loaded_datalayers.contains(&layer_id) {
            return false;
        }
    }
    true
}

pub fn sync_persisted_vehicle_npc_links<S: PushSink + ?Sized>(
    scene: &mut Scene,
    progress_comp: &PlayerProgressComponent,
    sink: &mut S,
    only_config_id: Option<i32>,
) {
    for entry in &progress_comp.info.dynamic.ints {
        let Some(rest) = entry.key.strip_prefix("vehicle:npc:") else {
            continue;
        };
        let Ok(npc_config_id) = rest.parse::<i32>() else {
            continue;
        };
        let Ok(vehicle_config_id) = i32::try_from(entry.val) else {
            continue;
        };
        if npc_config_id <= 0 || vehicle_config_id <= 0 {
            continue;
        }
        if let Some(filter) = only_config_id
            && filter != npc_config_id
            && filter != vehicle_config_id
        {
            continue;
        }
        let Some(npc_net_id) = scene.find_entity_net_id_by_config_id(npc_config_id) else {
            continue;
        };
        let Some(vehicle_net_id) = scene.find_entity_net_id_by_config_id(vehicle_config_id) else {
            continue;
        };
        let seat = i32::try_from(
            progress_comp
                .info
                .get_key_int(&format!("vehicle:npc_seat:{npc_config_id}")),
        )
        .unwrap_or(0);
        if let Some(v_storage) = scene.entity_mut(vehicle_net_id)
            && let Some(veh) = v_storage.vehicle.as_mut()
        {
            veh.occupied = true;
        } else {
            continue;
        }
        sink.push(
            VehicleUpdateEntityNotify::MSG_ID,
            &VehicleUpdateEntityNotify {
                EntityId: npc_net_id,
                VehicleCreatureId: vehicle_net_id,
                Seat: seat,
                IsEntering: true,
                ..Default::default()
            },
        );
    }
}

pub fn update_player_scene_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    location: [f32; 3],
    rotation_opt: Option<[f32; 3]>,
) -> io::Result<()> {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(());
    };
    if !location.iter().all(|v| v.is_finite()) {
        return Ok(());
    }
    if let Some(rot) = rotation_opt
        && !rot.iter().all(|v| v.is_finite())
    {
        return Ok(());
    }
    let mut rotation = [0.0f32; 3];
    let mut found_player = false;
    let scene_player_id = scene.player_id;
    for player in &mut scene.instance.players {
        if player.id != scene_player_id {
            continue;
        }
        found_player = true;
        rotation = player.rotation;
        player.location = location;
        if let Some(new_rot) = rotation_opt {
            rotation = new_rot;
            player.rotation = new_rot;
        }
        break;
    }
    if !found_player {
        return Ok(());
    }
    scene.last_spatial_check_loc = location;

    let direct_ids: Vec<i64> = scene
        .entities
        .iter()
        .filter(|s| s.direct_control)
        .map(|s| s.net_id)
        .collect();
    for nid in direct_ids {
        let _ = scene.set_entity_position(nid, location, Some(rotation[2]));
        if let Some(storage) = scene.entity_mut(nid)
            && let Some(pos) = storage.position.as_mut()
        {
            pos.rotation = rotation;
        }
        scene.save_entity_by_net_id(ctx.fs, nid)?;
    }
    scene.save_instance(ctx.fs)?;
    ctx.events.enqueue(Event::SpatialSpawnsCheck {
        player_id: scene_player_id,
    });
    Ok(())
}

pub fn apply_sub_level_change<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    load_levels: &[String],
    unload_levels: &[String],
    hide_levels: &[String],
    teleport_entity_id: i32,
) -> io::Result<()> {
    if load_levels.is_empty()
        && unload_levels.is_empty()
        && hide_levels.is_empty()
        && teleport_entity_id == 0
    {
        return Ok(());
    }

    let mut notify = SceneSubLevelsChangedNotify {
        TeleportEntityId: teleport_entity_id,
        ..Default::default()
    };
    for lvl in load_levels {
        append_unique_str(&mut ctx.quest_comp.info.world.loaded_sublevels, lvl);
        remove_str_value(&mut ctx.quest_comp.info.world.hidden_sublevels, lvl);
        notify.NewLoaded.push(lvl.as_bytes().to_vec());
    }
    for lvl in unload_levels {
        remove_str_value(&mut ctx.quest_comp.info.world.loaded_sublevels, lvl);
        notify.NewUnloaded.push(lvl.as_bytes().to_vec());
    }
    for lvl in hide_levels {
        append_unique_str(&mut ctx.quest_comp.info.world.hidden_sublevels, lvl);
        notify.HideSubLevels.push(lvl.as_bytes().to_vec());
    }

    if teleport_entity_id > 0
        && let Some(inst_id) = ctx.scene.as_deref().map(|s| s.instance_id)
        && let Some(cfg) = ctx
            .assets
            .find_level_entity(inst_id, i64::from(teleport_entity_id))
        && let Some(pos) = asset_indexes::entity_world_pos(&cfg)
    {
        let rot = asset_indexes::entity_world_rot(&cfg);
        update_player_scene_state(ctx, pos, Some(rot))?;
    }
    ctx.sink.push(SceneSubLevelsChangedNotify::MSG_ID, &notify);
    ctx.quest_comp.save(ctx.fs)
}

pub fn apply_weather_change<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    weather_id: i32,
) -> io::Result<()> {
    ctx.quest_comp.info.env.weather_id = weather_id;
    ctx.sink.push(
        WeatherNotify::MSG_ID,
        &WeatherNotify {
            WeatherId: weather_id,
            IsClient: false,
        },
    );
    if weather_id > 0 {
        let _ = js_patches::weather::push(ctx.sink, weather_id);
    }
    if let Some(cb) = callbacks.on_weather_changed {
        cb(ctx, weather_id)?;
    }
    Ok(())
}

pub fn apply_area_state_change<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    area_id: i32,
    state: bool,
) -> io::Result<()> {
    if area_id <= 0 {
        return Ok(());
    }
    ctx.quest_comp.info.set_area_state(area_id, state);
    ctx.sink.push(
        SceneAreaStateNotify::MSG_ID,
        &SceneAreaStateNotify {
            AreaState: Some(SceneAreaState {
                AreaId: area_id,
                State: state,
            }),
        },
    );
    ctx.quest_comp.save(ctx.fs)
}

pub fn apply_set_time<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    hour: i32,
    minute: i32,
    show_ui: bool,
) -> io::Result<()> {
    let norm_hour = hour.rem_euclid(24);
    let norm_min = minute.rem_euclid(60);
    let already_same = ctx.quest_comp.info.env.time_lock_hour == norm_hour
        && ctx.quest_comp.info.env.time_lock_minute == norm_min;
    if already_same && !show_ui {
        return Ok(());
    }

    ctx.quest_comp.info.env.time_lock_hour = norm_hour;
    ctx.quest_comp.info.env.time_lock_minute = norm_min;

    let now_ms = now_millis();
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.instance.map_time.hour = norm_hour;
        scene.instance.map_time.minute = norm_min;
        scene.instance.map_time.owner_clock_time_span = now_ms;
    }
    ctx.sink.push(
        SyncSceneTimeNotify::MSG_ID,
        &SyncSceneTimeNotify {
            TimeInfo: Some(SceneTimeInfo {
                Hour: norm_hour,
                Minute: norm_min,
                OwnerTimeClockTimeSpan: now_ms,
            }),
            ShowUi: show_ui,
        },
    );
    if let Some(cb) = callbacks.on_time_changed {
        cb(ctx, norm_hour, norm_min)?;
    }
    Ok(())
}

pub fn apply_time_lock_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    locked: bool,
    hour_opt: Option<i32>,
    min_opt: Option<i32>,
) -> io::Result<()> {
    ctx.quest_comp.info.env.time_locked = locked;
    if let Some(h) = hour_opt {
        ctx.quest_comp.info.env.time_lock_hour = h.rem_euclid(24);
    }
    if let Some(m) = min_opt {
        ctx.quest_comp.info.env.time_lock_minute = m.rem_euclid(60);
    }
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.instance.map_time.hour = ctx.quest_comp.info.env.time_lock_hour;
        scene.instance.map_time.minute = ctx.quest_comp.info.env.time_lock_minute;
        scene.instance.map_time.owner_clock_time_span = now_millis();
        scene.save_instance(ctx.fs)?;
    }
    ctx.sink.push(
        TimeLockInfoNotify::MSG_ID,
        &TimeLockInfoNotify {
            IsLock: locked,
            Hour: i64::from(ctx.quest_comp.info.env.time_lock_hour),
            Minute: i64::from(ctx.quest_comp.info.env.time_lock_minute),
        },
    );
    Ok(())
}

pub fn apply_trial_roles<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    role_ids: &[i32],
    enable_map_and_tp: bool,
) -> io::Result<()> {
    apply_trial_roles_with_options(ctx, role_ids, enable_map_and_tp, false, false, &[])
}

pub fn apply_instance_trial_roles<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    inst_id: i32,
) -> io::Result<()> {
    if inst_id <= 0 || inst_id == 8 || !ctx.quest_comp.info.env.trial_roles.is_empty() {
        return Ok(());
    }

    if let Some(trial_cfg) = ctx
        .assets
        .tables
        .instance_trial_role_config
        .get_data_by_id(&inst_id)
    {
        let is_male = ctx.basic_comp.info.attributes.sex == 1;
        let form = if is_male {
            &trial_cfg.MaleFormation
        } else {
            &trial_cfg.FemaleFormation
        };
        if !form.is_empty() {
            return apply_trial_roles_with_options(
                ctx,
                form,
                true,
                true,
                trial_cfg.AutoChange,
                &[inst_id],
            );
        }
    }

    if let Some(rt) = ctx
        .assets
        .tables
        .role_trial_info
        .items
        .iter()
        .find(|r| r.InstanceId == inst_id && r.TrialRoleId > 0)
    {
        return apply_trial_roles_with_options(ctx, &[rt.TrialRoleId], true, true, true, &[inst_id]);
    }

    if let Some(inst_cfg) = ctx.assets.tables.instance_dungeon.get_data_by_id(&inst_id) {
        let ids: Vec<i32> = inst_cfg
            .TrialRoleInfo
            .iter()
            .copied()
            .filter(|&id| id > 0)
            .collect();
        if !ids.is_empty() {
            return apply_trial_roles_with_options(ctx, &ids, true, true, true, &[inst_id]);
        }
    }
    Ok(())
}

fn apply_trial_roles_with_options<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    raw_role_ids: &[i32],
    enable_map_and_tp: bool,
    create_temp_team: bool,
    auto_change: bool,
    dungeon_ids: &[i32],
) -> io::Result<()> {
    let world_level = ctx.basic_comp.info.attributes.cur_world_level.max(1);
    let gender = ctx.basic_comp.info.attributes.sex;

    let mut resolved_ids = Vec::new();
    for &raw_id in raw_role_ids {
        if raw_id <= 0 {
            continue;
        }
        let resolved_id = if ctx.quest_comp.info.env.trial_roles.contains(&raw_id)
            && ctx
                .assets
                .tables
                .trial_role_info
                .get_data_by_id(&raw_id)
                .is_some()
        {
            raw_id
        } else {
            ctx.assets
                .tables
                .resolve_trial_role_id(raw_id, world_level, gender)
                .unwrap_or(raw_id)
        };
        if !resolved_ids.contains(&resolved_id) {
            resolved_ids.push(resolved_id);
        }
    }
    if resolved_ids.is_empty() {
        return Ok(());
    }

    let mut env_dirty = false;
    let mut newly_materialized = Vec::new();
    for &tid in &resolved_ids {
        if append_unique_int(&mut ctx.quest_comp.info.env.trial_roles, tid) {
            env_dirty = true;
        }
        if let Some(role_comp) = ctx.role_comp.as_deref_mut()
            && trial_roles::materialize_trial_role(
                ctx.assets,
                role_comp,
                ctx.weapon_comp,
                ctx.echo_comp,
                tid,
            )
        {
            newly_materialized.push(tid);
        }
    }

    if let Some(role_comp) = ctx.role_comp.as_deref_mut()
        && !newly_materialized.is_empty()
    {
        trial_roles::push_trial_role_list(
            ctx.sink,
            ctx.assets,
            role_comp,
            ctx.weapon_comp,
            ctx.echo_comp,
            &newly_materialized,
        )
        .map_err(io::Error::other)?;
    }

    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.set_trial_roles(&ctx.quest_comp.info.env.trial_roles);
    }

    let notify = RoleTrialOpenNotify {
        ShowTips: !newly_materialized.is_empty(),
        EnableMapAndTeleport: enable_map_and_tp,
        RoleIds: resolved_ids.clone(),
        TrialRoleItems: resolved_ids
            .iter()
            .map(|&tid| TrialRoleItem {
                RoleId: tid,
                Show: true,
            })
            .collect(),
        CanEnterDungeonList: dungeon_ids.iter().copied().filter(|&d| d > 0).collect(),
    };
    if !notify.RoleIds.is_empty() {
        ctx.sink.push(RoleTrialOpenNotify::MSG_ID, &notify);
    }

    if let (Some(scene), Some(role_comp)) = (ctx.scene.as_deref_mut(), ctx.role_comp.as_deref_mut())
    {
        let had_saved = !ctx.quest_comp.info.env.pre_trial_roles.is_empty();
        trial_roles::save_pre_trial_formation(ctx.quest_comp, scene);
        if !had_saved && !ctx.quest_comp.info.env.pre_trial_roles.is_empty() {
            env_dirty = true;
        }
        trial_roles::apply_trial_roles_to_scene_formation(
            scene,
            ctx.fs,
            ctx.assets,
            ctx.sink,
            ctx.events,
            role_comp,
            ctx.weapon_comp,
            ctx.echo_comp,
            &resolved_ids,
            create_temp_team,
            auto_change,
        )
        .map_err(io::Error::other)?;
    }

    if env_dirty {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn remove_trial_roles<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    role_ids: &[i32],
) -> io::Result<()> {
    let mut removed_ids = Vec::new();
    let mut notify = RoleTrialCloseNotify {
        EnableMapAndTeleport: true,
        ..Default::default()
    };

    if role_ids.is_empty() {
        for &rid in &ctx.quest_comp.info.env.trial_roles {
            if !removed_ids.contains(&rid) {
                removed_ids.push(rid);
                notify.RoleIds.push(rid);
            }
        }
        ctx.quest_comp.info.env.trial_roles.clear();
    } else {
        let world_level = ctx.basic_comp.info.attributes.cur_world_level.max(1);
        let gender = ctx.basic_comp.info.attributes.sex;
        for &raw_id in role_ids {
            if raw_id <= 0 {
                continue;
            }
            let mut matched_any = false;
            if remove_int_value(&mut ctx.quest_comp.info.env.trial_roles, raw_id) {
                matched_any = true;
                if !removed_ids.contains(&raw_id) {
                    removed_ids.push(raw_id);
                    notify.RoleIds.push(raw_id);
                }
            }
            if let Some(resolved_id) = ctx
                .assets
                .tables
                .resolve_trial_role_id(raw_id, world_level, gender)
                && remove_int_value(&mut ctx.quest_comp.info.env.trial_roles, resolved_id)
            {
                matched_any = true;
                if !removed_ids.contains(&resolved_id) {
                    removed_ids.push(resolved_id);
                    notify.RoleIds.push(resolved_id);
                }
            }
            for entry in &ctx.assets.tables.trial_role_info.items {
                if entry.GroupId != raw_id {
                    continue;
                }
                if remove_int_value(&mut ctx.quest_comp.info.env.trial_roles, entry.Id) {
                    matched_any = true;
                    if !removed_ids.contains(&entry.Id) {
                        removed_ids.push(entry.Id);
                        notify.RoleIds.push(entry.Id);
                    }
                }
            }
            if !matched_any && !notify.RoleIds.contains(&raw_id) {
                notify.RoleIds.push(raw_id);
            }
        }
    }

    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.set_trial_roles(&ctx.quest_comp.info.env.trial_roles);
    }
    if !notify.RoleIds.is_empty() {
        ctx.sink.push(RoleTrialCloseNotify::MSG_ID, &notify);
    }

    if !removed_ids.is_empty() {
        if let (Some(scene), Some(role_comp)) =
            (ctx.scene.as_deref_mut(), ctx.role_comp.as_deref_mut())
        {
            trial_roles::restore_formation_after_trial_removal(
                scene,
                ctx.fs,
                ctx.assets,
                ctx.sink,
                ctx.events,
                ctx.basic_comp,
                role_comp,
                ctx.weapon_comp,
                ctx.echo_comp,
                ctx.quest_comp,
                &removed_ids,
                &ctx.quest_comp.info.env.trial_roles.clone(),
            )
            .map_err(io::Error::other)?;
        } else if ctx.quest_comp.info.env.trial_roles.is_empty() {
            trial_roles::clear_pre_trial_formation(ctx.quest_comp);
        }

        if let Some(role_comp) = ctx.role_comp.as_deref_mut() {
            trial_roles::cleanup_removed_trial_roles(
                role_comp,
                ctx.weapon_comp,
                ctx.echo_comp,
                &removed_ids,
            );
        }
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn toggle_map_mark<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    mark_id: i32,
    action_type: &str,
) {
    if mark_id <= 0 {
        return;
    }
    let show = action_type == "Show" || action_type == "Enable";
    if show {
        append_unique_int(&mut ctx.quest_comp.info.world.unlocked_map_marks, mark_id);
        remove_int_value(&mut ctx.quest_comp.info.world.hidden_map_marks, mark_id);
    } else {
        append_unique_int(&mut ctx.quest_comp.info.world.hidden_map_marks, mark_id);
        remove_int_value(&mut ctx.quest_comp.info.world.unlocked_map_marks, mark_id);
    }
    ctx.sink.push(
        MapMarkShowIdInfoUpdateNotify::MSG_ID,
        &MapMarkShowIdInfoUpdateNotify {
            ShowFlag: u32::from(show),
            IsShow: show,
            MarkId: mark_id,
            NeedFocus: false,
        },
    );
}

pub fn resolve_teleport_dungeon_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    did: i32,
    opt_loc_eid: Option<i64>,
) -> io::Result<()> {
    if did <= 0 {
        return Ok(());
    }
    let mut opt_pos = None;
    let mut opt_rot = None;
    if let Some(loc_eid) = opt_loc_eid
        && loc_eid > 0
        && let Some(cfg) = ctx.assets.find_level_entity(did, loc_eid)
    {
        opt_pos = asset_indexes::entity_world_pos(&cfg);
        opt_rot = Some(asset_indexes::entity_world_rot(&cfg));
    }
    transition_to_scene(ctx, did, opt_pos, opt_rot)
}

pub fn transition_to_scene<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    target_dungeon_id: i32,
    opt_pos: Option<[f32; 3]>,
    opt_rot: Option<[f32; 3]>,
) -> io::Result<()> {
    if target_dungeon_id <= 0 {
        return Ok(());
    }
    let cur_inst_id = ctx.scene.as_deref().map_or(8, |s| s.instance_id);
    let Some(target_inst) = ctx
        .assets
        .tables
        .instance_dungeon
        .get_data_by_id(&target_dungeon_id)
    else {
        return Ok(());
    };

    if target_dungeon_id == cur_inst_id {
        if let Some(pos) = opt_pos {
            let rot = opt_rot.unwrap_or([0.0, 0.0, 0.0]);
            update_player_scene_state(ctx, pos, Some(rot))?;
            ctx.sink.push(
                TeleportNotify::MSG_ID,
                &TeleportNotify {
                    MapId: ctx.assets.resolve_map_config_id(target_dungeon_id),
                    Pos: Some(Vector {
                        X: pos[0],
                        Y: pos[1],
                        Z: pos[2],
                    }),
                    Rot: Some(Vector {
                        X: rot[0],
                        Y: rot[1],
                        Z: rot[2],
                    }),
                    Reason: Some(TeleportReason::Action),
                    ..Default::default()
                },
            );
        }
        return Ok(());
    }

    if let Some(scene) = ctx.scene.as_deref_mut() {
        if scene.pending_transition_instance_id == Some(target_dungeon_id) {
            return Ok(());
        }
        scene.pending_transition_instance_id = Some(target_dungeon_id);
        if cur_inst_id == 8 && !scene.instance.players.is_empty() {
            scene.instance.prev_world_instance_id = 8;
            scene.instance.prev_world_location = scene.instance.players[0].location;
            scene.instance.prev_world_rotation = scene.instance.players[0].rotation;
        }
        scene.instance.active_dungeon_id = if target_dungeon_id == 8 {
            0
        } else {
            target_dungeon_id
        };
        let _ = scene.save_persistent(ctx.fs);
    }

    if let Some(sc) = ctx.scene_comp.as_deref_mut() {
        sc.last_scene_info.instance_id = target_dungeon_id;
    }
    let last_path = format!("player/{}/last_scene", ctx.basic_comp.player_id);
    comp_util::save_struct(
        ctx.fs,
        &LastSceneInfo {
            instance_id: target_dungeon_id,
        },
        &last_path,
    )?;

    ctx.sink.push(
        LeaveSceneNotify::MSG_ID,
        &LeaveSceneNotify {
            PlayerId: ctx.basic_comp.player_id,
            ..Default::default()
        },
    );

    let spawn_pos: Option<[f32; 3]> = opt_pos.or_else(|| {
        if target_dungeon_id != 8
            && (target_inst.BornPosition[0] != 0
                || target_inst.BornPosition[1] != 0
                || target_inst.BornPosition[2] != 0)
        {
            Some([
                target_inst.BornPosition[0] as f32,
                target_inst.BornPosition[1] as f32,
                target_inst.BornPosition[2] as f32,
            ])
        } else {
            None
        }
    });

    let spawn_rot: Option<[f32; 3]> = opt_rot.or_else(|| {
        if target_dungeon_id != 8
            && (target_inst.BornRotation[0] != 0
                || target_inst.BornRotation[1] != 0
                || target_inst.BornRotation[2] != 0)
        {
            Some([
                target_inst.BornRotation[0] as f32,
                target_inst.BornRotation[1] as f32,
                target_inst.BornRotation[2] as f32,
            ])
        } else {
            None
        }
    });

    ctx.events.enqueue(Event::InitialSceneJoin {
        player_id: ctx.basic_comp.player_id,
        map_config_id: ctx.assets.resolve_map_config_id(target_dungeon_id),
        instance_id: target_dungeon_id,
        location: spawn_pos,
        rotation: spawn_rot,
        pending_flow: None,
    });
    Ok(())
}

pub fn leave_current_dungeon<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    pos_entity_id: i32,
) -> io::Result<()> {
    let cur_id = ctx.scene.as_deref().map_or(8, |s| s.instance_id);
    if cur_id == 8 {
        return Ok(());
    }

    let mut target_map_id = 8;
    let mut opt_pos = None;
    if let Some(cur_inst) = ctx.assets.tables.instance_dungeon.get_data_by_id(&cur_id)
        && cur_inst.RecoverWorldLocation.len() >= 4
        && cur_inst.RecoverWorldLocation[0] > 0
    {
        target_map_id = cur_inst.RecoverWorldLocation[0];
        let [rx, ry, rz] = [
            cur_inst.RecoverWorldLocation[1],
            cur_inst.RecoverWorldLocation[2],
            cur_inst.RecoverWorldLocation[3],
        ];
        if rx != 0 || ry != 0 || rz != 0 {
            opt_pos = Some([rx as f32, ry as f32, rz as f32]);
        }
    }

    if pos_entity_id > 0
        && let Some(cfg) = ctx
            .assets
            .find_level_entity(target_map_id, i64::from(pos_entity_id))
    {
        opt_pos = asset_indexes::entity_world_pos(&cfg);
    }

    remove_trial_roles(ctx, &[])?;
    transition_to_scene(ctx, target_map_id, opt_pos, None)
}

pub fn send_quest_entity_visibility<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_ids: &[i32],
    visible: bool,
) -> io::Result<()> {
    if ctx.scene.is_none() || entity_ids.is_empty() {
        return Ok(());
    }

    if visible {
        for &eid in entity_ids {
            if eid <= 0 {
                continue;
            }
            ensure_quest_entity_spawned(ctx, eid);
            append_unique_int(&mut ctx.quest_comp.info.world.shown_entities, eid);
            remove_int_value(&mut ctx.quest_comp.info.world.hidden_entities, eid);
        }
    } else {
        for &eid in entity_ids {
            if eid <= 0 {
                continue;
            }
            append_unique_int(&mut ctx.quest_comp.info.world.hidden_entities, eid);
            remove_int_value(&mut ctx.quest_comp.info.world.shown_entities, eid);
        }
    }

    let scene = ctx.scene.as_deref_mut().expect("checked");
    let mut combat_pack = Vec::new();
    let mut flow_ids = Vec::with_capacity(entity_ids.len());
    for &target_cfg in entity_ids {
        let matched_nids: Vec<i64> = scene
            .entities
            .iter()
            .filter(|s| s.config.config_id == target_cfg)
            .map(|s| s.net_id)
            .collect();
        for nid in matched_nids {
            let _ = scene.set_entity_visibility(nid, visible, &mut combat_pack);
        }
        flow_ids.push(target_cfg);
    }

    if !combat_pack.is_empty() {
        ctx.sink.push(
            sonoro_proto::pb::CombatReceivePackNotify::MSG_ID,
            &sonoro_proto::pb::CombatReceivePackNotify { Data: combat_pack },
        );
    }
    if !flow_ids.is_empty() {
        ctx.sink.push(
            FlowShowEntityNotify::MSG_ID,
            &FlowShowEntityNotify {
                EntityIds: flow_ids,
                IsHide: !visible,
            },
        );
    }
    Ok(())
}

pub fn apply_entity_state_change<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    entity_cfg_id: i32,
    state_str: &str,
) -> io::Result<()> {
    if entity_cfg_id <= 0 || state_str.is_empty() {
        return Ok(());
    }
    ctx.quest_comp
        .info
        .set_entity_state(i64::from(entity_cfg_id), state_str);

    let is_sleep = state_str.contains("休眠") || state_str.contains("未激活");
    let is_visible = !is_sleep && !state_str.contains("隐藏");
    if !is_sleep {
        ensure_quest_entity_spawned(ctx, entity_cfg_id);
    }

    if ctx.scene.is_some() {
        let mut combat_pack = Vec::new();
        let mut state_updates = Vec::new();
        let net_ids: Vec<i64> = {
            let scene = ctx.scene.as_deref_mut().expect("checked");
            scene
                .entities
                .iter()
                .filter(|s| s.config.config_id == entity_cfg_id)
                .map(|s| s.net_id)
                .collect()
        };
        for nid in net_ids {
            let scene = ctx.scene.as_deref_mut().expect("checked");
            if let Some(storage) = scene.entity_mut(nid) {
                storage.config.state = if state_str.contains("常态") {
                    EntityState::Default
                } else if is_sleep {
                    EntityState::Sleep
                } else {
                    EntityState::Other
                };
            }
            if let Some(upd) =
                scene.update_entity_state_tag(&ctx.assets.tables.gameplay_tag_remap, nid, state_str)
            {
                state_updates.push((nid, upd));
            }
            let _ = scene.set_entity_visibility(nid, is_visible, &mut combat_pack);
        }
        for (nid, upd) in state_updates {
            emit_entity_state_tags(ctx, nid, upd);
        }
        if !combat_pack.is_empty() {
            ctx.sink.push(
                sonoro_proto::pb::CombatReceivePackNotify::MSG_ID,
                &sonoro_proto::pb::CombatReceivePackNotify { Data: combat_pack },
            );
        }
    }

    if let Some(cb) = callbacks.on_entity_state_changed {
        cb(ctx, entity_cfg_id, state_str)?;
    }
    execute_entity_state_change_behaviors(ctx, callbacks, entity_cfg_id, state_str)?;
    Ok(())
}

fn execute_entity_state_change_behaviors<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    entity_cfg_id: i32,
    state_str: &str,
) -> io::Result<()> {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use sonoro_gamesv_data::tables::entity_components::components::MergeFields;

    thread_local! {
        static ACTIVE_BEHAVIORS: RefCell<HashSet<(i32, String)>> = RefCell::new(HashSet::new());
    }

    let key = (entity_cfg_id, state_str.to_string());
    let inserted = ACTIVE_BEHAVIORS.with(|set| set.borrow_mut().insert(key.clone()));
    if !inserted {
        return Ok(());
    }

    struct ActiveBehaviorGuard((i32, String));
    impl Drop for ActiveBehaviorGuard {
        fn drop(&mut self) {
            ACTIVE_BEHAVIORS.with(|set| {
                set.borrow_mut().remove(&self.0);
            });
        }
    }
    let _guard = ActiveBehaviorGuard(key);

    let map_id = ctx
        .scene
        .as_deref()
        .map_or(8, |s| ctx.assets.resolve_map_config_id(s.instance_id));
    let Some(lcfg) = ctx.assets.find_level_entity(map_id, i64::from(entity_cfg_id)) else {
        return Ok(());
    };
    let mut esc = lcfg.ComponentsData.EntityStateComponent.clone();
    if let Some(tpl) = ctx.assets.find_template_config(&lcfg.BlueprintType)
        && let Some(tpl_esc) = &tpl.ComponentsData.EntityStateComponent
    {
        match &mut esc {
            Some(cur) => cur.merge_fields_from(tpl_esc),
            None => esc = Some(tpl_esc.clone()),
        }
    }
    let Some(esc) = esc else {
        return Ok(());
    };
    if esc.StateChangeBehaviors.is_empty() {
        return Ok(());
    }

    let mut matched_actions = Vec::new();
    for beh in &esc.StateChangeBehaviors {
        let Some(obj) = beh.as_object() else {
            continue;
        };
        if json_str(obj.get("State")) != Some(state_str) {
            continue;
        }
        if let Some(cond_val) = obj.get("Condition")
            && !cond_val.is_null()
        {
            let mut eval_ctx = ctx.make_eval_context();
            eval_ctx.self_entity_config_id = entity_cfg_id;
            if !crate::helpers::condition::eval_inline_condition_block(&eval_ctx, cond_val) {
                continue;
            }
        }
        if let Some(acts) = obj
            .get("Action")
            .or_else(|| obj.get("Actions"))
            .and_then(|v| v.as_array())
        {
            matched_actions.extend(acts.iter().cloned());
        }
    }
    if matched_actions.is_empty() {
        return Ok(());
    }

    let mut behavior_cbs = ActionCallbacks {
        on_entity_state_changed: callbacks.on_entity_state_changed,
        ..Default::default()
    };
    execute_action_list(ctx, &mut behavior_cbs, entity_cfg_id, 0, &matched_actions)
}

pub fn sync_world_state_on_join<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    if ctx.quest_comp.info.env.weather_id > 0 {
        ctx.sink.push(
            WeatherNotify::MSG_ID,
            &WeatherNotify {
                WeatherId: ctx.quest_comp.info.env.weather_id,
                IsClient: false,
            },
        );
    }
    if ctx.quest_comp.info.env.time_locked {
        ctx.sink.push(
            TimeLockInfoNotify::MSG_ID,
            &TimeLockInfoNotify {
                IsLock: true,
                Hour: i64::from(ctx.quest_comp.info.env.time_lock_hour),
                Minute: i64::from(ctx.quest_comp.info.env.time_lock_minute),
            },
        );
    }
    if !ctx.quest_comp.info.env.trial_roles.is_empty() {
        let trial_roles_copy = ctx.quest_comp.info.env.trial_roles.clone();
        let is_temp_team = !ctx.quest_comp.info.env.pre_trial_roles.is_empty();
        apply_trial_roles_with_options(
            ctx,
            &trial_roles_copy,
            true,
            is_temp_team,
            is_temp_team,
            &[],
        )?;
    }
    for &mark_id in &ctx.quest_comp.info.world.unlocked_map_marks {
        ctx.sink.push(
            MapMarkShowIdInfoUpdateNotify::MSG_ID,
            &MapMarkShowIdInfoUpdateNotify {
                ShowFlag: 1,
                IsShow: true,
                MarkId: mark_id,
                NeedFocus: false,
            },
        );
    }
    for &mark_id in &ctx.quest_comp.info.world.hidden_map_marks {
        ctx.sink.push(
            MapMarkShowIdInfoUpdateNotify::MSG_ID,
            &MapMarkShowIdInfoUpdateNotify {
                ShowFlag: 0,
                IsShow: false,
                MarkId: mark_id,
                NeedFocus: false,
            },
        );
    }

    if ctx.scene.is_some() {
        let mut combat_pack = Vec::new();
        {
            let hidden_entities = ctx.quest_comp.info.world.hidden_entities.clone();
            let hidden_groups = ctx.quest_comp.info.world.hidden_groups.clone();
            let scene = ctx.scene.as_deref_mut().expect("checked");
            for hid_cfg in hidden_entities {
                if let Some(nid) = scene.find_entity_net_id_by_config_id(hid_cfg) {
                    let _ = scene.set_entity_visibility(nid, false, &mut combat_pack);
                }
            }
            for grp in &hidden_groups {
                for &eid in &grp.entity_ids {
                    if let Some(nid) = scene.find_entity_net_id_by_config_id(eid) {
                        let _ = scene.set_entity_visibility(nid, false, &mut combat_pack);
                    }
                }
            }
        }

        let entity_states = ctx.quest_comp.info.world.entity_states.clone();
        let mut state_updates = Vec::new();
        {
            let scene = ctx.scene.as_deref_mut().expect("checked");
            for rec in &entity_states {
                let Ok(cfg_id) = i32::try_from(rec.entity_id) else {
                    continue;
                };
                if let Some(nid) = scene.find_entity_net_id_by_config_id(cfg_id)
                    && let Some(upd) = scene.update_entity_state_tag(
                        &ctx.assets.tables.gameplay_tag_remap,
                        nid,
                        &rec.state_name,
                    )
                {
                    state_updates.push((nid, upd));
                }
            }
        }
        for (nid, upd) in state_updates {
            emit_entity_state_tags(ctx, nid, upd);
        }

        if let Some(pc) = ctx.progress_comp.as_deref() {
            let ints_snapshot = pc.info.dynamic.ints.clone();
            let vehicle_id = i32::try_from(pc.info.get_key_int("vehicle:cur")).unwrap_or(0);
            let vehicle_seat = i32::try_from(pc.info.get_key_int("vehicle:seat")).unwrap_or(0);
            {
                let scene = ctx.scene.as_deref_mut().expect("checked");
                for entry in &ints_snapshot {
                    if let Some(rest) = entry.key.strip_prefix("gravity:")
                        && let Ok(config_id) = rest.parse::<i32>()
                        && config_id > 0
                        && (0..=3).contains(&entry.val)
                        && let Some(nid) = scene.find_entity_net_id_by_config_id(config_id)
                    {
                        ctx.sink.push(
                            GravityFlipUpdateTypeNotify::MSG_ID,
                            &GravityFlipUpdateTypeNotify {
                                NewGravityFlipType: Some(GravityFlipType::from_i32_or_default(
                                    entry.val as i32,
                                )),
                                EntityId: nid,
                            },
                        );
                    }
                }
                for entry in &ints_snapshot {
                    if let Some(rest) = entry.key.strip_prefix("lift_target:")
                        && let Ok(config_id) = rest.parse::<i32>()
                        && config_id > 0
                        && let Some(net_id) = scene.find_entity_net_id_by_config_id(config_id)
                        && let Ok(location) = i32::try_from(entry.val)
                        && let Some(storage) = scene.entity_mut(net_id)
                    {
                        storage.world_item.lift = Some(LiftComponent {
                            location,
                        });
                        ctx.sink.push(
                            ElevatorMoveNotify::MSG_ID,
                            &ElevatorMoveNotify {
                                EntityId: net_id,
                                Location: location,
                                MoveType: None,
                            },
                        );
                    }
                }
            }
            if vehicle_id > 0 {
                ensure_quest_entity_spawned(ctx, vehicle_id);
                let player_id = ctx.basic_comp.player_id;
                let scene = ctx.scene.as_deref_mut().expect("checked");
                if let Some(net_id) = scene.find_entity_net_id_by_config_id(vehicle_id)
                    && let Some(storage) = scene.entity_mut(net_id)
                    && let Some(veh) = storage.vehicle.as_mut()
                {
                    veh.occupied = true;
                    ctx.sink.push(
                        VehicleUpdateNotify::MSG_ID,
                        &VehicleUpdateNotify {
                            EntityId: net_id,
                            PlayerId: player_id,
                            SeatId: vehicle_seat,
                            ..Default::default()
                        },
                    );
                }
            }
            if let (Some(scene), Some(pc)) = (ctx.scene.as_deref_mut(), ctx.progress_comp.as_deref())
            {
                sync_persisted_vehicle_npc_links(scene, pc, ctx.sink, None);
            }
        }
        if !combat_pack.is_empty() {
            ctx.sink.push(
                sonoro_proto::pb::CombatReceivePackNotify::MSG_ID,
                &sonoro_proto::pb::CombatReceivePackNotify { Data: combat_pack },
            );
        }
    }

    if !ctx.quest_comp.info.env.audio_states.is_empty() {
        let states = ctx
            .quest_comp
            .info
            .env
            .audio_states
            .iter()
            .map(|rec| AudioStatePb {
                Group: rec.group.as_bytes().to_vec(),
                State: rec.state.as_bytes().to_vec(),
                ..Default::default()
            })
            .collect();
        ctx.sink.push(
            AudioStateUpdateNotify::MSG_ID,
            &AudioStateUpdateNotify {
                AudioStates: states,
            },
        );
    }
    for &gid in &ctx.quest_comp.info.world.active_guests {
        ctx.sink.push(
            AddGuestNotify::MSG_ID,
            &AddGuestNotify { GuestId: gid },
        );
    }
    if !ctx.map_comp.info.temp_teleports.is_empty() {
        let tp_list = ctx
            .map_comp
            .info
            .temp_teleports
            .iter()
            .map(|tp| tp.to_pb())
            .collect();
        ctx.sink.push(
            TemporaryTeleportAllInfoNotify::MSG_ID,
            &TemporaryTeleportAllInfoNotify {
                AllTemporaryTeleportInfo: tp_list,
            },
        );
    }
    Ok(())
}

pub fn emit_entity_state_tag_update_checked<S: PushSink + ?Sized>(
    scene: &Scene,
    assets: Option<&Assets>,
    sink: &mut S,
    net_id: i64,
    upd: StateTagUpdate,
) {
    if tags_handler::has_client_level_tag_component(scene, assets, net_id) {
        emit_entity_state_tag_update(sink, net_id, upd);
        return;
    }
    if upd.new_tag_id != 0 {
        sink.push(
            EntityStateReadyNotify::MSG_ID,
            &EntityStateReadyNotify {
                EntityId: net_id,
                TagId: upd.new_tag_id,
                Ready: true,
            },
        );
    }
}

pub fn emit_entity_state_tag_update<S: PushSink + ?Sized>(
    sink: &mut S,
    net_id: i64,
    upd: StateTagUpdate,
) {
    let mut tag_notify = EntityCommonTagNotify {
        Id: net_id,
        Tags: Vec::new(),
    };
    if upd.old_tag_id != 0 && upd.old_tag_id != upd.new_tag_id {
        tag_notify.Tags.push(CommonTagData {
            TagId: upd.old_tag_id,
            RemoveTagIds: false,
        });
    }
    if upd.new_tag_id != 0 {
        tag_notify.Tags.push(CommonTagData {
            TagId: upd.new_tag_id,
            RemoveTagIds: true,
        });
    }
    if !tag_notify.Tags.is_empty() {
        sink.push(EntityCommonTagNotify::MSG_ID, &tag_notify);
    }
    if upd.new_tag_id != 0 {
        sink.push(
            EntityStateReadyNotify::MSG_ID,
            &EntityStateReadyNotify {
                EntityId: net_id,
                TagId: upd.new_tag_id,
                Ready: true,
            },
        );
    }
}

fn emit_entity_state_tags<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    net_id: i64,
    upd: StateTagUpdate,
) {
    if let Some(scene) = ctx.scene.as_deref() {
        emit_entity_state_tag_update_checked(scene, Some(ctx.assets), ctx.sink, net_id, upd);
    } else {
        emit_entity_state_tag_update(ctx.sink, net_id, upd);
    }
}

fn apply_change_entity_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let action_type = json_str(params.get("Type")).unwrap_or("Directly");
    if action_type == "BatchDirectly" {
        let Some(state_str) = json_str(params.get("State")) else {
            return Ok(());
        };
        for eid in parse_int_list(params.get("EntityIds")) {
            apply_entity_state_change(ctx, callbacks, eid, state_str)?;
        }
        return Ok(());
    }
    let Some(entity_id) = json_int(params.get("EntityId")) else {
        return Ok(());
    };
    if action_type == "Loop" {
        let cur = ctx
            .quest_comp
            .info
            .get_entity_state(i64::from(entity_id))
            .unwrap_or("");
        let next_state = if cur.contains("激活") {
            "关卡.Common.状态.常态"
        } else {
            "关卡.Common.状态.激活"
        };
        return apply_entity_state_change(ctx, callbacks, entity_id, next_state);
    }
    let Some(state_str) = json_str(params.get("State")) else {
        return Ok(());
    };
    apply_entity_state_change(ctx, callbacks, entity_id, state_str)
}

fn apply_change_self_entity_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(state_str) = json_str(params.get("EntityState"))
        .or_else(|| json_str(params.get("State")))
    else {
        return Ok(());
    };
    let Some(cfg_id) = ctx.scene.as_deref().and_then(|s| {
        let owner_nid = s.active_flow_owner_entity_id;
        if owner_nid == 0 {
            None
        } else {
            s.entity(owner_nid).map(|e| e.config.config_id)
        }
    }) else {
        return Ok(());
    };
    apply_entity_state_change(ctx, callbacks, cfg_id, state_str)
}

fn apply_set_entity_visible<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let visible = json_bool(params.get("Visible")).unwrap_or(true);
    let ids = parse_int_list(params.get("EntityIds"));
    send_quest_entity_visibility(ctx, &ids, visible)
}

fn apply_entity_visibility_list<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    ids_opt: Option<&Value>,
    visible: bool,
) -> io::Result<()> {
    let ids = parse_int_list(ids_opt);
    send_quest_entity_visibility(ctx, &ids, visible)
}

fn apply_hide_group<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let group_key = json_str(params.get("GroupKey")).unwrap_or("");
    let Some(cfg_val) = params.get("HideConfig").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let hide_ty = json_str(cfg_val.get("Type")).unwrap_or("");
    let mut entities_to_hide = Vec::new();
    let mut layers_to_unload = Vec::new();
    let mut lps_to_hide = Vec::new();

    match hide_ty {
        "SpecifyEntity" => {
            entities_to_hide.extend(parse_int_list(cfg_val.get("EntityIds")));
        }
        "SpecifyDataLayers" => {
            layers_to_unload.extend(parse_int_list(cfg_val.get("DataLayers")));
        }
        "SpecifyLevelPlay" => {
            let lps = parse_int_list(cfg_val.get("LevelPlays"));
            lps_to_hide.extend_from_slice(&lps);
            for lpid in lps {
                if let Some(lp) = ctx.assets.tables.level_play_data.get_data_by_id(&lpid)
                    && lp.Data.LevelPlayEntityId > 0
                {
                    entities_to_hide.push(lp.Data.LevelPlayEntityId);
                }
            }
        }
        "WorldEntityAndLevelPlay" | "WorldMonsterAndMonsterTreasure" => {
            let only_m_and_c = hide_ty == "WorldMonsterAndMonsterTreasure";
            for rid in parse_int_list(cfg_val.get("HideRangeEntities")) {
                collect_config_ids_in_range_entity(ctx, rid, only_m_and_c, &mut entities_to_hide);
            }
        }
        _ => {}
    }

    if !group_key.is_empty() {
        ctx.quest_comp.info.set_hidden_group(
            group_key,
            &entities_to_hide,
            &layers_to_unload,
            &lps_to_hide,
        );
    }
    if !entities_to_hide.is_empty() {
        send_quest_entity_visibility(ctx, &entities_to_hide, false)?;
    }
    if !layers_to_unload.is_empty() {
        apply_data_layer_change(ctx, &[], &layers_to_unload)?;
    }
    Ok(())
}

fn apply_show_hided_group<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(group_key) = json_str(params.get("GroupKey")) else {
        return Ok(());
    };
    let Some(rec) = ctx.quest_comp.info.pop_hidden_group(group_key) else {
        return Ok(());
    };
    if !rec.entity_ids.is_empty() {
        send_quest_entity_visibility(ctx, &rec.entity_ids, true)?;
    }
    if !rec.data_layers.is_empty() {
        apply_data_layer_change(ctx, &rec.data_layers, &[])?;
    }
    Ok(())
}

fn apply_hide_target_range<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(rid) = json_int(params.get("RangeEntity")) else {
        return Ok(());
    };
    let only_m_and_c = params
        .get("HideConfig")
        .and_then(|v| v.as_object())
        .and_then(|o| json_str(o.get("Type")))
        == Some("WorldMonsterAndMonsterTreasure");
    let mut ids = Vec::new();
    collect_config_ids_in_range_entity(ctx, rid, only_m_and_c, &mut ids);
    ctx.quest_comp
        .info
        .set_hidden_group(&format!("__range_{rid}"), &ids, &[], &[]);
    if !ids.is_empty() {
        send_quest_entity_visibility(ctx, &ids, false)?;
    }
    Ok(())
}

fn apply_show_target_range<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    for rid in parse_int_list(params.get("RangeEntities")) {
        if let Some(rec) = ctx.quest_comp.info.pop_hidden_group(&format!("__range_{rid}"))
            && !rec.entity_ids.is_empty()
        {
            send_quest_entity_visibility(ctx, &rec.entity_ids, true)?;
        }
    }
    Ok(())
}

fn apply_hide_by_range_in_flow<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let mut ids = parse_int_list(params.get("EntityIds"));
    if let Some(scene) = ctx.scene.as_deref()
        && let Some(center) = parse_vector_pos(params.get("Center"))
    {
        let radius = json_float(params.get("Radius")).unwrap_or(2000.0);
        let mut net_ids = Vec::new();
        scene.collect_entities_in_radius(center, radius, false, &mut net_ids);
        for nid in net_ids {
            if let Some(storage) = scene.entity(nid) {
                let cfg_id = storage.config.config_id;
                if cfg_id > 0 && !ids.contains(&cfg_id) {
                    ids.push(cfg_id);
                }
            }
        }
    }
    ctx.quest_comp
        .info
        .set_hidden_group("__flow_range", &ids, &[], &[]);
    if !ids.is_empty() {
        send_quest_entity_visibility(ctx, &ids, false)?;
    }
    Ok(())
}

pub(crate) fn restore_flow_range_hidden_group<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let Some(rec) = ctx.quest_comp.info.pop_hidden_group("__flow_range") else {
        return Ok(());
    };
    if !rec.entity_ids.is_empty() {
        send_quest_entity_visibility(ctx, &rec.entity_ids, true)?;
    }
    Ok(())
}

fn restore_all_hidden_groups<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    while !ctx.quest_comp.info.world.hidden_groups.is_empty() {
        let key_copy = ctx.quest_comp.info.world.hidden_groups[0]
            .group_key
            .clone();
        let Some(rec) = ctx.quest_comp.info.pop_hidden_group(&key_copy) else {
            break;
        };
        if !rec.entity_ids.is_empty() {
            send_quest_entity_visibility(ctx, &rec.entity_ids, true)?;
        }
        if !rec.data_layers.is_empty() {
            apply_data_layer_change(ctx, &rec.data_layers, &[])?;
        }
    }
    Ok(())
}

fn collect_config_ids_in_range_entity<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    range_cfg_id: i32,
    only_monsters_and_chests: bool,
    out: &mut Vec<i32>,
) {
    let Some(scene) = ctx.scene.as_deref() else {
        return;
    };
    let Some(cfg) = ctx
        .assets
        .find_level_entity(scene.instance_id, i64::from(range_cfg_id))
    else {
        return;
    };
    let Some(center) = asset_indexes::entity_world_pos(&cfg) else {
        return;
    };
    let rot = asset_indexes::entity_world_rot(&cfg);

    let mut comps = cfg.ComponentsData.clone();
    if let Some(tpl) = ctx.assets.find_template_config(&cfg.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut comps);
    }
    let shape = comps
        .RangeComponent
        .and_then(|rc| rc.Shape)
        .unwrap_or_else(|| ShapeConfig {
            Radius: Some(3000.0),
            ..Default::default()
        });

    for storage in &scene.entities {
        if only_monsters_and_chests
            && !matches!(
                storage.config.entity_type,
                EntityType::Monster | EntityType::SceneItem
            )
        {
            continue;
        }
        let cid = storage.config.config_id;
        if cid <= 0 {
            continue;
        }
        let Some(pos) = storage.position.map(|p| p.location) else {
            continue;
        };
        if !is_pos_inside_range_shape(pos, center, rot, &shape, 3000.0, 0.0) {
            continue;
        }
        if !out.contains(&cid) {
            out.push(cid);
        }
    }
}

fn set_entity_locked<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    eid: i32,
    locked: bool,
) {
    if eid <= 0 {
        return;
    }
    if locked {
        append_unique_int(&mut ctx.quest_comp.info.world.locked_entities, eid);
    } else {
        remove_int_value(&mut ctx.quest_comp.info.world.locked_entities, eid);
    }
}

fn apply_lock_entities<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    ids_opt: Option<&Value>,
    locked: bool,
) {
    for eid in parse_int_list(ids_opt) {
        set_entity_locked(ctx, eid, locked);
    }
}

fn apply_stop_scene_item_move<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    for eid in parse_int_list(params.get("EntityIds")) {
        if eid <= 0 {
            continue;
        }
        record_progress_int(ctx, &format!("quest_spline_active:{eid}"), 0, false)?;
        record_progress_int(ctx, &format!("scene_item_spline_active:{eid}"), 0, false)?;
        if let Some(scene) = ctx.scene.as_deref_mut()
            && let Some(net_id) = scene.find_entity_net_id_by_config_id(eid)
        {
            if let Some(storage) = scene.entity_mut(net_id) {
                storage.world_item.move_spline = None;
            }
            push_spline_status(ctx.sink, net_id, MoveStatus::Stop, 0, None, None);
        }
    }
    Ok(())
}

fn apply_reset_player_pos_presentation<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(inst_id) = ctx.scene.as_deref().map(|s| s.instance_id) else {
        return Ok(());
    };
    if let Some(pres) = params.get("Presentation").and_then(|v| v.as_object()) {
        for cue_id in parse_int_list(pres.get("GamePlayCues")) {
            ctx.sink.push(
                GameplayCueNotify::MSG_ID,
                &GameplayCueNotify {
                    GameplayCueId: i64::from(cue_id),
                },
            );
        }
    }
    let Some(rpc) = params.get("ResetPosConfig").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    if json_str(rpc.get("Type")).unwrap_or("") != "PosEntity" {
        return Ok(());
    }
    let Some(eid) = json_int(rpc.get("Entity")) else {
        return Ok(());
    };
    let Some(cfg) = ctx.assets.find_level_entity(inst_id, i64::from(eid)) else {
        return Ok(());
    };
    let Some(pos) = asset_indexes::entity_world_pos(&cfg) else {
        return Ok(());
    };
    let rot = asset_indexes::entity_world_rot(&cfg);
    update_player_scene_state(ctx, pos, Some(rot))?;
    ctx.sink.push(
        TeleportNotify::MSG_ID,
        &TeleportNotify {
            MapId: inst_id,
            Pos: Some(Vector {
                X: pos[0],
                Y: pos[1],
                Z: pos[2],
            }),
            Rot: Some(Vector {
                X: rot[0],
                Y: rot[1],
                Z: rot[2],
            }),
            Reason: Some(TeleportReason::Action),
            ..Default::default()
        },
    );
    Ok(())
}

fn is_tracked_or_client_action(name: &str) -> bool {
    matches!(
        name,
        "ActiveAntiGravitySafePoint"
            | "AdjustPlayerCameraGravity"
            | "BvbPlayDialog"
            | "BvbPlayerOperationConstraint"
            | "BvbSendAiEvent"
            | "BvbSendSystemEvent"
            | "ChangeNpcAbpState"
            | "ClearFishingCabinInSaleItems"
            | "ClientPreEnableSubLevels"
            | "ClientSetPlayerOperationRestriction"
            | "ClientSetSeasonState"
            | "ClientVehicleTeleport"
            | "CloseWalkingOverlayMontage"
            | "DisableEntityLookAt"
            | "EnableAoiNotify"
            | "EnableKey4Func"
            | "EnableTakePhotoClip"
            | "EndCommonTip"
            | "EnterOrbitalCamera"
            | "ExecBpActorFunc"
            | "ExitOrbitalCamera"
            | "FireBullet"
            | "FireBulletEffect"
            | "ModifyActorMaterial"
            | "ModifyActorMedia"
            | "MotorSoarInWindField"
            | "OpenQte"
            | "OpenQuickHackSystem"
            | "PlayLevelSequence"
            | "PlayWalkingOverlayMontage"
            | "PlayerInput"
            | "PlayerLookTowards"
            | "RemovePreloadResource"
            | "ReportTrackingEvent"
            | "RequestSystemFunction"
            | "ResumeSystemRewardPopup"
            | "RunActorCustomEvent"
            | "SendAiEvent"
            | "SendClientEvent"
            | "SendSceneActorsEvent"
            | "SeqEventAddTagToSelf"
            | "SeqEventFireBullet"
            | "SetActorVar"
            | "SetAudioEnable"
            | "SetJigsawFoundation"
            | "SetJigsawItem"
            | "SetSkillButtonEffect"
            | "SetSportsState"
            | "SetTargetElementalAbility"
            | "SlidePerformStart"
            | "SlideRailStart"
            | "SpecialOpModifyActorMaterialParamBySplineProgress"
            | "StartMotorCruise"
            | "TetrisThrowBlock"
            | "TetrisTriggerDelayFall"
            | "TrapDefenseChangeMiniMap"
            | "TrapDefensePlayerOperationConstraint"
            | "TriggerDeadeyeMode"
            | "TriggerSpecificScanEffect"
            | "UsePhantomSkill"
            | "VehicleEnterNpc"
            | "VehiclePlayPassengerVoice"
            | "VehicleSetSpeedImmediately"
            | "VehicleSprint"
            | "VehicleWaterfallClimbing"
            | "VisionDisplayTarget"
            | "AdjustPlayerCamera"
            | "CameraLookAt"
            | "CharacterLookAt"
            | "CommonTip"
            | "CommonTip2"
            | "EnableClientFunction"
            | "EnableEntityLookAt"
            | "EnterMovieCamera"
            | "EnterMovieMode"
            | "EntityLookAt"
            | "EntityTurnTo"
            | "ExitMovieCamera"
            | "ExitMovieMode"
            | "FadeInScreen"
            | "FadeOutScreen"
            | "GuestOperateUiAnimation"
            | "Log"
            | "PlayEffect"
            | "PlayEffect2"
            | "PlayMontage"
            | "PlayRegisteredMontage"
            | "PlayerLookAt"
            | "PostAkEvent"
            | "Prompt"
            | "ResetPlayerCameraFocus"
            | "RestorePlayerCameraAdjustment"
            | "SetSubScreenMode"
            | "SetTimeScale"
            | "StopCameraLookAt"
            | "StopUiScreenEffect"
            | "ToggleHighlightExploreUi"
            | "ToggleScanSplineEffect"
            | "TriggerCameraShake"
            | "TriggerGamepadShake"
            | "Wait"
    )
}

fn collect_trial_role_ids(params: &Map<String, Value>) -> Vec<i32> {
    let mut ids = Vec::new();
    if let Some(Value::Array(arr)) = params.get("CharacterGroupNew") {
        for entry in arr {
            if let Some(obj) = entry.as_object()
                && let Some(cid) = json_int(obj.get("CharacterId"))
                && cid > 0
            {
                ids.push(cid);
            }
        }
    }
    if ids.is_empty() {
        ids.extend(parse_int_list(params.get("CharacterGroup")));
    }
    ids
}

fn apply_add_trial_character<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let mut ids = collect_trial_role_ids(params);
    if ids.is_empty()
        && let Some(cid) = json_int(params.get("CharacterId"))
        && cid > 0
    {
        ids.push(cid);
    }
    let auto_change = json_bool(params.get("AutoChange")).unwrap_or(false);
    let create_temp_team = json_bool(params.get("CreateTempTeam")).unwrap_or(false);
    let dungeon_ids = parse_int_list(params.get("DungeonList"));
    if !ids.is_empty() {
        apply_trial_roles_with_options(ctx, &ids, true, create_temp_team, auto_change, &dungeon_ids)?;
    }
    Ok(())
}

fn apply_remove_trial_character<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let mut ids = parse_int_list(params.get("CharacterGroup"));
    if ids.is_empty()
        && let Some(cid) = json_int(params.get("CharacterId"))
        && cid > 0
    {
        ids.push(cid);
    }
    remove_trial_roles(ctx, &ids)
}

fn apply_area_time_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(cfg_val) = params.get("SetAreaTimeConfig").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let op_type = json_int(cfg_val.get("Type")).unwrap_or(0);
    let locked = op_type == 0;
    let mut hour = ctx.quest_comp.info.env.time_lock_hour;
    let mut min = ctx.quest_comp.info.env.time_lock_minute;
    if let Some(lt) = cfg_val.get("LockTime").and_then(|v| v.as_object()) {
        hour = json_int(lt.get("Hour")).unwrap_or(hour);
        min = json_int(lt.get("Minutes"))
            .or_else(|| json_int(lt.get("Minute")))
            .unwrap_or(min);
    }
    for area_id in parse_int_list(cfg_val.get("AreaIds")) {
        ctx.quest_comp.info.set_area_time(area_id, hour, min, locked);
    }
    apply_time_lock_state(ctx, locked, Some(hour), Some(min))
}

fn apply_phone_system_interact<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(itype) = params.get("InteractType").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let kind = json_str(itype.get("Type")).unwrap_or("");
    if kind == "SendPhoneMessage" {
        if let Some(cfg) = itype
            .get("PhoneRecordActionConfig")
            .and_then(|v| v.as_object())
            && let Some(pmid) = json_int(cfg.get("PhoneMessageId"))
            && pmid > 0
        {
            let unlock_sec = now_unix_secs();
            let (newly_unlocked, info_pb) = if let Some(pc) = ctx.progress_comp.as_deref_mut() {
                pc.info.remove_claim(&format!("sm:del:{pmid}"));
                let fresh = pc.info.mark_claim(&format!("sm:unlock:{pmid}"));
                let saved_time = if fresh {
                    pc.info.set_key_int(&format!("sm:time:{pmid}"), unlock_sec);
                    unlock_sec
                } else {
                    pc.info.get_key_int(&format!("sm:time:{pmid}"))
                };
                let saved_last = pc.info.get_key_int(&format!("sm:last:{pmid}"));
                let _ = pc.save(ctx.fs);
                (
                    fresh,
                    ShortMessageInfo {
                        ConfigId: pmid,
                        LastConfigId: if saved_last > 0 { saved_last as i32 } else { pmid },
                        IsRead: pc.info.has_claim(&format!("sm:read:{pmid}")),
                        IsReceived: pc.info.has_claim(&format!("sm:recv:{pmid}")),
                        IsFinish: pc.info.has_claim(&format!("sm:fin:{pmid}")),
                        UnlockTime: if saved_time > 0 { saved_time } else { unlock_sec },
                        ..Default::default()
                    },
                )
            } else {
                (
                    true,
                    ShortMessageInfo {
                        ConfigId: pmid,
                        LastConfigId: pmid,
                        UnlockTime: unlock_sec,
                        ..Default::default()
                    },
                )
            };
            if newly_unlocked {
                ctx.sink.push(
                    ShortMessageUpdateNotify::MSG_ID,
                    &ShortMessageUpdateNotify {
                        Adds: vec![info_pb],
                        Reason: Some(ChangeReason::ActionFinish),
                        ..Default::default()
                    },
                );
            }
        }
    } else if kind == "DeletePhoneMessage" {
        if let Some(pmid) = json_int(itype.get("PhoneMessageId"))
            && pmid > 0
        {
            remove_int_value(&mut ctx.quest_comp.info.progress.read_phone_messages, pmid);
            let removed = if let Some(pc) = ctx.progress_comp.as_deref_mut() {
                let was_unlocked = pc.info.remove_claim(&format!("sm:unlock:{pmid}"));
                pc.info.mark_claim(&format!("sm:del:{pmid}"));
                let _ = pc.save(ctx.fs);
                was_unlocked
            } else {
                true
            };
            if removed {
                ctx.sink.push(
                    ShortMessageUpdateNotify::MSG_ID,
                    &ShortMessageUpdateNotify {
                        Removes: vec![pmid],
                        Reason: Some(ChangeReason::ActionFinish),
                        ..Default::default()
                    },
                );
            }
        }
    } else if let Some(tid) = json_int(itype.get("TelecomId")) {
        append_unique_int(&mut ctx.quest_comp.info.progress.received_telecoms, tid);
        if let Some(cb) = callbacks.on_phone_message {
            cb(ctx, 0, tid)?;
        }
    }
    Ok(())
}

fn apply_send_npc_mail<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    mail_id: i32,
) -> io::Result<()> {
    if mail_id <= 0 {
        return Ok(());
    }
    let now_sec = now_unix_secs();
    let pb_entry = if let Some(mail_comp) = ctx.mail_comp.as_deref_mut() {
        let entry_pb = mail_comp.info.upsert_npc_mail(mail_id, now_sec).to_proto();
        mail_comp.save(ctx.fs)?;
        entry_pb
    } else {
        sonoro_proto::pb::PbMailInfo {
            Id: mail_id.to_string().into_bytes(),
            ConfigId: mail_id,
            ReceivedTime: now_sec,
            State: 0,
            ValidTime: 2_592_000,
            ReadValidTime: 2_592_000,
            ExpiryTime: now_sec + 2_592_000,
            ..Default::default()
        }
    };
    ctx.sink.push(
        MailInfosNotify::MSG_ID,
        &MailInfosNotify {
            MailInfos: vec![pb_entry],
        },
    );
    if let Some(cb) = callbacks.on_npc_mail {
        cb(ctx, mail_id)?;
    }
    Ok(())
}

fn apply_enable_level_play<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(Value::Array(cfgs)) = params.get("Configs") else {
        return Ok(());
    };
    for entry in cfgs {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let Some(lpid) = json_int(obj.get("LevelPlayId")) else {
            continue;
        };
        let enable = json_bool(obj.get("Enable")).unwrap_or(true);
        if enable
            && let Some(lp) = ctx.assets.tables.level_play_data.get_data_by_id(&lpid)
            && lp.Data.LevelPlayEntityId > 0
        {
            ensure_quest_entity_spawned(ctx, lp.Data.LevelPlayEntityId);
        }
        record_progress_int(ctx, &format!("lp_state:{lpid}"), if enable { 2 } else { 0 }, false)?;
        ctx.sink.push(
            LevelPlayStateNotify::MSG_ID,
            &LevelPlayStateNotify {
                Id: lpid,
                State: if enable { 2 } else { 0 },
                CompleteNumber: 0,
            },
        );
        if let Some(cb) = callbacks.on_level_play_enabled {
            cb(ctx, lpid, enable)?;
        }
    }
    Ok(())
}

fn grant_action_items<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    items_opt: Option<&Value>,
) -> io::Result<()> {
    let Some(Value::Array(arr)) = items_opt else {
        return Ok(());
    };
    let mut list = Vec::new();
    for it in arr {
        let Some(obj) = it.as_object() else {
            continue;
        };
        let Some(iid) = json_int(obj.get("ItemId")) else {
            continue;
        };
        let cnt = json_int(obj.get("Count")).unwrap_or(1);
        if iid > 0 && cnt > 0 {
            list.push(RewardItem {
                item_id: iid,
                count: cnt,
            });
        }
    }
    if !list.is_empty() {
        {
            let mut rctx = ctx.make_reward_context();
            let _ = reward::grant_items(&mut rctx, 0, false, &list, 2)?;
        }
        if let Some(cb) = callbacks.on_item_gained {
            for it in &list {
                cb(ctx, it.item_id)?;
            }
        }
    }
    Ok(())
}

fn destroy_quest_items<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(iid) = json_int(params.get("ItemId")) else {
        return Ok(());
    };
    if iid <= 0 {
        return Ok(());
    }
    let is_all = json_bool(params.get("IsAll")).unwrap_or(false);
    let owned = ctx.inventory_comp.info.normal_item_count(iid);
    if owned <= 0 {
        return Ok(());
    }
    let to_take = if is_all {
        owned
    } else {
        owned.min(json_int(params.get("Count")).unwrap_or(1))
    };
    if to_take <= 0 {
        return Ok(());
    }
    if ctx.inventory_comp.info.consume_normal_item(iid, to_take) {
        let i_path = format!(
            "player/{}/{}",
            ctx.inventory_comp.player_id,
            InventoryInfo::DATA_PATH
        );
        comp_util::save_struct(ctx.fs, &ctx.inventory_comp.info, &i_path)?;
        ctx.sink.push(
            NormalItemUpdateNotify::MSG_ID,
            &NormalItemUpdateNotify {
                NormalItemList: vec![NormalItem {
                    Id: iid,
                    Count: ctx.inventory_comp.info.normal_item_count(iid),
                    ExpireTime: 0,
                }],
                NoTips: false,
            },
        );
    }
    Ok(())
}

fn apply_quest_buff_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
    is_player: bool,
    is_remove: bool,
) {
    let Some(Value::Array(buff_ids)) = params.get("BuffIds") else {
        return;
    };
    if buff_ids.is_empty() || ctx.scene.is_none() {
        return;
    }

    let targets = if is_player {
        let scene = ctx.scene.as_deref().expect("checked");
        let mut t: Vec<_> = scene
            .entities
            .iter()
            .enumerate()
            .filter(|(_, s)| s.config.entity_type == EntityType::Player)
            .map(|(index, s)| crate::events::EntityHandle {
                index,
                net_id: s.net_id,
            })
            .collect();
        if t.is_empty() {
            let role_nid = scene.current_role_entity_id();
            if let Some(handle) = scene.entity_handle(role_nid) {
                t.push(handle);
            }
        }
        t
    } else {
        let mut cfg_ids = Vec::new();
        if let Some(eid) = json_int(params.get("EntityId"))
            && eid > 0
        {
            cfg_ids.push(eid);
        }
        cfg_ids.extend(parse_int_list(params.get("EntityIds")));
        let mut t = Vec::new();
        for cfg_id in cfg_ids {
            if !is_remove {
                ensure_quest_entity_spawned(ctx, cfg_id);
            }
            let scene = ctx.scene.as_deref().expect("checked");
            if let Some((index, s)) = scene
                .entities
                .iter()
                .enumerate()
                .find(|(_, s)| s.config.config_id == cfg_id)
            {
                t.push(crate::events::EntityHandle {
                    index,
                    net_id: s.net_id,
                });
            }
        }
        t
    };

    for target_entity in targets {
        if is_remove {
            for v in buff_ids {
                if let Some(raw_id) = json_i64(Some(v))
                    && raw_id > 0
                {
                    ctx.events.enqueue(Event::BuffRemovalById {
                        entity: target_entity,
                        buff_id: raw_id,
                    });
                }
            }
        } else {
            let additions: Vec<BuffAdditionEntry> = buff_ids
                .iter()
                .filter_map(|v| json_i64(Some(v)))
                .filter(|&raw_id| raw_id > 0)
                .map(|raw_id| BuffAdditionEntry {
                    id: raw_id,
                    stack_count: 1,
                    is_active: true,
                    ..Default::default()
                })
                .collect();
            if !additions.is_empty() {
                ctx.events.enqueue(Event::BuffAddition {
                    target: target_entity,
                    instigator: target_entity,
                    buffs: additions,
                });
            }
        }
    }
}

fn apply_enable_system_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) {
    let Some(sys_id) = json_int(params.get("SystemType")) else {
        return;
    };
    let enabled = json_bool(params.get("IsEnable")).unwrap_or(true);
    if enabled {
        ctx.quest_comp.info.unlock_system(sys_id);
        push_func_open(ctx);
        return;
    }
    ctx.sink.push(
        sonoro_proto::pb::FuncOpenNotify::MSG_ID,
        &sonoro_proto::pb::FuncOpenNotify {
            Func: vec![sonoro_proto::pb::Function {
                Id: sys_id,
                Flag: 0,
            }],
        },
    );
}

fn apply_enable_function_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(ty) = json_str(params.get("Type")) else {
        return Ok(());
    };
    match ty {
        "InteractiveItems" => {
            let enable = json_bool(params.get("Enable")).unwrap_or(true);
            record_progress_int(ctx, "func:interactive_items", i64::from(enable), false)?;
        }
        "MotorRadioUI" => {
            let enable = if let Some(dm) = params.get("DisplayMode").and_then(|v| v.as_object()) {
                json_str(dm.get("Type")) == Some("EnableAll")
            } else {
                json_bool(params.get("Enable")).unwrap_or(true)
            };
            record_progress_int(ctx, "func:motor_radio_ui", i64::from(enable), false)?;
        }
        "TeleportDungeon" => {
            let enable = json_bool(params.get("Enable")).unwrap_or(true);
            let dlist = parse_int_list(params.get("DungeonList"));
            if dlist.is_empty() {
                record_progress_int(ctx, "func:tp_dungeon_all", i64::from(enable), false)?;
            } else {
                for did in dlist {
                    record_progress_int(ctx, &format!("func:tp_dungeon:{did}"), i64::from(enable), false)?;
                }
            }
        }
        _ => {
            if let Some(fid) = json_int(params.get("FunctionId")) {
                let enable = json_bool(params.get("Enable")).unwrap_or(true);
                let changed = if enable {
                    ctx.quest_comp.info.unlock_system(fid)
                } else {
                    remove_int_value(&mut ctx.quest_comp.info.unlocked_systems, fid)
                };
                if changed {
                    push_func_open(ctx);
                }
            }
        }
    }
    Ok(())
}

fn teleport_player_from_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
    action_id: i32,
    is_flow_action: bool,
) {
    let Some(inst_id) = ctx.scene.as_deref().map(|s| s.instance_id) else {
        return;
    };
    let mut pos_xyz = None;
    let mut rot_xyz = [0.0f32; 3];

    if let Some(tp_cfg) = params.get("TelePortConfig") {
        let Some(tp_obj) = tp_cfg.as_object() else {
            return;
        };
        let tp_type = json_int(tp_obj.get("Type")).unwrap_or(0);
        if tp_type == 0 {
            if let Some(tp) = tp_obj.get("TargetPos")
                && let Some(tp_map) = tp.as_object()
                && let Some(vec) = parse_vector_pos(Some(tp))
            {
                pos_xyz = Some(vec);
                rot_xyz[2] = json_float(tp_map.get("A")).unwrap_or(0.0);
            }
        } else if tp_type == 1 {
            if let Some(Value::Array(arr)) = tp_obj.get("EntityIds")
                && let Some(first) = arr.first()
                && let Some(eid) = json_i64(Some(first))
                && let Some(cfg) = ctx.assets.find_level_entity(inst_id, eid)
            {
                pos_xyz = asset_indexes::entity_world_pos(&cfg);
                rot_xyz = asset_indexes::entity_world_rot(&cfg);
            }
        } else if tp_type == 2
            && let Some(eid) = json_i64(tp_obj.get("GravityAndPosEntityId"))
            && let Some(cfg) = ctx.assets.find_level_entity(inst_id, eid)
        {
            pos_xyz = asset_indexes::entity_world_pos(&cfg);
            rot_xyz = asset_indexes::entity_world_rot(&cfg);
            let gres = gravity::resolve_entity_gravity(&cfg, rot_xyz);
            if gres.configured
                && let Some(scene) = ctx.scene.as_deref_mut()
            {
                let role_nid = scene.current_role_entity_id();
                if let Some(storage) = scene.entity_mut(role_nid) {
                    storage.world_item.gravity =
                        Some(crate::component::entity::small::GravityComponent {
                            location: gres.dir,
                        });
                }
            }
        }
    } else if let Some(pos_val) = params.get("Pos") {
        if let Some(pos_map) = pos_val.as_object()
            && let Some(vec) = parse_vector_pos(Some(pos_val))
        {
            pos_xyz = Some(vec);
            rot_xyz[2] = json_float(pos_map.get("A")).unwrap_or(0.0);
        }
    } else {
        return;
    }

    let Some(pos) = pos_xyz else {
        return;
    };
    if update_player_scene_state(ctx, pos, Some(rot_xyz)).is_err() {
        return;
    }

    let mut notify = TeleportNotify {
        MapId: ctx.assets.resolve_map_config_id(inst_id),
        Pos: Some(Vector {
            X: pos[0],
            Y: pos[1],
            Z: pos[2],
        }),
        Rot: Some(Vector {
            X: rot_xyz[0],
            Y: rot_xyz[1],
            Z: rot_xyz[2],
        }),
        Reason: Some(TeleportReason::Action),
        ..Default::default()
    };
    if is_flow_action
        && action_id > 0
        && let Some(scene) = ctx.scene.as_deref()
        && !scene.active_flow_list_name.is_empty()
    {
        notify.Reason = Some(TeleportReason::FlowAction);
        notify.GameCtx = Some(GameCtxPb {
            CtxType: Some(GameCtxType::FlowActionCtx),
            CtxInfo: Some(GameCtxPbCtxInfoOneof::FlowAction(FlowActionCtxPb {
                FlowListName: scene.active_flow_list_name.as_bytes().to_vec(),
                FlowId: scene.active_flow_id,
                StateId: scene.active_flow_state_id,
                ActionId: action_id,
            })),
        });
    }
    ctx.sink.push(TeleportNotify::MSG_ID, &notify);
}

fn emit_var_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &mut ActionCallbacks<'_, S>,
    owner_id: i32,
    var_name: &str,
    var_define: VarDefinePb,
    is_global: bool,
) -> io::Result<()> {
    if is_global {
        let mut entries = BTreeMap::new();
        entries.insert(var_name.as_bytes().to_vec(), var_define.clone());
        ctx.sink.push(
            PlayerVarNotify::MSG_ID,
            &PlayerVarNotify { VarInfos: entries },
        );
    }
    if owner_id > 0 {
        if let Some(deferred) = callbacks.deferred_bt_vars.as_deref_mut() {
            deferred.push(DeferredBtVar {
                owner_id,
                var_name: var_name.to_string(),
                var_define,
            });
        } else {
            ctx.sink.push(
                BtVarUpdateNotify::MSG_ID,
                &BtVarUpdateNotify {
                    TreeOwnerId: owner_id,
                    TreeIncId: i64::from(owner_id),
                    VarName: var_name.as_bytes().to_vec(),
                    VarDefine: Some(var_define),
                },
            );
        }
    }
    if let Some(cb) = callbacks.on_var_changed {
        cb(ctx, owner_id, var_name)?;
    }
    Ok(())
}

fn persist_global_var_int<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    val: i64,
) {
    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.info.set_key_int(&format!("var:{name}"), val);
        let _ = pc.save(ctx.fs);
    }
}

fn persist_global_var_str<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    val: &str,
) {
    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.info.set_key_str(&format!("var:{name}"), val);
        let _ = pc.save(ctx.fs);
    }
}

fn apply_set_var<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &mut ActionCallbacks<'_, S>,
    owner_id: i32,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(left) = params.get("VarLeft").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let Some(base_name) = resolve_var_name(left) else {
        return Ok(());
    };
    let Some(right_val) = params.get("VarRight") else {
        return Ok(());
    };
    let Some(right) = right_val.as_object() else {
        return Ok(());
    };

    let src = json_str(left.get("Source")).unwrap_or("Self");
    let is_global = src == "Global";
    let store_key = if !is_global {
        scoped_var_key_with_owner(ctx.scene.as_deref(), owner_id, left, base_name)
            .unwrap_or_else(|| base_name.to_string())
    } else {
        base_name.to_string()
    };

    if let Some(val) = right.get("Value") {
        match val {
            Value::Bool(b) => {
                ctx.quest_comp.info.set_var_bool(&store_key, *b);
                ctx.quest_comp.info.set_var_int(&store_key, i32::from(*b));
                if is_global {
                    persist_global_var_int(ctx, base_name, i64::from(*b));
                }
                emit_var_changed(
                    ctx,
                    callbacks,
                    owner_id,
                    base_name,
                    VarDefinePb {
                        VarType: 0,
                        Value: Some(VarDefinePbValueOneof::Boolean(*b)),
                    },
                    is_global,
                )?;
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    let iv = i.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                    ctx.quest_comp.info.set_var_int(&store_key, iv);
                    if is_global {
                        persist_global_var_int(ctx, base_name, i);
                    }
                    emit_var_changed(
                        ctx,
                        callbacks,
                        owner_id,
                        base_name,
                        VarDefinePb {
                            VarType: 1,
                            Value: Some(VarDefinePbValueOneof::Int(i)),
                        },
                        is_global,
                    )?;
                } else if let Some(f) = n.as_f64() {
                    let iv = f.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
                    ctx.quest_comp.info.set_var_int(&store_key, iv);
                    if is_global {
                        persist_global_var_int(ctx, base_name, (f * 1000.0) as i64);
                    }
                    emit_var_changed(
                        ctx,
                        callbacks,
                        owner_id,
                        base_name,
                        VarDefinePb {
                            VarType: 3,
                            Value: Some(VarDefinePbValueOneof::Float(f as f32)),
                        },
                        is_global,
                    )?;
                }
            }
            Value::String(s) => {
                ctx.quest_comp.info.set_var_str(&store_key, s);
                if is_global {
                    persist_global_var_str(ctx, base_name, s);
                }
                emit_var_changed(
                    ctx,
                    callbacks,
                    owner_id,
                    base_name,
                    VarDefinePb {
                        VarType: 2,
                        Value: Some(VarDefinePbValueOneof::String(s.as_bytes().to_vec())),
                    },
                    is_global,
                )?;
            }
            _ => {}
        }
    } else if let Some(src_name) = resolve_var_name(right) {
        let src_type = json_str(right.get("Type")).unwrap_or("");
        if src_type == "String" {
            let sk = scoped_var_key(right, src_name).unwrap_or_else(|| src_name.to_string());
            let s1 = ctx.quest_comp.info.get_var_str(&sk);
            let sv = if !s1.is_empty() {
                s1.to_string()
            } else {
                ctx.quest_comp.info.get_var_str(src_name).to_string()
            };
            ctx.quest_comp.info.set_var_str(&store_key, &sv);
            if is_global {
                persist_global_var_str(ctx, base_name, &sv);
            }
            emit_var_changed(
                ctx,
                callbacks,
                owner_id,
                base_name,
                VarDefinePb {
                    VarType: 2,
                    Value: Some(VarDefinePbValueOneof::String(sv.into_bytes())),
                },
                is_global,
            )?;
        } else {
            let iv = resolve_var_int_for_owner(ctx, owner_id, Some(right_val));
            ctx.quest_comp.info.set_var_int(&store_key, iv);
            if is_global {
                persist_global_var_int(ctx, base_name, i64::from(iv));
            }
            emit_var_changed(
                ctx,
                callbacks,
                owner_id,
                base_name,
                VarDefinePb {
                    VarType: 1,
                    Value: Some(VarDefinePbValueOneof::Int(i64::from(iv))),
                },
                is_global,
            )?;
        }
    }
    Ok(())
}

fn apply_calculate_var<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &mut ActionCallbacks<'_, S>,
    owner_id: i32,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(res_obj) = params.get("Result").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let Some(base_name) = resolve_var_name(res_obj) else {
        return Ok(());
    };
    let src = json_str(res_obj.get("Source")).unwrap_or("Self");
    let is_global = src == "Global";
    let store_key = if !is_global {
        scoped_var_key_with_owner(ctx.scene.as_deref(), owner_id, res_obj, base_name)
            .unwrap_or_else(|| base_name.to_string())
    } else {
        base_name.to_string()
    };

    let v1 = resolve_var_int_for_owner(ctx, owner_id, params.get("Var1"));
    let v2 = resolve_var_int_for_owner(ctx, owner_id, params.get("Var2"));
    let op = json_str(params.get("Op")).unwrap_or("Add");

    let out = match op {
        "Add" => v1.saturating_add(v2),
        "Sub" => v1.saturating_sub(v2),
        "Mul" | "Mut" => v1.saturating_mul(v2),
        "Div" if v2 != 0 => v1.saturating_div(v2),
        _ => v1,
    };

    ctx.quest_comp.info.set_var_int(&store_key, out);
    if is_global {
        persist_global_var_int(ctx, base_name, i64::from(out));
    }
    emit_var_changed(
        ctx,
        callbacks,
        owner_id,
        base_name,
        VarDefinePb {
            VarType: 1,
            Value: Some(VarDefinePbValueOneof::Int(i64::from(out))),
        },
        is_global,
    )
}

/// Clears any persisted hidden state and guarantees the quest/flow entity is spawned in the active scene.
pub fn ensure_quest_entity_awakened<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_config_id: i32,
) {
    if entity_config_id <= 0 {
        return;
    }
    append_unique_int(&mut ctx.quest_comp.info.world.shown_entities, entity_config_id);
    remove_int_value(&mut ctx.quest_comp.info.world.hidden_entities, entity_config_id);
    ensure_quest_entity_spawned(ctx, entity_config_id);
}

fn apply_awake_entity<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let mut ids = Vec::new();
    if let Some(eid) = json_int(params.get("EntityId"))
        && eid > 0
    {
        ids.push(eid);
    }
    for eid in parse_int_list(params.get("EntityIds")) {
        if eid > 0 && !ids.contains(&eid) {
            ids.push(eid);
        }
    }
    for &eid in &ids {
        ensure_quest_entity_awakened(ctx, eid);
    }
    if !ids.is_empty() {
        send_quest_entity_visibility(ctx, &ids, true)?;
    }
    Ok(())
}

pub fn ensure_quest_entity_spawned<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_config_id: i32,
) {
    if ctx.scene.is_none() || entity_config_id <= 0 {
        return;
    }
    if ctx.quest_comp.info.has_opened_chest(entity_config_id) {
        return;
    }
    if ctx
        .quest_comp
        .info
        .world
        .hidden_entities
        .contains(&entity_config_id)
    {
        return;
    }
    {
        let scene = ctx.scene.as_deref().expect("checked");
        if let Some(entity_cfg) = ctx
            .assets
            .find_level_entity(scene.instance_id, i64::from(entity_config_id))
            && !entity_data_layers_active(
                ctx.assets,
                &ctx.quest_comp.info,
                scene.instance_id,
                &entity_cfg,
            )
        {
            return;
        }
        if scene
            .entities
            .iter()
            .any(|s| s.config.config_id == entity_config_id)
        {
            return;
        }
    }

    {
        let scene = ctx.scene.as_deref_mut().expect("checked");
        prologue_quest::spawn_quest_entity(
            ctx.events,
            scene,
            ctx.fs,
            ctx.assets,
            ctx.sink,
            ctx.basic_comp,
            i64::from(entity_config_id),
        );
    }

    let nid_opt = ctx
        .scene
        .as_deref()
        .and_then(|s| s.find_entity_net_id_by_config_id(entity_config_id));
    let Some(nid) = nid_opt else {
        return;
    };

    if let Some(pc) = ctx.progress_comp.as_deref() {
        let xr = pc.info.get_key_int(&format!("world_pos:{entity_config_id}:x"));
        let yr = pc.info.get_key_int(&format!("world_pos:{entity_config_id}:y"));
        let zr = pc.info.get_key_int(&format!("world_pos:{entity_config_id}:z"));
        let cval = pc.info.get_key_int(&format!("world_camp:{entity_config_id}"));
        let scene = ctx.scene.as_deref_mut().expect("checked");
        if xr != 0 || yr != 0 || zr != 0 {
            let fx = f32::from_bits(xr as u32);
            let fy = f32::from_bits(yr as u32);
            let fz = f32::from_bits(zr as u32);
            let _ = scene.set_entity_position(nid, [fx, fy, fz], None);
        }
        if cval > 0
            && let Some(storage) = scene.entity_mut(nid)
            && let Ok(c32) = i32::try_from(cval)
        {
            storage.config.camp = c32;
        }
    }

    if let Some(saved_state) = ctx
        .quest_comp
        .info
        .get_entity_state(i64::from(entity_config_id))
        && !saved_state.is_empty()
    {
        let upd_opt = ctx.scene.as_deref_mut().and_then(|scene| {
            scene.update_entity_state_tag(&ctx.assets.tables.gameplay_tag_remap, nid, saved_state)
        });
        if let Some(upd) = upd_opt
            && upd.old_tag_id != upd.new_tag_id
        {
            let scene = ctx.scene.as_deref().expect("checked");
            emit_entity_state_tag_update_checked(scene, Some(ctx.assets), ctx.sink, nid, upd);
        }
    }

    if let Some(scene) = ctx.scene.as_deref_mut() {
        let cfg64 = i64::from(entity_config_id);
        if scene.join_scene_pushed {
            let _ = crate::helpers::quest_engine::sync_dynamic_interact_options_for_entity(
                scene,
                ctx.assets,
                ctx.sink,
                &ctx.quest_comp.info,
                ctx.progress_comp.as_deref(),
                cfg64,
            );
        } else {
            let _ = crate::helpers::quest_engine::register_dynamic_interact_options_for_entity(
                scene,
                ctx.assets,
                &ctx.quest_comp.info,
                ctx.progress_comp.as_deref(),
                cfg64,
            );
        }
    }
}

fn apply_destroy_entity<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let mut ids = Vec::new();
    if let Some(eid) = json_int(params.get("EntityId"))
        && eid > 0
    {
        ids.push(eid);
    }
    for eid in parse_int_list(params.get("EntityIds")) {
        if eid > 0 && !ids.contains(&eid) {
            ids.push(eid);
        }
    }
    destroy_config_ids(ctx, &ids)
}

fn apply_destroy_self<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) -> io::Result<()> {
    let Some(scene) = ctx.scene.as_deref() else {
        return Ok(());
    };
    let owner_nid = scene.active_flow_owner_entity_id;
    if owner_nid == 0 {
        return Ok(());
    }
    if let Some(cfg_id) = scene.find_config_id_by_net_id(owner_nid)
        && cfg_id > 0
    {
        return destroy_config_ids(ctx, &[cfg_id]);
    }
    let scene = ctx.scene.as_deref_mut().expect("checked");
    let _ = scene.remove(ctx.fs, owner_nid);
    ctx.sink.push(
        EntityRemoveNotify::MSG_ID,
        &EntityRemoveNotify {
            RemoveInfos: vec![EntityRemoveInfo {
                EntityId: owner_nid,
                Type: Some(ERemoveEntityType::RemoveTypeNormal),
            }],
            IsRemove: true,
        },
    );
    Ok(())
}

fn resolve_action_entity_cfg_id(scene_opt: Option<&Scene>, params: &Map<String, Value>) -> Option<i32> {
    let is_self = json_bool(params.get("IsSelf")).unwrap_or(false);
    let param_eid = json_int(params.get("EntityId")).unwrap_or(0);
    if !is_self && param_eid > 0 {
        return Some(param_eid);
    }
    if let Some(scene) = scene_opt
        && scene.active_flow_owner_entity_id != 0
        && let Some(oc) = scene.find_config_id_by_net_id(scene.active_flow_owner_entity_id)
        && oc > 0
    {
        return Some(oc);
    }
    if param_eid > 0 { Some(param_eid) } else { None }
}

/// The client has no LevelEvent for EnableNearbyTracking; it only sets
/// SceneItemNearbyTrackingComponent.EnableTracking from EnableNearbyTrackingNotify
/// (LevelGamePlayController.OnEnableNearbyTrackingNotify). `ControlType` Other lists config ids;
/// Self (or no ControlType) is the entity that owns the action.
fn apply_enable_nearby_tracking<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    params: &Map<String, Value>,
) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    let is_enable = json_bool(params.get("IsEnable")).unwrap_or(true);
    let control = params.get("ControlType").and_then(Value::as_object);
    let net_ids: Vec<i64> = match control.and_then(|c| json_str(c.get("Type"))) {
        None | Some("Self") => nearby_tracking_self_net_id(scene, owner_id).into_iter().collect(),
        Some("Other") => parse_int_list(control.and_then(|c| c.get("Entities")))
            .into_iter()
            .filter_map(|cfg_id| scene.find_entity_net_id_by_config_id(cfg_id))
            .collect(),
        Some(_) => Vec::new(),
    };
    if net_ids.is_empty() {
        return;
    }
    for &nid in &net_ids {
        if let Some(storage) = scene.entity_mut(nid)
            && storage.world_item.nearby_tracking_enable.is_some()
        {
            storage.world_item.nearby_tracking_enable = Some(is_enable);
        }
    }
    ctx.sink.push(
        EnableNearbyTrackingNotify::MSG_ID,
        &EnableNearbyTrackingNotify {
            EntityIds: net_ids,
            IsEnable: is_enable,
        },
    );
}

/// Entity-owned action lists pass the owner config id as `owner_id` (StateChangeBehaviors,
/// interact options) and/or set `active_flow_owner_entity_id` (interact, trigger, listener).
fn nearby_tracking_self_net_id(scene: &Scene, owner_id: i32) -> Option<i64> {
    let flow_owner = scene.active_flow_owner_entity_id;
    if owner_id > 0 {
        if flow_owner != 0 && scene.find_config_id_by_net_id(flow_owner) == Some(owner_id) {
            return Some(flow_owner);
        }
        if let Some(nid) = scene.find_entity_net_id_by_config_id(owner_id) {
            return Some(nid);
        }
    }
    (flow_owner != 0 && scene.entity(flow_owner).is_some()).then_some(flow_owner)
}

fn apply_toggle_elemental_ability<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(());
    };
    let enable = json_bool(params.get("Enable")).unwrap_or(true);
    let cfg_id = resolve_action_entity_cfg_id(Some(scene), params).unwrap_or(0);
    let net_id = if cfg_id > 0 {
        scene.find_entity_net_id_by_config_id(cfg_id)
    } else if scene.active_flow_owner_entity_id != 0 {
        Some(scene.active_flow_owner_entity_id)
    } else {
        None
    };
    if let Some(nid) = net_id
        && let Some(storage) = scene.entity_mut(nid)
    {
        storage.world_item.elemental_holder_enable = Some(enable);
    }
    if cfg_id > 0 {
        record_progress_int(ctx, &format!("elem_ability:{cfg_id}"), i64::from(enable), false)?;
    }
    Ok(())
}

fn apply_interact_collect_or_reward<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(inst_id) = ctx.scene.as_deref().map(|s| s.instance_id) else {
        return Ok(());
    };
    let cfg_id = resolve_action_entity_cfg_id(ctx.scene.as_deref(), params).unwrap_or(0);
    let is_collect = name == "Collect";
    let is_sun_spirit = name == "CollectSunSpirit";
    let is_lp_reward = name == "ClaimLevelPlayReward";
    let is_dyn_reward = name == "ClaimDynamicEntityReward";

    let mut reward_id = 0i32;
    let mut tb_type = 0i32;
    let mut has_refresh = false;
    let mut completed_lp_id = None;

    if cfg_id > 0 {
        if let Some(lcfg) = ctx.assets.find_level_entity(inst_id, i64::from(cfg_id)) {
            let mut comps = lcfg.ComponentsData.clone();
            if let Some(tpl) = ctx.assets.find_template_config(&lcfg.BlueprintType) {
                tpl.ComponentsData.merge_into(&mut comps);
            }
            if let Some(rc) = &comps.RewardComponent {
                reward_id = rc.RewardId.unwrap_or(0);
            }
            has_refresh = comps.RefreshComponent.is_some();
            if let Some(tb) = &comps.TreasureBoxComponent {
                tb_type = tb.TypeId.unwrap_or(0);
            }
        }
        if is_lp_reward {
            let scene_map = ctx.assets.resolve_map_config_id(inst_id);
            for &lp_idx in ctx
                .assets
                .indexes
                .level_play_indices_for_reward_entity(cfg_id)
            {
                let lp = &ctx.assets.tables.level_play_data.items[lp_idx as usize];
                let lp_src = if lp.Data.InstanceId != 0 {
                    lp.Data.InstanceId
                } else {
                    lp.Data.LevelId
                };
                let lp_map = ctx.assets.resolve_map_config_id(lp_src);
                if lp.Data.InstanceId != inst_id
                    && lp.Data.LevelId != inst_id
                    && lp_map != scene_map
                {
                    continue;
                }
                if let Some(rc) = &lp.Data.LevelPlayRewardConfig
                    && reward_id == 0
                    && rc.RewardId > 0
                {
                    reward_id = rc.RewardId;
                }
                completed_lp_id = Some(lp.LevelPlayId);
                break;
            }
        }
    }

    let now_ms = now_millis();
    let already_taken = cfg_id > 0
        && (ctx.quest_comp.info.has_opened_chest(cfg_id)
            || ctx
                .scene
                .as_deref()
                .expect("checked")
                .instance
                .is_on_respawn_cooldown(cfg_id, now_ms));

    if !already_taken {
        if is_lp_reward || tb_type > 0 {
            let mut rctx = ctx.make_reward_context();
            let _ = reward::grant_treasure_box_reward(&mut rctx, tb_type, reward_id, 7000)?;
        } else if reward_id > 0 {
            let mut rctx = ctx.make_reward_context();
            let res = reward::grant_drop_package(&mut rctx, reward_id, 1)?;
            if res.granted_count == 0 && is_collect {
                let fallback = [RewardItem {
                    item_id: 2,
                    count: 150,
                }];
                let _ = reward::grant_items(&mut rctx, 0, false, &fallback, 1)?;
            }
        } else if is_collect || is_sun_spirit {
            let mut rctx = ctx.make_reward_context();
            let fallback = [RewardItem {
                item_id: 2,
                count: 150,
            }];
            let _ = reward::grant_items(&mut rctx, 0, false, &fallback, 1)?;
        }

        if cfg_id > 0 {
            if is_sun_spirit {
                record_progress_claim(ctx, &format!("sun_spirit:{cfg_id}"))?;
                ctx.quest_comp.info.mark_chest_opened(cfg_id);
            } else if is_lp_reward || tb_type > 0 || !has_refresh {
                ctx.quest_comp.info.mark_chest_opened(cfg_id);
            } else if let Some(scene) = ctx.scene.as_deref_mut() {
                scene
                    .instance
                    .set_respawn_cooldown(cfg_id, now_ms, now_ms + 300_000);
                let _ = scene.save_persistent(ctx.fs);
            }
            if is_dyn_reward {
                record_progress_claim(ctx, &format!("dyn_rw:{cfg_id}"))?;
            }
        }
    }

    if let Some(lp_id) = completed_lp_id {
        remove_int_value(&mut ctx.quest_comp.info.progress.entered_level_plays, lp_id);
        ctx.quest_comp.info.mark_level_play_completed(lp_id);
        record_progress_int(ctx, &format!("lp_state:{lp_id}"), 3, false)?;
    }

    if is_lp_reward && cfg_id > 0 {
        apply_entity_state_change(ctx, callbacks, cfg_id, "关卡.Common.状态.激活")?;
    }
    if is_collect && has_refresh && cfg_id > 0 {
        // Repeatable collectibles with RefreshComponent rely on respawn cooldown;
        // do not persist them in permanent world.hidden_entities.
        if let Some(scene) = ctx.scene.as_deref_mut() {
            let mut to_remove = Vec::new();
            for storage in &scene.entities {
                if storage.config.config_id == cfg_id {
                    to_remove.push(storage.net_id);
                }
            }
            if !to_remove.is_empty() {
                let mut remove_infos = Vec::with_capacity(to_remove.len());
                for net_id in to_remove {
                    scene.remove(ctx.fs, net_id)?;
                    remove_infos.push(EntityRemoveInfo {
                        EntityId: net_id,
                        Type: Some(ERemoveEntityType::RemoveTypeNormal),
                    });
                }
                ctx.sink.push(
                    EntityRemoveNotify::MSG_ID,
                    &EntityRemoveNotify {
                        RemoveInfos: remove_infos,
                        IsRemove: true,
                    },
                );
            }
        }
    } else if is_collect || is_sun_spirit {
        apply_destroy_self(ctx)?;
    }
    Ok(())
}

fn apply_reset_point_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    is_activate: bool,
) -> io::Result<()> {
    let Some(scene) = ctx.scene.as_deref() else {
        return Ok(());
    };
    let inst_id = scene.instance_id;
    if is_activate {
        let empty = Map::new();
        let cfg_id = resolve_action_entity_cfg_id(Some(scene), &empty).unwrap_or(0);
        let mut pos_opt = None;
        if cfg_id > 0
            && let Some(lcfg) = ctx.assets.find_level_entity(inst_id, i64::from(cfg_id))
        {
            pos_opt = asset_indexes::entity_world_pos(&lcfg);
        }
        if pos_opt.is_none() && !scene.instance.players.is_empty() {
            pos_opt = Some(scene.instance.players[0].location);
        }
        record_progress_int(ctx, "reset_point:cfg", i64::from(cfg_id), false)?;
        if let Some(pos) = pos_opt {
            record_progress_int(ctx, "reset_point:x", i64::from(pos[0].to_bits() as i32), false)?;
            record_progress_int(ctx, "reset_point:y", i64::from(pos[1].to_bits() as i32), false)?;
            record_progress_int(ctx, "reset_point:z", i64::from(pos[2].to_bits() as i32), false)?;
        }
        return Ok(());
    }

    let mut target_pos = None;
    if let Some(pc) = ctx.progress_comp.as_deref() {
        let cfg_id = i32::try_from(pc.info.get_key_int("reset_point:cfg")).unwrap_or(0);
        let xb = pc.info.get_key_int("reset_point:x");
        let yb = pc.info.get_key_int("reset_point:y");
        let zb = pc.info.get_key_int("reset_point:z");
        if xb != 0 || yb != 0 || zb != 0 {
            target_pos = Some([
                f32::from_bits(xb as u32),
                f32::from_bits(yb as u32),
                f32::from_bits(zb as u32),
            ]);
        } else if cfg_id > 0
            && let Some(lcfg) = ctx.assets.find_level_entity(inst_id, i64::from(cfg_id))
        {
            target_pos = asset_indexes::entity_world_pos(&lcfg);
        }
    }
    if let Some(pos) = target_pos {
        update_player_scene_state(ctx, pos, None)?;
        ctx.sink.push(
            TeleportNotify::MSG_ID,
            &TeleportNotify {
                MapId: inst_id,
                Pos: Some(Vector {
                    X: pos[0],
                    Y: pos[1],
                    Z: pos[2],
                }),
                Reason: Some(TeleportReason::Action),
                ..Default::default()
            },
        );
    }
    Ok(())
}

fn apply_reset_level_play_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let mut dirty_q = false;
    for lpid in parse_int_list(params.get("ResetLevelPlayList")) {
        if lpid <= 0 {
            continue;
        }
        if remove_int_value(&mut ctx.quest_comp.info.completed_level_plays, lpid) {
            dirty_q = true;
        }
        if remove_int_value(&mut ctx.quest_comp.info.progress.entered_level_plays, lpid) {
            dirty_q = true;
        }
        record_progress_int(ctx, &format!("lp_state:{lpid}"), 0, false)?;
    }
    if dirty_q {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

fn resolve_spawn_anchor<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    pos_eid: i32,
) -> ([f32; 3], [f32; 3]) {
    if let Some(scene) = ctx.scene.as_deref() {
        if pos_eid > 0 {
            if let Some(nid) = scene.find_entity_net_id_by_config_id(pos_eid)
                && let Some(pos) = scene.entity(nid).and_then(|s| s.position)
            {
                return (pos.location, pos.rotation);
            }
            if let Some(cfg) = ctx
                .assets
                .find_level_entity(scene.instance_id, i64::from(pos_eid))
                && let Some(loc) = asset_indexes::entity_world_pos(&cfg)
            {
                return (loc, asset_indexes::entity_world_rot(&cfg));
            }
        }
        if let Some(p) = scene.instance.players.first() {
            return (p.location, p.rotation);
        }
    }
    ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0])
}

fn despawn_tracked_net_ids<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    net_ids: Vec<i64>,
) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    let mut infos = Vec::new();
    for nid in net_ids {
        if scene.remove(ctx.fs, nid).is_ok() {
            infos.push(EntityRemoveInfo {
                EntityId: nid,
                Type: Some(ERemoveEntityType::RemoveTypeNormal),
            });
        }
    }
    if !infos.is_empty() {
        ctx.sink.push(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                RemoveInfos: infos,
                IsRemove: true,
            },
        );
    }
}

fn apply_summon_entity_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(cfg_val) = params.get("SummonEntityConfig").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let Some(tpl_id) = json_str(cfg_val.get("TemplateId")) else {
        return Ok(());
    };
    let pos_eid = json_int(cfg_val.get("PositionEntityId")).unwrap_or(0);
    let (loc, rot) = resolve_spawn_anchor(ctx, pos_eid);
    let (player_nid, spawned_nid) = {
        let Some(scene) = ctx.scene.as_deref_mut() else {
            return record_progress_int(
                ctx,
                &format!("summon:{tpl_id}"),
                if pos_eid > 0 { i64::from(pos_eid) } else { 1 },
                true,
            );
        };
        let pn = scene.current_role_entity_id();
        let nid = crate::commands::entity::spawn_synthetic_template_entity(
            scene,
            ctx.fs,
            ctx.assets,
            ctx.sink,
            ctx.basic_comp,
            tpl_id,
            i64::from(pos_eid),
            loc,
            rot,
        );
        if let Some(id) = nid {
            scene
                .summoned_entities
                .entry(tpl_id.to_string())
                .or_default()
                .push(id);
        }
        (pn, nid)
    };
    if let Some(id) = spawned_nid {
        ctx.sink.push(
            SummonEntityNotify::MSG_ID,
            &SummonEntityNotify {
                SummonerId: player_nid,
                SummonIds: vec![id],
            },
        );
    }
    record_progress_int(
        ctx,
        &format!("summon:{tpl_id}"),
        if pos_eid > 0 { i64::from(pos_eid) } else { 1 },
        true,
    )
}

fn apply_destroy_summoned_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(cfg_val) = params
        .get("DestroyEntitiesConfig")
        .and_then(|v| v.as_object())
    else {
        return Ok(());
    };
    if let Some(tpl_id) = json_str(cfg_val.get("TemplateId")) {
        let removed = ctx
            .scene
            .as_deref_mut()
            .and_then(|s| s.summoned_entities.remove(tpl_id))
            .unwrap_or_default();
        despawn_tracked_net_ids(ctx, removed);
        return record_progress_int(ctx, &format!("summon:{tpl_id}"), 0, false);
    }
    if name == "DestroyDynamicEntities" {
        let dtype = json_str(cfg_val.get("Type")).unwrap_or("dynamic");
        let all_summoned = ctx
            .scene
            .as_deref_mut()
            .map(|s| s.summoned_entities.drain().flat_map(|(_, v)| v).collect())
            .unwrap_or_default();
        despawn_tracked_net_ids(ctx, all_summoned);
        return record_progress_int(ctx, &format!("dyn_ent:{dtype}"), 0, false);
    }
    Ok(())
}

fn apply_prefab_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let pname = params
        .get("VarName")
        .and_then(Value::as_object)
        .and_then(|vn| json_str(vn.get("Name")))
        .unwrap_or("prefab");
    if name == "DestroyPrefab" {
        let removed = ctx
            .scene
            .as_deref_mut()
            .and_then(|s| s.prefab_entities.remove(pname))
            .unwrap_or_default();
        despawn_tracked_net_ids(ctx, removed);
        return record_progress_int(ctx, &format!("prefab:{pname}"), 0, false);
    }

    let pos_eid = json_int(params.get("PosEntityId")).unwrap_or(0);
    let prefab_id = params
        .get("Config")
        .and_then(Value::as_object)
        .and_then(|c| json_int(c.get("PrefabId")))
        .unwrap_or(0);
    let (anchor_loc, anchor_rot) = resolve_spawn_anchor(ctx, pos_eid);
    if prefab_id > 0
        && let Some(prefab) = ctx
            .assets
            .tables
            .prefab_config
            .get_data_by_id(&prefab_id)
            .map(|p| p.into_owned())
    {
        let mut spawned = Vec::new();
        if let Some(scene) = ctx.scene.as_deref_mut() {
            for ent in &prefab.Entities {
                let Some(ed) = ent.get("EntityData").and_then(Value::as_object) else {
                    continue;
                };
                let Some(bp) = json_str(ed.get("BlueprintType")) else {
                    continue;
                };
                let cfg_id = json_i64(ed.get("Id")).unwrap_or(0);
                let tf = ed.get("Transform").and_then(Value::as_object);
                let pos = tf
                    .and_then(|t| t.get("Pos"))
                    .and_then(Value::as_object)
                    .map_or(anchor_loc, |p| {
                        [
                            anchor_loc[0] + json_float(p.get("X")).unwrap_or(0.0),
                            anchor_loc[1] + json_float(p.get("Y")).unwrap_or(0.0),
                            anchor_loc[2] + json_float(p.get("Z")).unwrap_or(0.0),
                        ]
                    });
                let rot = tf
                    .and_then(|t| t.get("Rot"))
                    .and_then(Value::as_object)
                    .map_or(anchor_rot, |r| {
                        [
                            json_float(r.get("X")).unwrap_or(0.0),
                            json_float(r.get("Y")).unwrap_or(0.0),
                            json_float(r.get("Z")).unwrap_or(0.0),
                        ]
                    });
                if let Some(nid) = crate::commands::entity::spawn_synthetic_template_entity(
                    scene,
                    ctx.fs,
                    ctx.assets,
                    ctx.sink,
                    ctx.basic_comp,
                    bp,
                    cfg_id,
                    pos,
                    rot,
                ) {
                    spawned.push(nid);
                }
            }
            if !spawned.is_empty() {
                scene
                    .prefab_entities
                    .entry(pname.to_string())
                    .or_default()
                    .extend(spawned);
            }
        }
    }
    record_progress_int(ctx, &format!("prefab:{pname}"), 1, false)
}

fn destroy_config_ids<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    ids: &[i32],
) -> io::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    for &target_cfg in ids {
        append_unique_int(&mut ctx.quest_comp.info.world.hidden_entities, target_cfg);
        remove_int_value(&mut ctx.quest_comp.info.world.shown_entities, target_cfg);
    }
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(());
    };
    let mut to_remove = Vec::new();
    for &target_cfg in ids {
        for storage in &scene.entities {
            if storage.config.config_id == target_cfg {
                to_remove.push(storage.net_id);
            }
        }
    }
    if to_remove.is_empty() {
        return Ok(());
    }
    let mut remove_infos = Vec::with_capacity(to_remove.len());
    for net_id in to_remove {
        scene.remove(ctx.fs, net_id)?;
        remove_infos.push(EntityRemoveInfo {
            EntityId: net_id,
            Type: Some(ERemoveEntityType::RemoveTypeNormal),
        });
    }
    ctx.sink.push(
        EntityRemoveNotify::MSG_ID,
        &EntityRemoveNotify {
            RemoveInfos: remove_infos,
            IsRemove: true,
        },
    );
    Ok(())
}

fn parse_vector_pos(val_opt: Option<&Value>) -> Option<[f32; 3]> {
    let v = val_opt?.as_object()?;
    let x = json_float(v.get("X"))?;
    let y = json_float(v.get("Y"))?;
    let z = json_float(v.get("Z")).unwrap_or(0.0);
    Some([x, y, z])
}

fn push_entity_moved<S: PushSink + ?Sized>(
    sink: &mut S,
    net_id: i64,
    pos: [f32; 3],
    yaw_opt: Option<f32>,
) {
    sink.push(
        MovePackageNotify::MSG_ID,
        &MovePackageNotify {
            MovingEntities: vec![MovingEntityData {
                EntityId: net_id,
                Originator: net_id,
                MoveInfos: vec![MoveReplaySample {
                    Location: Some(Vector {
                        X: pos[0],
                        Y: pos[1],
                        Z: pos[2],
                    }),
                    Rotation: yaw_opt.map(|yaw| Rotator {
                        Pitch: 0.0,
                        Yaw: yaw,
                        Roll: 0.0,
                    }),
                    ..Default::default()
                }],
                ForcePush: true,
            }],
        },
    );
}

fn push_spline_status<S: PushSink + ?Sized>(
    sink: &mut S,
    net_id: i64,
    status: MoveStatus,
    spline_entity_id: i32,
    runtime_data: Option<SceneItemSplineRuntimeData>,
    move_spline_config: Option<MoveSplineConfig>,
) {
    sink.push(
        MoveSplineStatusNotify::MSG_ID,
        &MoveSplineStatusNotify {
            SceneItemRuntimeData: runtime_data,
            MoveStatus: Some(status),
            EntityId: net_id,
            SplineEntityId: spline_entity_id,
            MoveSplineConfig: move_spline_config,
        },
    );
}

fn move_spline_config_from_action(params: &Map<String, Value>) -> Option<MoveSplineConfig> {
    let mut config = MoveSplineConfig::default();
    let mut has_config = false;
    if let Some(index) = json_int(params.get("StartPointIndex")) {
        config.StartPoint = Some(MoveSplineConfigStartPointOneof::StartPointIndex(index));
        has_config = true;
    }
    if let Some(index) = json_int(params.get("EndPointIndex")) {
        config.EndPoint = Some(MoveSplineConfigEndPointOneof::EndPointIndex(index));
        has_config = true;
    }
    let look_dir = json_bool(params.get("IsLookDir")).or_else(|| {
        params
            .get("MoveTarget")
            .and_then(|v| v.as_object())
            .and_then(|o| json_bool(o.get("IsLookDir")))
    });
    if let Some(enabled) = look_dir {
        config.LookDir = Some(MoveSplineConfigLookDirOneof::IsLookDir(enabled));
        has_config = true;
    }
    has_config.then_some(config)
}

fn apply_set_audio_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    params: &Map<String, Value>,
) {
    let Some(cfg) = params.get("AudioConfig").and_then(|v| v.as_object()) else {
        return;
    };
    let Some(group) = json_str(cfg.get("Group")) else {
        return;
    };
    let Some(state) = json_str(cfg.get("State")) else {
        return;
    };
    ctx.quest_comp.info.set_audio_state(group, state);
    ctx.sink.push(
        AudioStateUpdateNotify::MSG_ID,
        &AudioStateUpdateNotify {
            AudioStates: vec![AudioStatePb {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                Group: group.as_bytes().to_vec(),
                State: state.as_bytes().to_vec(),
            }],
        },
    );
}

fn apply_add_play_bubble<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
    guid: &str,
) {
    if guid.is_empty() {
        return;
    }
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    for cfg_id in parse_int_list(params.get("EntityIds")) {
        if cfg_id <= 0 {
            continue;
        }
        scene.add_play_bubble(cfg_id, guid);
        if let Some(nid) = scene.find_entity_net_id_by_config_id(cfg_id) {
            ctx.sink.push(
                EntityAddBubbleNotify::MSG_ID,
                &EntityAddBubbleNotify {
                    EntityId: nid,
                    ActionGuid: guid.as_bytes().to_vec(),
                },
            );
        }
    }
}

fn apply_clear_play_bubble<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    for cfg_id in parse_int_list(params.get("EntityIds")) {
        if cfg_id <= 0 {
            continue;
        }
        let removed = scene.take_play_bubbles_for_entity(cfg_id);
        let net_id = scene.find_entity_net_id_by_config_id(cfg_id);
        for rec in removed {
            if let Some(nid) = net_id {
                ctx.sink.push(
                    EntityRemoveBubbleNotify::MSG_ID,
                    &EntityRemoveBubbleNotify {
                        EntityId: nid,
                        ActionGuid: rec.action_guid.into_bytes(),
                    },
                );
            }
        }
    }
}

fn apply_add_flow_interact_option<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    params: &Map<String, Value>,
) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    let Some(cfg_id) = json_int(params.get("EntityId")) else {
        return;
    };
    if cfg_id <= 0 {
        return;
    }
    let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id) else {
        return;
    };
    let Some(opt_val) = params.get("Option").and_then(|v| v.as_object()) else {
        return;
    };
    let Some(guid) = json_str(opt_val.get("Guid")) else {
        return;
    };
    if guid.is_empty() {
        return;
    }
    let text = json_str(opt_val.get("TidContent")).unwrap_or("");

    let mut kind = DynamicInteractKind::None;
    let mut flow_name = String::new();
    let mut flow_id = 0;
    let mut state_id = 1;
    let mut actions_json = String::new();

    if let Some(ty_val) = opt_val.get("Type").and_then(|v| v.as_object()) {
        let ty_name = json_str(ty_val.get("Type")).unwrap_or("");
        if ty_name == "Flow" {
            if let Some(fv) = ty_val.get("Flow").and_then(|v| v.as_object()) {
                flow_name = json_str(fv.get("FlowListName")).unwrap_or("").to_string();
                flow_id = json_int(fv.get("FlowId")).unwrap_or(0);
                state_id = json_int(fv.get("StateId")).unwrap_or(1);
                if !flow_name.is_empty() && flow_id > 0 {
                    kind = DynamicInteractKind::Flow;
                }
            }
        } else if ty_name == "Actions" {
            kind = DynamicInteractKind::Actions;
            if let Some(Value::Array(av)) = ty_val.get("Actions")
                && !av.is_empty()
            {
                actions_json = serde_json::to_string(av).unwrap_or_else(|_| "[]".to_string());
            } else {
                actions_json = "[]".to_string();
            }
        }
    }
    if kind == DynamicInteractKind::None {
        kind = DynamicInteractKind::Actions;
        actions_json = "[]".to_string();
    }

    scene.upsert_dynamic_interact_option(
        guid,
        net_id,
        cfg_id,
        quest_id,
        node_id,
        kind,
        &flow_name,
        flow_id,
        state_id,
        &actions_json,
        text,
    );

    let game_ctx = if quest_id > 0 {
        GameCtxPb {
            CtxType: Some(GameCtxType::BehaviorTree),
            CtxInfo: Some(GameCtxPbCtxInfoOneof::BehaviorTree(BehaviorTreeCtxPb {
                IncId: i64::from(quest_id),
                BtType: Some(BtType::BtTypeQuest),
                BtId: quest_id,
                NodeId: node_id,
            })),
        }
    } else {
        GameCtxPb {
            CtxType: Some(GameCtxType::Entity),
            CtxInfo: Some(GameCtxPbCtxInfoOneof::Entity(EntityCtxPb {
                ConfigId: cfg_id,
                IncId: net_id,
            })),
        }
    };

    ctx.sink.push(
        EntityAddDynamicInteractNotify::MSG_ID,
        &EntityAddDynamicInteractNotify {
            GameCtx: Some(game_ctx),
            EntityId: net_id,
            DelayRemove: false,
            Text: text.as_bytes().to_vec(),
            OptionGuid: guid.as_bytes().to_vec(),
        },
    );
}

fn apply_remove_flow_interact_option<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    let cfg_id = json_int(params.get("EntityId")).unwrap_or(0);
    let mut flow_name_opt = None;
    let mut flow_id_opt = None;
    if let Some(fv) = params.get("Flow").and_then(|v| v.as_object()) {
        flow_name_opt = json_str(fv.get("FlowListName"));
        flow_id_opt = json_int(fv.get("FlowId"));
    }
    let removed = scene.take_matching_dynamic_interact_options(cfg_id, flow_name_opt, flow_id_opt);
    for opt in removed {
        ctx.sink.push(
            EntityRemoveDynamicInteractNotify::MSG_ID,
            &EntityRemoveDynamicInteractNotify {
                OptionGuid: opt.option_guid.into_bytes(),
                EntityId: opt.entity_net_id,
            },
        );
    }
}

fn apply_set_entity_pos<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(cfg_id) = json_int(params.get("EntityId")) else {
        return Ok(());
    };
    let Some(pos) = parse_vector_pos(params.get("Pos")) else {
        return Ok(());
    };
    let yaw_opt = params
        .get("Pos")
        .and_then(|v| v.as_object())
        .and_then(|o| json_float(o.get("A")));
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(());
    };
    let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id) else {
        return Ok(());
    };
    if !scene.set_entity_position(net_id, pos, yaw_opt) {
        return Ok(());
    }
    push_entity_moved(ctx.sink, net_id, pos, yaw_opt);
    let _ = persist_quest_entity_position(ctx, cfg_id, net_id, pos);
    Ok(())
}

fn apply_reset_entity_pos<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(scene) = ctx.scene.as_deref() else {
        return Ok(());
    };
    let Some(cfg_id) = json_int(params.get("EntityId")) else {
        return Ok(());
    };
    let Some(cfg) = ctx
        .assets
        .find_level_entity(scene.instance_id, i64::from(cfg_id))
    else {
        return Ok(());
    };
    let Some(pos) = asset_indexes::entity_world_pos(&cfg) else {
        return Ok(());
    };
    if let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id) {
        push_entity_moved(ctx.sink, net_id, pos, None);
        let _ = persist_quest_entity_position(ctx, cfg_id, net_id, pos);
    }
    ctx.sink.push(
        EntityPosResetNotify::MSG_ID,
        &EntityPosResetNotify { EntityId: cfg_id },
    );
    Ok(())
}

fn apply_change_entity_camp<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    if ctx.scene.is_none() {
        return Ok(());
    }
    let Some(cfg_id) = json_int(params.get("TargetEntity"))
        .or_else(|| json_int(params.get("EntityId")))
    else {
        return Ok(());
    };
    let Some(camp) = json_int(params.get("Camp")) else {
        return Ok(());
    };
    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.info
            .set_key_int(&format!("world_camp:{cfg_id}"), i64::from(camp));
        pc.save(ctx.fs)?;
    }
    let scene = ctx.scene.as_deref_mut().expect("checked");
    let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id) else {
        return Ok(());
    };
    if let Some(storage) = scene.entity_mut(net_id) {
        storage.config.camp = camp;
    }
    ctx.sink.push(
        ModifyEntityCampNotify::MSG_ID,
        &ModifyEntityCampNotify {
            TargetEntityId: net_id,
            Camp: camp,
        },
    );
    Ok(())
}

fn apply_guest_character<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
    add: bool,
) {
    let Some(gid) = json_int(params.get("GuestCharacterId"))
        .or_else(|| json_int(params.get("GuestId")))
    else {
        return;
    };
    if gid <= 0 {
        return;
    }
    if add {
        append_unique_int(&mut ctx.quest_comp.info.world.active_guests, gid);
        ctx.sink
            .push(AddGuestNotify::MSG_ID, &AddGuestNotify { GuestId: gid });
    } else {
        remove_int_value(&mut ctx.quest_comp.info.world.active_guests, gid);
        ctx.sink.push(
            RemoveGuestNotify::MSG_ID,
            &RemoveGuestNotify { GuestId: gid },
        );
    }
}

fn apply_update_trial_character<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let ids = collect_trial_role_ids(params);
    if ids.is_empty() && ctx.quest_comp.info.env.trial_roles.is_empty() {
        return Ok(());
    }
    let target_ids = if !ids.is_empty() {
        ids
    } else {
        ctx.quest_comp.info.env.trial_roles.clone()
    };
    let auto_change = json_bool(params.get("AutoChange")).unwrap_or(false);
    let enable_map = json_bool(params.get("EnableMapAndTeleport")).unwrap_or(true);
    let dungeon_ids = parse_int_list(params.get("DungeonList"));
    apply_trial_roles_with_options(ctx, &target_ids, enable_map, false, auto_change, &dungeon_ids)
}

fn apply_exec_resurrection<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let revive_id = json_int(params.get("ReviveId")).unwrap_or(1);
    record_progress_add(ctx, "combat:revive_cnt", 1)?;
    if let Some(scene) = ctx.scene.as_deref() {
        ctx.sink.push(
            PlayerReviveNotify::MSG_ID,
            &PlayerReviveNotify {
                PlayerId: scene.player_id,
                ReviveType: revive_id,
                TeleportId: scene.instance_id,
                ..Default::default()
            },
        );
    }
    Ok(())
}

fn apply_guide_trigger<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(gid) = json_int(params.get("GuideId")) else {
        return Ok(());
    };
    if gid <= 0 {
        return Ok(());
    }
    if let Some(cb) = callbacks.complete_guide {
        return cb(ctx, gid);
    }
    if let Some(gc) = ctx.guide_comp.as_deref_mut() {
        gc.info.add_finished(gid);
    }
    Ok(())
}

fn apply_finish_dungeon<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let is_success = json_bool(params.get("IsSuccess")).unwrap_or(true);
    let inst_id = ctx.scene.as_deref().map_or(0, |s| s.instance_id);
    if is_success && inst_id > 0 {
        ctx.quest_comp.info.mark_dungeon_completed(inst_id);
        ctx.quest_comp.save(ctx.fs)?;
        record_progress_add(ctx, "inst:finish_cnt", 1)?;
    }
    if inst_id > 0 {
        ctx.sink.push(
            InstResultNotify::MSG_ID,
            &InstResultNotify {
                Id: inst_id,
                Succ: is_success,
                IsRecReward: is_success,
                Reason: 1,
            },
        );
    }
    Ok(())
}

fn apply_settlement_dungeon<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let inst_id = ctx.scene.as_deref().map_or(0, |s| s.instance_id);
    if inst_id > 0 {
        ctx.quest_comp.info.mark_dungeon_completed(inst_id);
        ctx.quest_comp.save(ctx.fs)?;
    }
    ctx.sink.push(
        InstSettleNotify::MSG_ID,
        &InstSettleNotify {
            IsSuccess: true,
            Magnification: 1,
            ..Default::default()
        },
    );
    Ok(())
}

fn apply_timer_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let is_pause = if name == "ToggleTimerPauseState" {
        json_bool(params.get("IsPause")).unwrap_or(true)
    } else {
        false
    };
    let timer_ty = json_str(params.get("TimerType")).unwrap_or("Default");
    record_progress_int(
        ctx,
        &format!("inst:timer:{timer_ty}:paused"),
        i64::from(is_pause),
        false,
    )?;
    let status = if is_pause { 2 } else { 1 };
    let mut timers = BTreeMap::new();
    timers.insert(
        1,
        InstanceTimer {
            Status: status,
            RemainTick: 60,
        },
    );
    ctx.sink.push(
        InstanceTimerNotify::MSG_ID,
        &InstanceTimerNotify {
            InstanceTimers: timers,
        },
    );
    Ok(())
}

fn record_progress_add<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    key: &str,
    delta: i64,
) -> io::Result<i64> {
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return Ok(0);
    };
    let next = pc.info.add_key_int(key, delta);
    pc.save(ctx.fs)?;
    Ok(next)
}

fn record_progress_int<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    key: &str,
    val: i64,
    also_claim: bool,
) -> io::Result<()> {
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return Ok(());
    };
    pc.info.set_key_int(key, val);
    if also_claim {
        pc.info.mark_claim(key);
    }
    pc.save(ctx.fs)
}

fn record_progress_claim<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    key: &str,
) -> io::Result<()> {
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return Ok(());
    };
    if pc.info.mark_claim(key) {
        pc.save(ctx.fs)?;
    }
    Ok(())
}

fn apply_random_var<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &mut ActionCallbacks<'_, S>,
    owner_id: i32,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(rv) = params
        .get("Result")
        .or_else(|| params.get("Var"))
        .and_then(|v| v.as_object())
    else {
        return Ok(());
    };
    let Some(base_name) = resolve_var_name(rv) else {
        return Ok(());
    };
    let src = json_str(rv.get("Source")).unwrap_or("Self");
    let is_global = src == "Global";
    let store_key = if !is_global {
        scoped_var_key_with_owner(ctx.scene.as_deref(), owner_id, rv, base_name)
            .unwrap_or_else(|| base_name.to_string())
    } else {
        base_name.to_string()
    };
    let max_val = params
        .get("RightVar")
        .and_then(|rw| rw.as_object())
        .and_then(|obj| json_int(obj.get("Value")))
        .unwrap_or(1)
        .max(1);
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u64, |d| d.as_nanos() as u64);
    let chosen = (now_ns % (max_val as u64)) as i32;
    ctx.quest_comp.info.set_var_int(&store_key, chosen);
    if is_global {
        persist_global_var_int(ctx, base_name, i64::from(chosen));
    }
    emit_var_changed(
        ctx,
        callbacks,
        owner_id,
        base_name,
        VarDefinePb {
            VarType: 1,
            Value: Some(VarDefinePbValueOneof::Int(i64::from(chosen))),
        },
        is_global,
    )
}

fn apply_reset_entity_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    let cfg_obj = params
        .get("ResetEntityConfig")
        .and_then(|rc| rc.as_object())
        .unwrap_or(params);
    for cfg_id in parse_int_list(cfg_obj.get("EntityIds")) {
        if let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id) {
            scene.remove_combine_relations_for_entity(net_id);
            if let Some(ecfg) = ctx
                .assets
                .find_level_entity(scene.instance_id, i64::from(cfg_id))
                && let Some(pos) = asset_indexes::entity_world_pos(&ecfg)
            {
                let _ = scene.set_entity_position(net_id, pos, None);
            }
        }
    }
}

fn apply_change_other_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    callbacks: &ActionCallbacks<'_, S>,
    quest_id: i32,
    node_id: i32,
    params: &Map<String, Value>,
) -> io::Result<()> {
    if let Some(cb) = callbacks.start_flow_state
        && let Some(sv) = params.get("State").and_then(|v| v.as_object())
    {
        let fname = json_str(sv.get("FlowListName")).unwrap_or("");
        let fid = json_int(sv.get("FlowId")).unwrap_or(0);
        let sid = json_int(sv.get("StateId")).unwrap_or(1);
        if !fname.is_empty() && fid > 0 {
            cb(ctx, quest_id, node_id, fname, fid, sid, true)?;
        }
    }
    Ok(())
}

fn wake_entity_fsm(scene: &mut Scene, cfg_id: i32, reason: u16) {
    if let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id) {
        scene.mark_fsm_dirty(net_id, reason);
    }
}

fn apply_entity_gameplay_tag_change<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    net_id: i64,
    tag_ids: &[i32],
    is_add: bool,
) {
    if tag_ids.is_empty() {
        return;
    }
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    if scene.entity(net_id).is_none() {
        return;
    }

    let mut notify_tags = Vec::new();
    if let Some(storage) = scene.entity_mut(net_id) {
        for &tid in tag_ids {
            if tid == 0 {
                continue;
            }
            if is_add {
                let tags = storage.tag.get_or_insert_with(TagComponent::default);
                if !tags.entity_common_tags.contains(&tid) {
                    tags.entity_common_tags.push(tid);
                }
                let cur = tags.tag_count(tid);
                tags.add_tag(tid, 1 - cur);
            } else if let Some(tags) = storage.tag.as_mut() {
                tags.entity_common_tags.retain(|&t| t != tid);
                tags.remove_tag(tid);
            }
            notify_tags.push(CommonTagData {
                TagId: tid,
                RemoveTagIds: is_add,
            });
        }
    }

    if !notify_tags.is_empty()
        && tags_handler::has_client_level_tag_component(scene, Some(ctx.assets), net_id)
    {
        ctx.sink.push(
            EntityCommonTagNotify::MSG_ID,
            &EntityCommonTagNotify {
                Id: net_id,
                Tags: notify_tags,
            },
        );
    }
    scene.mark_fsm_dirty(net_id, wake_reason::TAG);
}

fn apply_set_battle_state_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) {
    if ctx.scene.is_none() {
        return;
    }
    if let Some(so) = params.get("StateOption").and_then(|v| v.as_object())
        && json_str(so.get("Type")) == Some("SetBattleTag")
        && let Some(Value::Array(arr)) = so.get("SetTags")
    {
        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let cfg_id = json_int(obj.get("EntityId")).unwrap_or(0);
            if cfg_id <= 0 {
                continue;
            }
            let set_type = json_str(obj.get("SetType")).unwrap_or("Add");
            let is_add = set_type != "Remove";
            let Some(tag_name) = json_str(obj.get("GameplayTag")) else {
                continue;
            };
            let Some(tid) = gameplay_tags::id_from_name(&ctx.assets.tables.gameplay_tag_remap, tag_name)
            else {
                continue;
            };
            if is_add {
                ensure_quest_entity_spawned(ctx, cfg_id);
            }
            if let Some(net_id) = ctx
                .scene
                .as_deref()
                .and_then(|s| s.find_entity_net_id_by_config_id(cfg_id))
            {
                apply_entity_gameplay_tag_change(ctx, net_id, &[tid], is_add);
            }
        }
        return;
    }
    let in_battle = json_bool(params.get("InBattle"))
        .or_else(|| json_bool(params.get("IsInBattle")))
        .unwrap_or(true);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.force_battle_state(in_battle);
    }
}

fn apply_exec_battle_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(bo_val) = params.get("BattleOption") else {
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.force_battle_state(true);
        }
        return Ok(());
    };
    let Some(bo) = bo_val.as_object() else {
        return Ok(());
    };
    let Some(btype) = json_str(bo.get("Type")) else {
        return Ok(());
    };
    let Some(inst_id) = ctx.scene.as_deref().map(|s| s.instance_id) else {
        return Ok(());
    };

    match btype {
        "SetBattleTags" => {
            if let Some(Value::Array(cfgs)) = bo.get("Configs") {
                for entry in cfgs {
                    let Some(obj) = entry.as_object() else {
                        continue;
                    };
                    let cfg_id = json_int(obj.get("EntityId")).unwrap_or(0);
                    let tid = json_int(obj.get("TagConfigId")).unwrap_or(0);
                    if cfg_id <= 0 || tid == 0 {
                        continue;
                    }
                    ensure_quest_entity_spawned(ctx, cfg_id);
                    if let Some(net_id) = ctx
                        .scene
                        .as_deref()
                        .and_then(|s| s.find_entity_net_id_by_config_id(cfg_id))
                    {
                        apply_entity_gameplay_tag_change(ctx, net_id, &[tid], true);
                    }
                }
            }
        }
        "SetMonsterMoveTarget" => {
            let target_eid = json_int(bo.get("TargetEntityId")).unwrap_or(0);
            let mut target_pos = None;
            if target_eid > 0 {
                if let Some(scene) = ctx.scene.as_deref()
                    && let Some(tnid) = scene.find_entity_net_id_by_config_id(target_eid)
                    && let Some(storage) = scene.entity(tnid)
                    && let Some(tp) = storage.position
                {
                    target_pos = Some(tp.location);
                }
                if target_pos.is_none()
                    && let Some(lcfg) = ctx.assets.find_level_entity(inst_id, i64::from(target_eid))
                {
                    target_pos = asset_indexes::entity_world_pos(&lcfg);
                }
            }
            for mid in parse_int_list(bo.get("MonsterEntityIds")) {
                if mid <= 0 {
                    continue;
                }
                ensure_quest_entity_spawned(ctx, mid);
                if let Some(pos) = target_pos
                    && let Some(mnid) = ctx
                        .scene
                        .as_deref()
                        .and_then(|s| s.find_entity_net_id_by_config_id(mid))
                {
                    persist_quest_entity_position(ctx, mid, mnid, pos)?;
                    push_entity_moved(ctx.sink, mnid, pos, None);
                }
                if let Some(scene) = ctx.scene.as_deref_mut() {
                    wake_entity_fsm(scene, mid, wake_reason::EVENT);
                }
            }
        }
        "SendBattleEvents" => {
            if let Some(Value::Array(cfgs)) = bo.get("Configs") {
                for entry in cfgs {
                    let Some(obj) = entry.as_object() else {
                        continue;
                    };
                    if let Some(cfg_id) = json_int(obj.get("EntityId"))
                        && cfg_id > 0
                        && let Some(scene) = ctx.scene.as_deref_mut()
                    {
                        wake_entity_fsm(scene, cfg_id, wake_reason::EVENT);
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply_exec_alert_system_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(opt) = params.get("Option").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let Some(atype) = json_str(opt.get("Type")) else {
        return Ok(());
    };
    let Some(area_id) = json_int(opt.get("AreaId")) else {
        return Ok(());
    };
    if area_id <= 0 {
        return Ok(());
    }

    let k_enable = format!("alert_enable:{area_id}");
    let k_ui = format!("alert_ui:{area_id}");
    let k_vis = format!("alert_vis:{area_id}");
    let k_val = format!("alert_val:{area_id}");

    let mut remove_ids = Vec::new();
    let mut areas = Vec::new();

    match atype {
        "EnableAlertArea" => {
            let enable = json_bool(opt.get("IsEnable")).unwrap_or(true);
            record_progress_int(ctx, &k_enable, i64::from(enable), false)?;
            if !enable {
                remove_ids.push(area_id);
            } else {
                let pc = ctx.progress_comp.as_deref();
                areas.push(AlertArea {
                    AreaId: area_id,
                    AlertValue: pc.map_or(0.0, |p| p.info.get_key_int(&k_val) as f32),
                    EnableUi: pc.is_some_and(|p| p.info.get_key_int(&k_ui) != 0),
                    VisibleUi: pc.is_some_and(|p| p.info.get_key_int(&k_vis) != 0),
                    ..Default::default()
                });
            }
        }
        "EnableAlertUi" => {
            let enable = json_bool(opt.get("IsEnable")).unwrap_or(true);
            record_progress_int(ctx, &k_ui, i64::from(enable), false)?;
            let pc = ctx.progress_comp.as_deref();
            areas.push(AlertArea {
                AreaId: area_id,
                AlertValue: pc.map_or(0.0, |p| p.info.get_key_int(&k_val) as f32),
                EnableUi: enable,
                VisibleUi: pc.map_or(enable, |p| p.info.get_key_int(&k_vis) != 0),
                ..Default::default()
            });
        }
        "SetAlertUiVisible" => {
            let vis = json_bool(opt.get("IsVisible")).unwrap_or(true);
            record_progress_int(ctx, &k_vis, i64::from(vis), false)?;
            let pc = ctx.progress_comp.as_deref();
            areas.push(AlertArea {
                AreaId: area_id,
                AlertValue: pc.map_or(0.0, |p| p.info.get_key_int(&k_val) as f32),
                EnableUi: pc.is_none_or(|p| p.info.get_key_int(&k_ui) != 0),
                VisibleUi: vis,
                ..Default::default()
            });
        }
        "ModifyAlertValue" => {
            let val = json_int(opt.get("AlertValue")).unwrap_or(0);
            record_progress_int(ctx, &k_val, i64::from(val), false)?;
            let pc = ctx.progress_comp.as_deref();
            areas.push(AlertArea {
                AreaId: area_id,
                AlertValue: val as f32,
                EnableUi: pc.is_none_or(|p| p.info.get_key_int(&k_ui) != 0),
                VisibleUi: pc.is_none_or(|p| p.info.get_key_int(&k_vis) != 0),
                ..Default::default()
            });
        }
        _ => {}
    }

    if !areas.is_empty() || !remove_ids.is_empty() {
        ctx.sink.push(
            AlertAreaNotify::MSG_ID,
            &AlertAreaNotify {
                AlertAreas: areas,
                RemoveAreaIds: remove_ids,
            },
        );
    }
    Ok(())
}

fn apply_elemental_box_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(target) = params.get("Target").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    let ttype = json_str(target.get("Type")).unwrap_or("");
    if name == "SetElementalBoxToTarget" {
        let Some(box_id) = json_i64(params.get("ElementalBoxId")) else {
            return Ok(());
        };
        let k = if ttype == "Target" {
            let Some(eid) = json_i64(target.get("EntityId")) else {
                return Ok(());
            };
            format!("elemental_box:ent:{eid}")
        } else {
            "elemental_box:player".to_string()
        };
        record_progress_int(ctx, &k, box_id, false)?;
    } else if name == "ChangeElementalBoxState" {
        let Some(state_str) = json_str(params.get("State")) else {
            return Ok(());
        };
        if ttype == "TargetBoxId" {
            let Some(box_id) = json_int(target.get("TargetBoxId")) else {
                return Ok(());
            };
            if box_id <= 0 {
                return Ok(());
            }
            ctx.quest_comp
                .info
                .set_entity_state(i64::from(box_id), state_str);
            ctx.quest_comp.save(ctx.fs)?;
            let upd_opt = ctx.scene.as_deref_mut().and_then(|scene| {
                let nid = scene.find_entity_net_id_by_config_id(box_id)?;
                scene
                    .update_entity_state_tag(&ctx.assets.tables.gameplay_tag_remap, nid, state_str)
                    .map(|upd| (nid, upd))
            });
            if let Some((nid, upd)) = upd_opt {
                emit_entity_state_tags(ctx, nid, upd);
            }
        } else {
            let box_id = ctx
                .progress_comp
                .as_deref()
                .map_or(0, |pc| pc.info.get_key_int("elemental_box:player"));
            record_progress_int(ctx, &format!("elemental_box:state:{box_id}"), 1, false)?;
            ctx.quest_comp
                .info
                .set_var_str("elemental_box_state", state_str);
        }
    }
    Ok(())
}

fn apply_ai_control_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    for cfg_id in parse_int_list(params.get("EntityIds")) {
        if cfg_id <= 0 {
            continue;
        }
        match name {
            "EnableHostility" => {
                let en = json_bool(params.get("IsEnable")).unwrap_or(true);
                record_progress_int(ctx, &format!("hostility:{cfg_id}"), i64::from(en), false)?;
                if let Some(scene) = ctx.scene.as_deref_mut() {
                    wake_entity_fsm(scene, cfg_id, wake_reason::HATE);
                }
            }
            "EnableAI" => {
                let en = json_bool(params.get("IsEnable")).unwrap_or(true);
                record_progress_int(ctx, &format!("ai_enabled:{cfg_id}"), i64::from(en), false)?;
                if let Some(scene) = ctx.scene.as_deref_mut()
                    && let Some(nid) = scene.find_entity_net_id_by_config_id(cfg_id)
                {
                    if !en {
                        scene.set_battle_entity_active(nid, false);
                    }
                    scene.mark_fsm_dirty(nid, wake_reason::INITIAL);
                }
            }
            "OverrideAiBehaviorTree" => {
                let bt_id = i64::from(json_int(params.get("BehaviorTree")).unwrap_or(0));
                let bkey = json_str(params.get("Key")).unwrap_or("default");
                record_progress_int(ctx, &format!("ai_bt:{cfg_id}:{bkey}"), bt_id, false)?;
                record_progress_int(ctx, &format!("ai_bt_cur:{cfg_id}"), bt_id, false)?;
                if let Some(scene) = ctx.scene.as_deref_mut() {
                    wake_entity_fsm(scene, cfg_id, wake_reason::BLACKBOARD);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn apply_occupation_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    is_force: bool,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let otype = json_str(params.get("OccupationType")).unwrap_or("Request");
    let is_req = otype != "Release";
    let inc_id = i64::from(quest_id);
    let mut occ_names = Vec::new();
    if let Some(Value::Array(arr)) = params.get("Occupations") {
        for item in arr {
            let Some(occ_name) = json_str(Some(item)) else {
                continue;
            };
            occ_names.push(occ_name.to_string());
            record_progress_int(ctx, &format!("occ:{occ_name}"), i64::from(is_req), false)?;
        }
    }
    if let Some(scene) = ctx.scene.as_deref_mut() {
        if is_req {
            for occ_name in &occ_names {
                if is_force
                    && let Some(prev) = scene
                        .bt_occupations
                        .iter()
                        .find(|o| o.resource_name == *occ_name && (o.owner_id != quest_id || o.inc_id != inc_id))
                        .cloned()
                {
                    ctx.sink.push(
                        BtSuspendNotify::MSG_ID,
                        &BtSuspendNotify {
                            NodeId: prev.node_id,
                            SuspendType: SuspendType::Occupation as i32,
                            OccupationInfo: vec![OccupationPbInfo {
                                ResourceName: occ_name.as_bytes().to_vec(),
                                NodeId: node_id,
                                IncId: inc_id,
                            }],
                            TreeOwnerId: prev.owner_id,
                            TreeIncId: prev.inc_id,
                            IsScreenOccupy: false,
                        },
                    );
                }
                scene.upsert_occupation(
                    BtType::BtTypeQuest,
                    quest_id,
                    inc_id,
                    node_id,
                    occ_name,
                );
            }
        } else if !occ_names.is_empty() {
            scene
                .bt_occupations
                .retain(|o| !occ_names.iter().any(|n| n == &o.resource_name));
        } else if quest_id > 0 {
            scene.release_occupations_for_node(quest_id, node_id);
        }
    }
    record_progress_int(ctx, "world:occupation", i64::from(is_req), false)
}

fn apply_set_npc_group_perform<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let pkey = json_str(params.get("Key")).unwrap_or("");
    let Some(pt) = params.get("PerformType").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    if !pkey.is_empty() {
        record_progress_int(ctx, &format!("npc:group_perform:{pkey}"), 1, false)?;
    }
    if let Some(tv) = pt.get("Type") {
        match tv {
            Value::String(pt_type) => {
                if pt_type == "SupportedWalking" {
                    record_progress_int(ctx, "npc:supported_walking", 1, false)?;
                } else if pt_type == "CancelSupportedWalking" {
                    record_progress_int(ctx, "npc:supported_walking", 0, false)?;
                }
            }
            Value::Number(n) => {
                if let Some(pt_id) = n.as_i64() {
                    if pt_id == 0 {
                        if let Some(role_id) = ctx
                            .scene
                            .as_deref()
                            .map(|s| s.current_role_entity_id())
                            .filter(|&id| id > 0)
                        {
                            record_progress_int(ctx, &format!("hold_hand:{role_id}"), 1, false)?;
                        }
                    } else if pt_id == 5 || pt_id == 6 {
                        record_progress_int(ctx, "npc:supported_walking", 1, false)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn apply_modify_scene_tag_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    params: &Map<String, Value>,
) -> io::Result<()> {
    if ctx.scene.is_none() {
        return Ok(());
    }
    let ids = parse_int_list(params.get("EntityIds"));
    let tags = parse_int_list(params.get("PerformanceTag"));
    if tags.is_empty() {
        return Ok(());
    }
    let is_add = json_bool(params.get("IsAddTag")).unwrap_or(true);
    for cfg_id in ids {
        for &t in &tags {
            record_progress_int(ctx, &format!("tag_ent:{cfg_id}:{t}"), i64::from(is_add), false)?;
        }
        if is_add {
            ensure_quest_entity_spawned(ctx, cfg_id);
        }
        let Some(net_id) = ctx
            .scene
            .as_deref()
            .and_then(|s| s.find_entity_net_id_by_config_id(cfg_id))
        else {
            continue;
        };
        apply_entity_gameplay_tag_change(ctx, net_id, &tags, is_add);
        if is_add {
            let upd = ctx
                .scene
                .as_deref_mut()
                .and_then(|s| s.update_entity_state_tag_id(net_id, tags[0]));
            if let Some(upd) = upd {
                emit_entity_state_tags(ctx, net_id, upd);
            }
        } else if let Some(scene) = ctx.scene.as_deref_mut() {
            let cur_state = scene
                .entity(net_id)
                .and_then(|s| s.state_tag.map(|st| st.state_tag_id));
            if let Some(st_id) = cur_state
                && tags.contains(&st_id)
            {
                let _ = scene.update_entity_state_tag_id(net_id, 0);
            }
        }
    }
    Ok(())
}

fn resolve_quest_spline_end_pos(
    assets: &Assets,
    instance_id: i32,
    spline_cfg_id: i64,
) -> Option<[f32; 3]> {
    let lcfg = assets.find_level_entity(instance_id, spline_cfg_id)?;
    let base = asset_indexes::entity_world_pos(&lcfg).unwrap_or([0.0, 0.0, 0.0]);
    let rot = asset_indexes::entity_world_rot(&lcfg);
    let mut components = lcfg.ComponentsData.clone();
    if let Some(tpl) = assets.find_template_config(&lcfg.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut components);
    }
    let spline = components.SplineComponent?;
    let opt = spline.Option?.as_object().cloned()?;
    let pts = opt.get("Points")?.as_array()?;
    let last = pts.last()?.as_object()?;
    let off = gravity::rotate_local_offset_ue(parse_vector_pos(last.get("Position"))?, rot);
    Some([base[0] + off[0], base[1] + off[1], base[2] + off[2]])
}

fn apply_server_move_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    let Some(inst_id) = ctx.scene.as_deref().map(|s| s.instance_id) else {
        return Ok(());
    };
    match name {
        "MoveSceneItem" => {
            let Some(cfg_id) = json_int(params.get("EntityId")) else {
                return Ok(());
            };
            let Some(mc) = params.get("MoveConfig").and_then(|v| v.as_object()) else {
                return Ok(());
            };
            let Some(pos) = parse_vector_pos(mc.get("Point")) else {
                return Ok(());
            };
            let move_time = json_float(mc.get("Time")).unwrap_or(0.0);
            ensure_quest_entity_spawned(ctx, cfg_id);
            if let Some(net_id) = ctx
                .scene
                .as_deref()
                .and_then(|s| s.find_entity_net_id_by_config_id(cfg_id))
            {
                persist_quest_entity_position(ctx, cfg_id, net_id, pos)?;
                if move_time > 0.0 {
                    ctx.sink.push(
                        SceneItemMoveTargetNotify::MSG_ID,
                        &SceneItemMoveTargetNotify {
                            MoveInfo: Some(SceneItemMoveTargetInfo {
                                EntityId: net_id,
                                Location: Some(Vector {
                                    X: pos[0],
                                    Y: pos[1],
                                    Z: pos[2],
                                }),
                                MoveTime: move_time,
                                ..Default::default()
                            }),
                        },
                    );
                } else {
                    push_entity_moved(ctx.sink, net_id, pos, None);
                }
            }
        }
        "NpcMoveToPoint" => {
            let Some(cfg_id) =
                parse_quest_entity_id(params.get("Target").or_else(|| params.get("EntityId")))
            else {
                return Ok(());
            };
            let mut pos_opt =
                parse_vector_pos(params.get("Pos")).or_else(|| parse_vector_pos(params.get("TargetPos")));
            if pos_opt.is_none()
                && let Some(Value::Array(pts)) = params.get("Points")
                && let Some(last) = pts.last()
            {
                pos_opt = last
                    .as_object()
                    .and_then(|o| parse_vector_pos(o.get("Position").or(Some(last))));
            }
            let Some(pos) = pos_opt else {
                return Ok(());
            };
            ensure_quest_entity_spawned(ctx, cfg_id);
            if let Some(net_id) = ctx
                .scene
                .as_deref()
                .and_then(|s| s.find_entity_net_id_by_config_id(cfg_id))
            {
                persist_quest_entity_position(ctx, cfg_id, net_id, pos)?;
                push_entity_moved(ctx.sink, net_id, pos, None);
            }
        }
        "CharacterMoveToPoint" => {
            let Some(cfg_id) = parse_quest_entity_id(params.get("Target")) else {
                return Ok(());
            };
            let Some(pos) = parse_vector_pos(params.get("Pos")) else {
                return Ok(());
            };
            ensure_quest_entity_spawned(ctx, cfg_id);
            if let Some(net_id) = ctx
                .scene
                .as_deref()
                .and_then(|s| s.find_entity_net_id_by_config_id(cfg_id))
            {
                persist_quest_entity_position(ctx, cfg_id, net_id, pos)?;
                push_entity_moved(ctx.sink, net_id, pos, None);
            }
        }
        "MoveWithSpline" | "NewMoveWithSpline" => {
            let mut target = parse_quest_entity_id(params.get("MoveTarget")).unwrap_or(0);
            if target <= 0
                && let Some(scene) = ctx.scene.as_deref()
                && scene.active_flow_owner_entity_id != 0
            {
                target = scene
                    .find_config_id_by_net_id(scene.active_flow_owner_entity_id)
                    .unwrap_or(0);
            }
            let Some(spline_id) = json_int(params.get("SplineEntityId")) else {
                return Ok(());
            };
            if target <= 0 || spline_id <= 0 {
                return Ok(());
            }
            ensure_quest_entity_spawned(ctx, target);
            let move_spline_config = move_spline_config_from_action(params);
            let net_id_opt = ctx.scene.as_deref_mut().and_then(|scene| {
                let net_id = scene.find_entity_net_id_by_config_id(target)?;
                let storage = scene.entity_mut(net_id)?;
                storage.world_item.move_spline = Some(MoveSplineComponent {
                    move_status: Some(MoveStatus::Moving),
                    spline_entity_id: spline_id,
                    runtime_data: None,
                    move_spline_config: move_spline_config.clone(),
                });
                Some(net_id)
            });
            if let Some(net_id) = net_id_opt {
                record_progress_int(ctx, &format!("quest_spline:{target}"), i64::from(spline_id), false)?;
                record_progress_int(ctx, &format!("quest_spline_active:{target}"), 1, false)?;
                if let Some(pc) = ctx.progress_comp.as_deref_mut() {
                    pc.info
                        .set_key_int(&format!("scene_item_spline_active:{target}"), 0);
                    pc.info
                        .set_key_int(&format!("vehicle:spline_active:{target}"), 0);
                    spline_state::persist_move_spline_config(
                        &mut pc.info,
                        target,
                        move_spline_config.as_ref(),
                    );
                    pc.save(ctx.fs)?;
                }
                push_spline_status(
                    ctx.sink,
                    net_id,
                    MoveStatus::Moving,
                    spline_id,
                    None,
                    move_spline_config,
                );
            }
        }
        "PlayerMoveToPoint" => {
            if let Some(pos) =
                parse_vector_pos(params.get("Pos")).or_else(|| parse_vector_pos(params.get("TargetPos")))
            {
                update_player_scene_state(ctx, pos, None)?;
                ctx.sink.push(
                    TeleportNotify::MSG_ID,
                    &TeleportNotify {
                        MapId: inst_id,
                        Pos: Some(Vector {
                            X: pos[0],
                            Y: pos[1],
                            Z: pos[2],
                        }),
                        Reason: Some(TeleportReason::Action),
                        ..Default::default()
                    },
                );
            }
        }
        "ChangeLiftTarget" => {
            if let Some(cfg_id) = resolve_action_entity_cfg_id(ctx.scene.as_deref(), params) {
                let loc = json_int(params.get("Location")).unwrap_or(0);
                let prev_loc = ctx
                    .scene
                    .as_deref()
                    .and_then(|scene| {
                        let net_id = scene.find_entity_net_id_by_config_id(cfg_id)?;
                        scene.entity(net_id)?.world_item.lift.map(|l| l.location)
                    })
                    .or_else(|| {
                        let pc = ctx.progress_comp.as_deref()?;
                        let v = pc.info.get_key_int(&format!("lift_target:{cfg_id}"));
                        (v > 0).then(|| i32::try_from(v).ok()).flatten()
                    });
                record_progress_int(ctx, &format!("lift_target:{cfg_id}"), i64::from(loc), false)?;
                record_progress_int(ctx, &format!("elevator:{cfg_id}"), i64::from(loc), false)?;
                ensure_quest_entity_spawned(ctx, cfg_id);
                if let Some(scene) = ctx.scene.as_deref_mut()
                    && let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id)
                    && let Some(storage) = scene.entity_mut(net_id)
                {
                    storage.world_item.lift = Some(LiftComponent { location: loc });
                    if loc > 0 && prev_loc != Some(loc) {
                        ctx.sink.push(
                            ElevatorMoveNotify::MSG_ID,
                            &ElevatorMoveNotify {
                                EntityId: net_id,
                                Location: loc,
                                MoveType: None,
                            },
                        );
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_quest_entity_id(value_opt: Option<&Value>) -> Option<i32> {
    let value = value_opt?;
    if let Some(id) = json_int(Some(value)) {
        return Some(id);
    }
    value
        .as_object()
        .and_then(|o| json_int(o.get("EntityId")))
}

fn persist_quest_entity_position<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    config_id: i32,
    net_id: i64,
    pos: [f32; 3],
) -> io::Result<()> {
    if !pos.iter().all(|v| v.is_finite()) {
        return Ok(());
    }
    let (entity_type, is_player_entity, position_opt) = {
        let Some(scene) = ctx.scene.as_deref_mut() else {
            return Ok(());
        };
        if !scene.set_entity_position(net_id, pos, None) {
            return Ok(());
        }
        scene.save_entity_by_net_id(ctx.fs, net_id)?;
        let Some(storage) = scene.entity(net_id) else {
            return Ok(());
        };
        (
            storage.config.entity_type,
            storage.player_id.is_some(),
            storage.position,
        )
    };

    if !matches!(entity_type, EntityType::Player | EntityType::PlayerEntity) && !is_player_entity
    {
        let Some(pc) = ctx.progress_comp.as_deref_mut() else {
            return Ok(());
        };
        let Some(position) = position_opt else {
            return Ok(());
        };
        let fields = ["x", "y", "z", "roll", "pitch", "yaw"];
        let values = [
            position.location[0],
            position.location[1],
            position.location[2],
            position.rotation[0],
            position.rotation[1],
            position.rotation[2],
        ];
        for (field, value) in fields.into_iter().zip(values) {
            pc.info.set_key_int(
                &format!("world_pos:{config_id}:{field}"),
                i64::from(value.to_bits()),
            );
        }
        pc.save(ctx.fs)?;
    }
    Ok(())
}

fn apply_vehicle_enter_for_player<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    veh_cid: i32,
    seat: i32,
    is_teleport_enter: bool,
) -> io::Result<()> {
    record_progress_int(ctx, "vehicle:cur", i64::from(veh_cid), false)?;
    record_progress_int(ctx, "vehicle:seat", i64::from(seat), false)?;
    let player_id = ctx.basic_comp.player_id;
    let found = ctx.scene.as_deref_mut().and_then(|scene| {
        let inst_id = scene.instance_id;
        let veh_nid = scene.find_entity_net_id_by_config_id(veh_cid)?;
        let mut tp_pos = None;
        if let Some(storage) = scene.entity_mut(veh_nid) {
            if let Some(veh) = storage.vehicle.as_mut() {
                veh.occupied = true;
            }
            if is_teleport_enter {
                tp_pos = storage.position;
            }
        }
        Some((inst_id, veh_nid, tp_pos))
    });
    if let Some((inst_id, veh_nid, tp_pos)) = found {
        if let Some(vp) = tp_pos {
            update_player_scene_state(ctx, vp.location, Some(vp.rotation))?;
            ctx.sink.push(
                TeleportNotify::MSG_ID,
                &TeleportNotify {
                    MapId: inst_id,
                    Pos: Some(Vector {
                        X: vp.location[0],
                        Y: vp.location[1],
                        Z: vp.location[2],
                    }),
                    Rot: Some(Vector {
                        X: vp.rotation[0],
                        Y: vp.rotation[1],
                        Z: vp.rotation[2],
                    }),
                    Reason: Some(TeleportReason::Action),
                    ..Default::default()
                },
            );
        }
        ctx.sink.push(
            VehicleUpdateNotify::MSG_ID,
            &VehicleUpdateNotify {
                EntityId: veh_nid,
                PlayerId: player_id,
                SeatId: seat,
                ..Default::default()
            },
        );
    }
    Ok(())
}

fn apply_vehicle_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    name: &str,
    params: &Map<String, Value>,
) -> io::Result<()> {
    match name {
        "VehicleEnter" | "TeleportToAndEnterVehicle" => {
            let seat = json_int(params.get("Seat")).unwrap_or(0);
            let is_teleport_enter = name == "TeleportToAndEnterVehicle";
            let vt_vid = params
                .get("VehicleTarget")
                .and_then(|v| v.as_object())
                .and_then(|vt| json_int(vt.get("VehicleId")));

            if let Some(et) = params.get("EnteringTarget").and_then(|v| v.as_object()) {
                let target_ty = json_str(et.get("Type")).unwrap_or("Player");
                if let Some(vid) = json_int(et.get("TargetVehicle")).or(vt_vid) {
                    ensure_quest_entity_spawned(ctx, vid);
                    if target_ty == "Npc" {
                        if let Some(npc_cid) = json_int(et.get("TargetNpc")) {
                            ensure_quest_entity_spawned(ctx, npc_cid);
                            record_vehicle_npc_link(ctx, i64::from(npc_cid), i64::from(vid), seat)?;
                            if let Some(scene) = ctx.scene.as_deref_mut()
                                && let (Some(npc_nid), Some(veh_nid)) = (
                                    scene.find_entity_net_id_by_config_id(npc_cid),
                                    scene.find_entity_net_id_by_config_id(vid),
                                )
                            {
                                if let Some(vs) = scene.entity_mut(veh_nid)
                                    && let Some(veh) = vs.vehicle.as_mut()
                                {
                                    veh.occupied = true;
                                }
                                if let Some(ns) = scene.entity_mut(npc_nid) {
                                    ns.npc_drive_vehicle = Some(
                                        crate::component::entity::small::NpcDriveVehicleComponent {
                                            vehicle_entity_id: veh_nid,
                                            seat,
                                        },
                                    );
                                }
                                ctx.sink.push(
                                    VehicleUpdateEntityNotify::MSG_ID,
                                    &VehicleUpdateEntityNotify {
                                        EntityId: npc_nid,
                                        VehicleCreatureId: veh_nid,
                                        Seat: seat,
                                        IsEntering: true,
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                        return Ok(());
                    }
                    return apply_vehicle_enter_for_player(ctx, vid, seat, is_teleport_enter);
                }
            }

            if let Some(vid) = json_int(params.get("VehicleEntityId"))
                .or_else(|| json_int(params.get("TargetVehicle")))
                .or(vt_vid)
            {
                ensure_quest_entity_spawned(ctx, vid);
                if vid > 0 {
                    apply_vehicle_enter_for_player(ctx, vid, seat, is_teleport_enter)?;
                }
            } else if let Some(tv) = params.get("ToVehicle").and_then(|v| v.as_object()) {
                let tv_type = json_str(tv.get("Type")).unwrap_or("");
                if let Some(vid) = json_int(tv.get("EntityId")) {
                    ensure_quest_entity_spawned(ctx, vid);
                    if let Some(pc) = ctx.progress_comp.as_deref() {
                        let prev_vid = pc.info.get_key_int("vehicle:cur") as i32;
                        if prev_vid > 0 && prev_vid != vid {
                            if let Some(scene) = ctx.scene.as_deref_mut()
                                && let Some(old_nid) = scene.find_entity_net_id_by_config_id(prev_vid)
                                && let Some(os) = scene.entity_mut(old_nid)
                                && let Some(old_veh) = os.vehicle.as_mut()
                            {
                                old_veh.occupied = false;
                            }
                            ctx.sink.push(
                                VehicleUpdateNotify::MSG_ID,
                                &VehicleUpdateNotify {
                                    EntityId: 0,
                                    PlayerId: ctx.basic_comp.player_id,
                                    SeatId: -1,
                                    ExitType: Some(ExitVehicleType::ExitVehicleTypeNormal),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    apply_vehicle_enter_for_player(ctx, vid, seat, is_teleport_enter)?;
                } else if tv_type == "FishingBoat" {
                    record_progress_int(ctx, "fish:boat_active", 1, false)?;
                    let matched = ctx.scene.as_deref_mut().and_then(|scene| {
                        for storage in &mut scene.entities {
                            if let Some(veh) = storage.vehicle.as_mut() {
                                veh.occupied = true;
                                return Some((storage.net_id, storage.config.config_id, storage.position));
                            }
                        }
                        None
                    });
                    if let Some((nid, cfg_id, pos_opt)) = matched {
                        record_progress_int(ctx, "vehicle:cur", i64::from(cfg_id), false)?;
                        record_progress_int(ctx, "vehicle:seat", i64::from(seat), false)?;
                        if is_teleport_enter
                            && let Some(vp) = pos_opt
                            && let Some(inst_id) = ctx.scene.as_deref().map(|s| s.instance_id)
                        {
                            update_player_scene_state(ctx, vp.location, Some(vp.rotation))?;
                            ctx.sink.push(
                                TeleportNotify::MSG_ID,
                                &TeleportNotify {
                                    MapId: inst_id,
                                    Pos: Some(Vector {
                                        X: vp.location[0],
                                        Y: vp.location[1],
                                        Z: vp.location[2],
                                    }),
                                    Rot: Some(Vector {
                                        X: vp.rotation[0],
                                        Y: vp.rotation[1],
                                        Z: vp.rotation[2],
                                    }),
                                    Reason: Some(TeleportReason::Action),
                                    ..Default::default()
                                },
                            );
                        }
                        ctx.sink.push(
                            VehicleUpdateNotify::MSG_ID,
                            &VehicleUpdateNotify {
                                EntityId: nid,
                                PlayerId: ctx.basic_comp.player_id,
                                SeatId: seat,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }
        "VehicleExitPlayer" => {
            let vehicle_id = ctx
                .progress_comp
                .as_deref()
                .map_or(0, |pc| pc.info.get_key_int("vehicle:cur") as i32);
            record_progress_int(ctx, "vehicle:cur", 0, false)?;
            record_progress_int(ctx, "vehicle:seat", -1, false)?;
            if ctx.scene.is_some() {
                let clear_occupied =
                    vehicle_id > 0 && !vehicle_has_other_occupants(ctx, vehicle_id, None);
                if clear_occupied
                    && let Some(scene) = ctx.scene.as_deref_mut()
                    && let Some(veh_nid) = scene.find_entity_net_id_by_config_id(vehicle_id)
                    && let Some(vs) = scene.entity_mut(veh_nid)
                    && let Some(veh) = vs.vehicle.as_mut()
                {
                    veh.occupied = false;
                }
                ctx.sink.push(
                    VehicleUpdateNotify::MSG_ID,
                    &VehicleUpdateNotify {
                        EntityId: 0,
                        PlayerId: ctx.basic_comp.player_id,
                        SeatId: -1,
                        ExitType: Some(ExitVehicleType::ExitVehicleTypeNormal),
                        ..Default::default()
                    },
                );
            }
        }
        "VehicleExitNpc" => {
            if let Some(npc_cfg) = json_int(params.get("NpcEntityId"))
                .or_else(|| json_int(params.get("TargetNpc")))
            {
                let link_key = format!("vehicle:npc:{npc_cfg}");
                let previous_vehicle_id = ctx
                    .progress_comp
                    .as_deref()
                    .and_then(|pc| i32::try_from(pc.info.get_key_int(&link_key)).ok())
                    .unwrap_or(0);
                record_vehicle_npc_link(ctx, i64::from(npc_cfg), 0, -1)?;
                if ctx.scene.is_some() {
                    if npc_cfg > 0
                        && let Some(scene) = ctx.scene.as_deref_mut()
                        && let Some(npc_nid) = scene.find_entity_net_id_by_config_id(npc_cfg)
                    {
                        if let Some(ns) = scene.entity_mut(npc_nid) {
                            ns.npc_drive_vehicle = None;
                        }
                        ctx.sink.push(
                            VehicleUpdateEntityNotify::MSG_ID,
                            &VehicleUpdateEntityNotify {
                                EntityId: npc_nid,
                                VehicleCreatureId: 0,
                                Seat: -1,
                                IsEntering: false,
                                ExitType: Some(ExitVehicleType::ExitVehicleTypeNormal),
                                ..Default::default()
                            },
                        );
                    }
                    if previous_vehicle_id > 0
                        && !vehicle_has_other_occupants(ctx, previous_vehicle_id, Some(npc_cfg))
                        && let Some(scene) = ctx.scene.as_deref_mut()
                        && let Some(veh_nid) = scene.find_entity_net_id_by_config_id(previous_vehicle_id)
                        && let Some(vs) = scene.entity_mut(veh_nid)
                        && let Some(veh) = vs.vehicle.as_mut()
                    {
                        veh.occupied = false;
                    }
                }
            }
        }
        "VehicleTeleport" => {
            if let Some(pos) = parse_vector_pos(params.get("Pos"))
                && ctx.scene.is_some()
            {
                update_player_scene_state(ctx, pos, None)?;
                let occupied_vehicles: Vec<(i32, i64)> = ctx
                    .scene
                    .as_deref()
                    .expect("checked")
                    .entities
                    .iter()
                    .filter(|s| s.vehicle.is_some_and(|v| v.occupied))
                    .map(|s| (s.config.config_id, s.net_id))
                    .collect();
                for (cfg_id, net_id) in occupied_vehicles {
                    persist_quest_entity_position(ctx, cfg_id, net_id, pos)?;
                    push_entity_moved(ctx.sink, net_id, pos, None);
                }
                let inst_id = ctx.scene.as_deref().expect("checked").instance_id;
                ctx.sink.push(
                    TeleportNotify::MSG_ID,
                    &TeleportNotify {
                        MapId: inst_id,
                        Pos: Some(Vector {
                            X: pos[0],
                            Y: pos[1],
                            Z: pos[2],
                        }),
                        Reason: Some(TeleportReason::Action),
                        ..Default::default()
                    },
                );
            }
        }
        "VehicleMoveWithPathLine" => {
            let Some(control) = params.get("ControlType").and_then(|v| v.as_object()) else {
                return Ok(());
            };
            let Some(control_type) = json_str(control.get("Type")) else {
                return Ok(());
            };
            let Some(target) = params.get("TargetVehicle").and_then(|v| v.as_object()) else {
                return Ok(());
            };
            let Some(target_type) = json_str(target.get("Type")) else {
                return Ok(());
            };
            let vehicle_cfg_id = match target_type {
                "Appointed" => json_int(target.get("VehicleId")).unwrap_or(0),
                "Current" => {
                    let Some(pc) = ctx.progress_comp.as_deref() else {
                        return Ok(());
                    };
                    pc.info.get_key_int("vehicle:cur") as i32
                }
                _ => 0,
            };
            if vehicle_cfg_id <= 0 {
                return Ok(());
            }
            let spline_id = json_int(params.get("SplineEntityId")).unwrap_or(0);
            let is_entering = control_type == "EnterPathMoving";
            record_progress_int(
                ctx,
                &format!("vehicle:spline:{vehicle_cfg_id}"),
                if is_entering { i64::from(spline_id) } else { 0 },
                false,
            )?;
            record_progress_int(
                ctx,
                &format!("vehicle:spline_active:{vehicle_cfg_id}"),
                i64::from(is_entering),
                false,
            )?;
            record_progress_int(ctx, &format!("scene_item_spline_active:{vehicle_cfg_id}"), 0, false)?;
            record_progress_int(ctx, &format!("quest_spline_active:{vehicle_cfg_id}"), 0, false)?;
            ensure_quest_entity_spawned(ctx, vehicle_cfg_id);
            if let Some(scene) = ctx.scene.as_deref_mut()
                && let Some(net_id) = scene.find_entity_net_id_by_config_id(vehicle_cfg_id)
            {
                if let Some(storage) = scene.entity_mut(net_id) {
                    storage.world_item.move_spline = if is_entering {
                        Some(MoveSplineComponent {
                            move_status: Some(MoveStatus::Moving),
                            spline_entity_id: spline_id,
                            runtime_data: None,
                            move_spline_config: None,
                        })
                    } else {
                        None
                    };
                }
                push_spline_status(
                    ctx.sink,
                    net_id,
                    if is_entering {
                        MoveStatus::Moving
                    } else {
                        MoveStatus::Stop
                    },
                    spline_id,
                    None,
                    None,
                );
            }
        }
        "StopNewMoveWithSpline" => {
            let Some(stop_target) = params.get("StopTarget").and_then(|v| v.as_object()) else {
                return Ok(());
            };
            let Some(stop_type) = json_str(stop_target.get("Type")) else {
                return Ok(());
            };
            let target_cfg_id = if stop_type == "Vehicle" {
                let Some(target) = stop_target.get("TargetVehicle").and_then(|v| v.as_object()) else {
                    return Ok(());
                };
                match json_str(target.get("Type")) {
                    Some("Appointed") => json_int(target.get("VehicleId")).unwrap_or(0),
                    Some("Current") => {
                        let Some(pc) = ctx.progress_comp.as_deref() else {
                            return Ok(());
                        };
                        pc.info.get_key_int("vehicle:cur") as i32
                    }
                    _ => 0,
                }
            } else if matches!(stop_type, "SceneItem" | "Entity") {
                json_int(stop_target.get("EntityId")).unwrap_or(0)
            } else {
                0
            };
            if target_cfg_id <= 0 {
                return Ok(());
            }

            let mut spline_id = ctx.progress_comp.as_deref().map_or(0, |progress| {
                let vsp = progress
                    .info
                    .get_key_int(&format!("vehicle:spline:{target_cfg_id}"));
                let qsp = progress
                    .info
                    .get_key_int(&format!("quest_spline:{target_cfg_id}"));
                let isp = progress
                    .info
                    .get_key_int(&format!("scene_item_spline:{target_cfg_id}"));
                let chosen = if vsp != 0 {
                    vsp
                } else if qsp != 0 {
                    qsp
                } else {
                    isp
                };
                i32::try_from(chosen).unwrap_or(0)
            });

            record_progress_int(ctx, &format!("vehicle:spline_active:{target_cfg_id}"), 0, false)?;
            record_progress_int(ctx, &format!("quest_spline_active:{target_cfg_id}"), 0, false)?;
            record_progress_int(ctx, &format!("scene_item_spline_active:{target_cfg_id}"), 0, false)?;

            let stop_at_end = params
                .get("StopType")
                .and_then(|v| v.as_object())
                .and_then(|st| json_str(st.get("Type")))
                == Some("StopAtEndPoint");

            let scene_info = ctx.scene.as_deref_mut().and_then(|scene| {
                let inst_id = scene.instance_id;
                let net_id = scene.find_entity_net_id_by_config_id(target_cfg_id)?;
                if let Some(storage) = scene.entity_mut(net_id) {
                    if spline_id == 0
                        && let Some(ms) = &storage.world_item.move_spline
                    {
                        spline_id = ms.spline_entity_id;
                    }
                    storage.world_item.move_spline = None;
                }
                Some((inst_id, net_id))
            });
            if let Some((inst_id, net_id)) = scene_info {
                if stop_at_end
                    && spline_id > 0
                    && let Some(end_pos) =
                        resolve_quest_spline_end_pos(ctx.assets, inst_id, i64::from(spline_id))
                {
                    persist_quest_entity_position(ctx, target_cfg_id, net_id, end_pos)?;
                    push_entity_moved(ctx.sink, net_id, end_pos, None);
                }
                push_spline_status(ctx.sink, net_id, MoveStatus::Stop, spline_id, None, None);
            }
        }
        _ => {}
    }
    Ok(())
}

fn record_vehicle_npc_link<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    npc_config_id: i64,
    vehicle_config_id: i64,
    seat: i32,
) -> io::Result<()> {
    let Some(progress) = ctx.progress_comp.as_deref_mut() else {
        return Ok(());
    };
    progress
        .info
        .set_key_int(&format!("vehicle:npc:{npc_config_id}"), vehicle_config_id);
    progress
        .info
        .set_key_int(&format!("vehicle:npc_seat:{npc_config_id}"), i64::from(seat));
    progress.save(ctx.fs)
}

fn vehicle_has_other_occupants<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    vehicle_config_id: i32,
    excluding_npc_id: Option<i32>,
) -> bool {
    let Some(progress) = ctx.progress_comp.as_deref() else {
        return false;
    };
    if let Ok(cur_vehicle_id) = i32::try_from(progress.info.get_key_int("vehicle:cur"))
        && cur_vehicle_id == vehicle_config_id
    {
        return true;
    }
    for entry in &progress.info.dynamic.ints {
        let Some(rest) = entry.key.strip_prefix("vehicle:npc:") else {
            continue;
        };
        let Ok(npc_id) = rest.parse::<i32>() else {
            continue;
        };
        if excluding_npc_id == Some(npc_id) {
            continue;
        }
        if let Ok(linked_vid) = i32::try_from(entry.val)
            && linked_vid == vehicle_config_id
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonoro_common::fs::FileSystem;
    use sonoro_gamesv_fs::quest_info::append_unique_int;
    use crate::component::player::{
        PlayerBasicComponent, PlayerEchoComponent, PlayerInventoryComponent, PlayerMapComponent,
        PlayerProgressComponent, PlayerQuestComponent, PlayerWeaponComponent,
    };
    use crate::event_queue::EventQueue;
    use crate::helpers::quest_types::QuestContext;
    use sonoro_gamesv_fs::scene_instance::SceneInstance;
    use crate::scene::Scene;
    use crate::sink::VecPushSink;
    use sonoro_proto::PbMessage;
    use std::path::PathBuf;

    fn test_temp_dir(name: &str) -> (PathBuf, FileSystem) {
        let path = std::env::temp_dir().join(format!(
            "sonoro_quest_actions_{name}_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let fs = FileSystem::open(path.to_str().unwrap()).unwrap();
        (path, fs)
    }

    fn test_empty_assets() -> Assets {
        Assets::default()
    }

    #[test]
    fn quest_info_data_layer_sub_level_area_state_and_var_transitions_consistent() {
        let mut info = QuestInfo::default();
        info.set_var_int("调查", 2);
        assert_eq!(info.get_var_int("调查"), 2);

        append_unique_int(&mut info.world.loaded_datalayers, 3);
        append_unique_int(&mut info.world.unloaded_datalayers, 2);
        remove_int_value(&mut info.world.loaded_datalayers, 2);
        assert!(info.world.loaded_datalayers.contains(&3));
        assert!(!info.world.loaded_datalayers.contains(&2));

        append_unique_str(&mut info.world.loaded_sublevels, "/Game/A");
        assert!(info.world.loaded_sublevels.iter().any(|s| s == "/Game/A"));
        assert!(remove_str_value(&mut info.world.loaded_sublevels, "/Game/A"));
        assert!(!info.world.loaded_sublevels.iter().any(|s| s == "/Game/A"));
    }

    #[test]
    fn execute_action_handles_run_actions_random_var_sheriff_wuyinqu_and_vehicle() {
        let (path, fs) = test_temp_dir("run_actions");
        let assets = test_empty_assets();
        let mut sink = VecPushSink::default();
        let mut events = EventQueue::new();

        let mut basic = PlayerBasicComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut inv = PlayerInventoryComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut wep = PlayerWeaponComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut echo = PlayerEchoComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut map = PlayerMapComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut quest = PlayerQuestComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut prog = PlayerProgressComponent {
            player_id: 1,
            ..Default::default()
        };

        let mut ctx = QuestContext {
            assets: &assets,
            fs: &fs,
            sink: &mut sink,
            scene: None,
            scene_comp: None,
            events: &mut events,
            basic_comp: &mut basic,
            inventory_comp: &mut inv,
            weapon_comp: &mut wep,
            echo_comp: &mut echo,
            map_comp: &mut map,
            quest_comp: &mut quest,
            role_comp: None,
            guide_comp: None,
            mail_comp: None,
            progress_comp: Some(&mut prog),
            activity_notify: None,
        };

        fn trace_current_quest<S: PushSink + ?Sized>(
            action_ctx: &mut QuestContext<'_, S>,
            quest_id: i32,
        ) -> io::Result<()> {
            let progress = action_ctx
                .progress_comp
                .as_deref_mut()
                .ok_or_else(|| io::Error::other("MissingProgress"))?;
            progress
                .info
                .set_key_int("test:trace_current_quest", i64::from(quest_id));
            Ok(())
        }
        let mut callbacks = ActionCallbacks {
            on_trace_current_quest: Some(trace_current_quest),
            ..Default::default()
        };

        let act: Value = serde_json::from_str(
            r#"{"Name":"RunActions","Params":{"ActionList":[
              {"Name":"RandomVar","Params":{"Result":{"Name":"roll_res"},"RightVar":{"Value":5}}},
              {"Name":"SheriffUnlockClue","Params":{"ClueId":42}},
              {"Name":"SetWuYinQuState","Params":{"WuYinQuName":"zone_alpha","State":2}},
              {"Name":"TraceCurrentQuest","Params":{}},
              {"Name":"SetPlayerOperationRestriction","Params":{"Type":"DisableAll"}},
              {"Name":"VehicleEnter","Params":{"VehicleEntityId":8001}}
            ]}}"#,
        )
        .unwrap();
        execute_action(&mut ctx, &mut callbacks, 100, 1, &act).unwrap();

        let roll = ctx.quest_comp.info.get_var_int("roll_res");
        assert!((0..5).contains(&roll));
        let p = ctx.progress_comp.as_deref().unwrap();
        assert!(p.info.has_claim("sheriff_clue:42"));
        assert_eq!(p.info.get_key_int("wuyinqu:zone_alpha"), 2);
        assert_eq!(p.info.get_key_int("test:trace_current_quest"), 100);
        assert_eq!(p.info.get_key_int("vehicle:cur"), 8001);

        let exit_act: Value =
            serde_json::from_str(r#"{"Name":"VehicleExitPlayer","Params":{}}"#).unwrap();
        let mut default_cbs = ActionCallbacks::default();
        execute_action(&mut ctx, &mut default_cbs, 100, 2, &exit_act).unwrap();
        assert_eq!(
            ctx.progress_comp.as_deref().unwrap().info.get_key_int("vehicle:cur"),
            0
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn execute_action_handles_3_8_11_role_tower_rover_rogue_sheriff_and_phantom_formation() {
        let (path, fs) = test_temp_dir("3811_actions");
        let assets = test_empty_assets();
        let mut sink = VecPushSink::default();
        let mut events = EventQueue::new();

        let mut basic = PlayerBasicComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut inv = PlayerInventoryComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut wep = PlayerWeaponComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut echo = PlayerEchoComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut map = PlayerMapComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut quest = PlayerQuestComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut prog = PlayerProgressComponent {
            player_id: 1,
            ..Default::default()
        };

        let mut ctx = QuestContext {
            assets: &assets,
            fs: &fs,
            sink: &mut sink,
            scene: None,
            scene_comp: None,
            events: &mut events,
            basic_comp: &mut basic,
            inventory_comp: &mut inv,
            weapon_comp: &mut wep,
            echo_comp: &mut echo,
            map_comp: &mut map,
            quest_comp: &mut quest,
            role_comp: None,
            guide_comp: None,
            mail_comp: None,
            progress_comp: Some(&mut prog),
            activity_notify: None,
        };

        let act: Value = serde_json::from_str(
            r#"{"Name":"RunActions","Params":{"ActionList":[
              {"Name":"RoleTowerGotoNextFloor","Params":{}},
              {"Name":"RoverRogueRoomFinish","Params":{}},
              {"Name":"SheriffUnlockProgress","Params":{"ProgressId":9}},
              {"Name":"ChangePhantomFormation","Params":{"FormationId":3}}
            ]}}"#,
        )
        .unwrap();
        let mut callbacks = ActionCallbacks::default();
        execute_action(&mut ctx, &mut callbacks, 101, 1, &act).unwrap();

        let p = ctx.progress_comp.as_deref().unwrap();
        assert_eq!(p.info.get_key_int("role_tower:stage"), 1);
        assert_eq!(p.info.get_key_int("rover_rogue:layer"), 1);
        assert!(p.info.has_claim("sheriff_prog:9"));
        assert_eq!(p.info.get_key_int("phantom:formation_id"), 3);

        let restore_act: Value =
            serde_json::from_str(r#"{"Name":"RestorePhantomFormation","Params":{}}"#).unwrap();
        execute_action(&mut ctx, &mut callbacks, 101, 2, &restore_act).unwrap();
        assert_eq!(
            ctx.progress_comp
                .as_deref()
                .unwrap()
                .info
                .get_key_int("phantom:formation_id"),
            0
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn execute_action_set_player_pos_attaches_flow_action_ctx_when_inside_active_flow() {
        let (path, fs) = test_temp_dir("flow_action_ctx");
        let assets = test_empty_assets();
        let mut sink = VecPushSink::default();
        let mut events = EventQueue::new();

        let mut basic = PlayerBasicComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut inv = PlayerInventoryComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut wep = PlayerWeaponComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut echo = PlayerEchoComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut map = PlayerMapComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut quest = PlayerQuestComponent {
            player_id: 1,
            ..Default::default()
        };
        let mut prog = PlayerProgressComponent {
            player_id: 1,
            ..Default::default()
        };

        let mut inst = SceneInstance {
            owner_id: 1,
            ..Default::default()
        };
        inst.players.push(Default::default());
        inst.players[0].id = 1;
        let mut scene = Scene::from_parts(
            1,
            8,
            inst,
            Default::default(),
            Default::default(),
        );
        scene.set_active_flow(1, 139000025, 21, "剧情_新主线_CE版_序章", 1, 12);

        let mut ctx = QuestContext {
            assets: &assets,
            fs: &fs,
            sink: &mut sink,
            scene: Some(&mut scene),
            scene_comp: None,
            events: &mut events,
            basic_comp: &mut basic,
            inventory_comp: &mut inv,
            weapon_comp: &mut wep,
            echo_comp: &mut echo,
            map_comp: &mut map,
            quest_comp: &mut quest,
            role_comp: None,
            guide_comp: None,
            mail_comp: None,
            progress_comp: Some(&mut prog),
            activity_notify: None,
        };

        let act: Value = serde_json::from_str(
            r#"{"Name":"SetPlayerPos","ActionId":5000073,"Params":{"TelePortConfig":{"Type":0,"TargetPos":{"X":-114682,"Y":71774,"Z":13457,"A":30}}}}"#,
        )
        .unwrap();

        let mut non_flow_cbs = ActionCallbacks {
            is_flow_action: false,
            ..Default::default()
        };
        execute_action(&mut ctx, &mut non_flow_cbs, 139000025, 21, &act).unwrap();
        let nf_pkt = ctx
            .sink
            .packets
            .iter()
            .find(|p| p.msg_id == TeleportNotify::MSG_ID)
            .expect("non-flow TeleportNotify");
        let notify_nf = TeleportNotify::decode(&nf_pkt.payload).unwrap();
        assert_eq!(notify_nf.Reason, Some(TeleportReason::Action));
        assert!(notify_nf.GameCtx.is_none());

        ctx.sink.packets.clear();
        let mut flow_cbs = ActionCallbacks {
            is_flow_action: true,
            ..Default::default()
        };
        execute_action(&mut ctx, &mut flow_cbs, 139000025, 21, &act).unwrap();
        let fl_pkt = ctx
            .sink
            .packets
            .iter()
            .find(|p| p.msg_id == TeleportNotify::MSG_ID)
            .expect("flow TeleportNotify");
        let notify = TeleportNotify::decode(&fl_pkt.payload).unwrap();
        assert_eq!(notify.Reason, Some(TeleportReason::FlowAction));
        let gctx = notify.GameCtx.unwrap();
        assert_eq!(gctx.CtxType, Some(GameCtxType::FlowActionCtx));
        let Some(GameCtxPbCtxInfoOneof::FlowAction(flow_ctx)) = gctx.CtxInfo else {
            panic!("expected FlowAction");
        };
        assert_eq!(
            String::from_utf8_lossy(&flow_ctx.FlowListName),
            "剧情_新主线_CE版_序章"
        );
        assert_eq!(flow_ctx.FlowId, 1);
        assert_eq!(flow_ctx.StateId, 12);
        assert_eq!(flow_ctx.ActionId, 5000073);

        let _ = std::fs::remove_dir_all(path);
    }
}
