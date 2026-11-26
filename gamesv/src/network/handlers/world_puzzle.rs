use std::collections::BTreeMap;

use serde_json::Value;
use sonoro_gamesv_data::asset_indexes::{entity_world_pos, entity_world_rot};
use sonoro_gamesv_data::tables::entity_components::extra::{ExtraEntry, HookInteractConfig};
use sonoro_gamesv_fs::formation_info::FormationRole;
use sonoro_gamesv_logic::component::entity::small::{
    EntityType, LiftComponent as EntityLiftComponent, MoveSplineComponent, MoveToPointComponent,
    SceneItemComponent,
};
use sonoro_gamesv_logic::component::entity::tag::TagComponent;
use sonoro_gamesv_logic::component::player::PlayerQuestComponent;
use sonoro_gamesv_logic::event_queue::EventQueue;
use sonoro_gamesv_logic::events::Event;
use sonoro_gamesv_logic::fsm::types::wake_reason;
use sonoro_gamesv_logic::handlers::tags as tags_handler;
use sonoro_gamesv_logic::helpers::consume::CostItem;
use sonoro_gamesv_logic::helpers::level_play::LevelPlayEventKind;
use sonoro_gamesv_logic::helpers::quest_actions;
use sonoro_gamesv_logic::helpers::quest_engine;
use sonoro_gamesv_logic::helpers::reward::RewardItem;
use sonoro_gamesv_logic::helpers::{
    condition, consume, energy, entity_proto, gameplay_tags, gravity, level_play, map as map_helper,
    pb_kv, reward, role as role_helper, spline_state,
};
use sonoro_proto::enums::{
    ArchiveType, EAttributeType, ErrorCode, GameCtxType, GravityFlipType, HookInteractActionType,
    MoveStatus, SneakFinishType, TeleportReason,
};
use sonoro_proto::pb::{
    AcceptEntrustRequest, AcceptEntrustResponse, AccessPasserbyNpcSpawnerRequest,
    AccessPasserbyNpcSpawnerResponse, ActionNotify, ActionOperationScreenSuccessRequest,
    ActionOperationScreenSuccessResponse, ActionResetPosPrePerformanceEndRequest,
    ActionResetPosPrePerformanceEndResponse, AddDecalRequest, AddDecalResponse,
    AddTreasureBoxSlotRequest, AddTreasureBoxSlotResponse, AttachEntityRequest,
    AttachEntityResponse, AwakeDynamicPortalRequest, AwakeDynamicPortalResponse,
    BatchMaterialReplaceRequest, BatchMaterialReplaceResponse, BeControlledDestroyRequest,
    BeControlledDestroyResponse, BeControlledEntityHoverRequest,
    BeControlledEntityHoverResponse, BeControlledRequest, BeControlledResponse,
    BeControlledShowRequest, BeControlledShowResponse, BeControlledThrowRequest,
    BeControlledThrowResponse, BeamReceiveNotify, BlackboardParamValueOneof,
    BtForcedOccupationRequest, BtForcedOccupationResponse, CaptureBatchEntityRequest,
    CaptureBatchEntityResponse, CaptureEntityRequest, CaptureEntityResponse,
    ChangeEntityRoleRequest, ChangeEntityRoleResponse, ChangeEntityStateRequest,
    ChangeEntityStateResponse, CharacterMoveToPointEndPush, ChatMutePlayerRequest,
    ChatMutePlayerResponse, CombineEntitiesRequest, CombineEntitiesResponse,
    CommonOrganResetTimeRequest, CommonOrganResetTimeResponse, CommonTagData,
    CommonTouchUiSettingUpdateRequest, CommonTouchUiSettingUpdateResponse,
    ConnectorCanConnectRequest, ConnectorCanConnectResponse, ConnectorConnectEntityNotify,
    ConnectorConnectEntityRequest, ConnectorConnectEntityResponse, ControlPointData,
    DiscardWeaponPush, DiscardWeaponRequest, DiscardWeaponResponse,
    DissolveCombineRelationRequest, DissolveCombineRelationResponse, ElevatorMoveNotify,
    ElevatorStateChangeRequest, ElevatorStateChangeResponse, EnrichmentAreaRequest,
    EnrichmentAreaResponse, EntityAdsorbRequest, EntityAdsorbResponse, EntityBattleInfoRequest,
    EntityBattleInfoResponse, EntityBeamReceiveRequest, EntityBeamReceiveResponse,
    EntityBlackboardRequest, EntityBlackboardResponse, EntityCameraAlertRequest,
    EntityCameraAlertResponse, EntityCommonTagNotify, EntityCtxPb, EntityPatrolAddTagRequest,
    EntityPatrolAddTagResponse, EntityPatrolChangeDirRequest, EntityPatrolChangeDirResponse,
    EntityRandomInteractRequest, EntityRandomInteractResponse, EntityRemoveInfo,
    EntityRemoveNotify, EntitySendEventRequest, EntitySendEventResponse,
    EntitySimplyMoveInfoPackagePush, EntityTimeFlowPush, EntityWalkingPatternFinishRequest,
    EntityWalkingPatternFinishResponse, EntrustInfo, ExitGamePush,
    ExploreActivityTaskRewardRequest, ExploreActivityTaskRewardResponse, FastReturnDungeonRequest,
    FastReturnDungeonResponse, FlowTeleportToPlotPosRequest, FlowTeleportToPlotPosResponse,
    FollowerChairPresentationPush, FoodProcessRequest, FoodProcessResponse,
    FurnitureDiySaveRequest, FurnitureDiySaveResponse, GameCtxPb, GameCtxPbCtxInfoOneof,
    GameplayAttributeData, GravityFlipInteractRequest, GravityFlipInteractResponse,
    GravityFlipUpdateTypeNotify, GravityHookLockPointChangeGravityRequest,
    GravityHookLockPointChangeGravityResponse, HardnessModeChangedRequest,
    HardnessModeChangedResponse, HoldHandRequest, HoldHandResponse, HoldWeaponPush,
    HookLockBatchCollectRequest, HookLockBatchCollectResponse, HookLockPointActionCtxPb,
    HookLockPointExitRequest, HookLockPointExitResponse, HookLockPointFinishRequest,
    HookLockPointFinishResponse, HookLockPointRequest, HookLockPointResponse, InstDetailEntry,
    InstEntityTeleportRequest, InstEntityTeleportResponse, InstEntranceDetailRequest,
    InstEntranceDetailResponse, InteractiveUpdateRequest, InteractiveUpdateResponse,
    InviteRechallengeRequest, InviteRechallengeResponse, LastBigSceneMiniMapInfoRequest,
    LastBigSceneMiniMapInfoResponse, LevelSequenceFrameEventActionRequest,
    LevelSequenceFrameEventActionResponse, LoadingAreaConfig, LoadingLevelAreaRequest,
    LoadingLevelAreaResponse, LoopTeleportVolumeActionRequest, LoopTeleportVolumeActionResponse,
    LowMemoryChangeSceneFinishRequest, LowMemoryChangeSceneFinishResponse, LstBattleRecordRequest,
    LstBattleRecordResponse, MaterialReplaceRequest, MaterialReplaceResponse,
    MobileButtonSettingUpdateRequest, MobileButtonSettingUpdateResponse,
    MonsterPatrolPointReachedRequest, MonsterPatrolPointReachedResponse, MovePackageNotify,
    MovePlacementNotify, MovePlacementRequest, MovePlacementResponse, MoveReplaySample,
    MoveSceneItemPush, MoveSceneItemResetPositionRequest, MoveSceneItemResetPositionResponse,
    MovingEntityData, MulFishingIllustratedRewardRequest, MulFishingIllustratedRewardResponse,
    NavigationFindPathRequest, NavigationFindPathResponse, NpcTraceFailedRequest,
    NpcTraceFailedResponse, OnlineMotorActivityLevelRequest, OnlineMotorActivityLevelResponse,
    PassPortalRequest, PassPortalResponse, PhotographPush, PickUpDropItemRequest,
    PickUpDropItemResponse, PilotThrowOperationRequest, PilotThrowOperationResponse,
    PlayEntityMontagePush, PlayerCreatePsnSessionRequest, PlayerCreatePsnSessionResponse,
    PlayerEnterPollutionRequest, PlayerEnterPollutionResponse, PlayerNetStateBackServerPush,
    PlayerPsnInfoGetRequest, PlayerPsnInfoGetResponse, PlayerReviveNotify, PlayerSceneGroupData,
    PlayerSceneGroupDataList, PlayerXboxInfoGetRequest, PlayerXboxInfoGetResponse,
    ProgressBarFinishRequest, ProgressBarFinishResponse, PsnSettingUpdateRequest,
    PsnSettingUpdateResponse, QuestFastReturnPosRequest, QuestFastReturnPosResponse,
    QuestNpcMoveOverRequest, QuestNpcMoveOverResponse, QuestReadyForGiveUpRequest,
    QuestReadyForGiveUpResponse, ReadDisplayInfoRequest, ReadDisplayInfoResponse,
    ReceiveRechallengeRequest, ReceiveRechallengeResponse, RemoteBatchCollectRequest,
    RemoteBatchCollectResponse, RemoveTreasureBoxSlotRequest, RemoveTreasureBoxSlotResponse,
    ReorderFormationRequest, ReorderFormationResponse, ResonantChainOptionalLimitRequest,
    ResonantChainOptionalLimitResponse, ReviveRequest, ReviveResponse,
    RoadNetworkEntityUpdateCurRoadPush, RoleGood, RoleProperSettle, Rotator,
    SceneGroupDataAndCurFormationEntityRequest, SceneGroupDataAndCurFormationEntityResponse,
    SceneItemBlackboardParamValueOneof, SceneItemMoveSplineRequest, SceneItemMoveSplineResponse,
    SceneItemMoveTargetRequest, SceneItemMoveTargetResponse, SceneItemSequenceFrameEndRequest,
    SceneItemSequenceFrameEndResponse, SceneItemSequenceFrameStartRequest,
    SceneItemSequenceFrameStartResponse, SceneItemSetPosSenderRequest,
    SceneItemSetPosSenderResponse, SceneItemSplineRuntimeDataDistanceOneof,
    SceneItemSplineRuntimeDataRotOneof, SceneItemStateRequest, SceneItemStateResponse,
    SceneItemStopRequest, SceneItemStopResponse, SceneModeChangeFinishRequest,
    SceneModeChangeFinishResponse, SceneRoleInformation, SeamlessTeleportRequest,
    SeamlessTeleportResponse, SeamlessTeleportVehicleRequest, SeamlessTeleportVehicleResponse,
    SetBaoziStateRequest, SetBaoziStateResponse, SetFanNumberOfTurnsRequest,
    SetFanNumberOfTurnsResponse, SetFanStateRequest, SetFanStateResponse, SetInitTagRequest,
    SetInitTagResponse, SingleItemInfo, SitChairRequest, SitChairResponse, SneakFinishRequest,
    SneakFinishResponse, SneakRequest, SneakResponse, SneakTimeRequest, SneakTimeResponse,
    SplineMoveRequest, SplineMoveResponse, SplinePositionUpdateRequest,
    SplinePositionUpdateResponse, StopHoldHandNotify, StopMoveSplineRequest,
    StopMoveSplineResponse, SubmitWalkingPatternPointRequest, SubmitWalkingPatternPointResponse,
    Summon2Request, Summon2Response, SummonRandomRequest, SummonRandomResponse,
    SwitchBattleModeRequest, SwitchBattleModeResponse, TeleportNotify, TeleportUpdateNotify,
    TestSpawnTemplateEntityPush, TimelineTraceControlRequest, TimelineTraceControlResponse,
    TimelineTraceExitRequest, TimelineTraceExitResponse, TimelineTraceStartRequest,
    TimelineTraceStartResponse, TreasureBoxSlotAllInfoNotify, TreasureBoxSlotInfo,
    UDPMovePackagePush, UiGamePlayRequest, UiGamePlayResponse, UnOpenedAreaPullbackRequest,
    UnOpenedAreaPullbackResponse, UpdateFightRoleRequest, UpdateFightRoleResponse,
    UpdateSceneGroupDataRequest, UpdateSceneGroupDataResponse, UpdateSceneItemBlackboardRequest,
    UpdateSceneItemBlackboardResponse, VarDefinePb, VarDefinePbValueOneof, Vector,
    VehiclePlatformGetOnRequest, VehiclePlatformGetOnResponse, VfxNpcPatrolFinishRequest,
    VfxNpcPatrolFinishResponse, WeatherControlInfoRequest, WeatherControlInfoResponse,
    WeatherControlRequest, WeatherControlResponse, WeatherSwitchInfoRequest,
    WeatherSwitchInfoResponse, WellStreamingFinisRequest, WellStreamingFinisResponse,
    XboxSettingUpdateRequest, XboxSettingUpdateResponse,
};
use sonoro_proto::wire::PbEnum;

use super::super::connection::drain_event_queue_best_effort;
use super::State;

fn flush_quest_condition_changed(state: &mut State) {
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
}

pub fn build_level_play_vars(quest_comp: &PlayerQuestComponent) -> BTreeMap<Vec<u8>, VarDefinePb> {
    let mut vars = BTreeMap::new();
    for qv in &quest_comp.info.vars {
        // Aki.TDConfigMgr.AutoGenerated.IVar.EVar: Boolean=0, Int=1, String=2
        let vdef = if !qv.str_val.is_empty() {
            VarDefinePb {
                VarType: 2,
                Value: Some(VarDefinePbValueOneof::String(qv.str_val.as_bytes().to_vec())),
            }
        } else if qv.bool_val {
            VarDefinePb {
                VarType: 0,
                Value: Some(VarDefinePbValueOneof::Boolean(true)),
            }
        } else {
            VarDefinePb {
                VarType: 1,
                Value: Some(VarDefinePbValueOneof::Int(i64::from(qv.int_val))),
            }
        };
        vars.insert(qv.key.as_bytes().to_vec(), vdef);
    }
    vars
}

// =========================================================================
// Entity / Puzzle / Spline / HookLock / Gravity / BeControlled / Combine
// =========================================================================

fn safe_float_to_i64(f: f32) -> i64 {
    if !f.is_finite() {
        return 0;
    }
    f.clamp(-9_000_000_000_000_000_000.0, 9_000_000_000_000_000_000.0) as i64
}

fn state_name_from_tag_id(
    state: &State,
    map_id: i32,
    entity_cfg_id: i64,
    tag_id: i32,
) -> Option<String> {
    if tag_id == 0 {
        return None;
    }
    let remaps = &state.assets.tables.gameplay_tag_remap;
    const CANDIDATES: &[&str] = &[
        "关卡.Common.状态.常态",
        "关卡.Common.状态.激活",
        "关卡.Common.状态.完成",
        "关卡.Common.状态.销毁",
        "关卡.Common.状态.解锁",
        "关卡.Common.状态.锁定",
        "关卡.Common.状态.封锁",
        "关卡.Common.状态.失败",
        "关卡.Common.状态.交互中",
        "关卡.Common.状态.准备中",
        "关卡.Common.状态.低压",
        "关卡.Common.状态.高压",
        "关卡.Common.状态.死眼命中",
        "关卡.Common.状态.场景物件交互中",
        "关卡.Common.a79721bf.常态",
        "关卡.Common.a79721bf.激活",
        "关卡.Common.表现.7a21323a.常态",
        "关卡.Common.表现.娃娃机.常态",
        "关卡.Common.表现.娃娃机.进行中",
    ];
    for &name in CANDIDATES {
        if gameplay_tags::id_from_name(remaps, name) == Some(tag_id) {
            return Some(name.to_string());
        }
    }
    if let Some(lcfg) = state.assets.find_level_entity(map_id, entity_cfg_id)
        && let Some(esc) = &lcfg.ComponentsData.EntityStateComponent
        && let Some(st) = &esc.State
        && gameplay_tags::id_from_name(remaps, st) == Some(tag_id)
    {
        return Some(st.clone());
    }
    None
}

pub fn handle_change_entity_state_request(
    state: &mut State,
    req: ChangeEntityStateRequest,
) -> Result<ChangeEntityStateResponse, String> {
    let map_id = state
        .scene
        .as_ref()
        .map(|s| state.assets.resolve_map_config_id(s.instance_id))
        .unwrap_or(8);
    let entity_id = i64::from(req.EntityId);
    let mut resolved_cfg_id = entity_id;
    let mut net_id_opt = None;
    if let Some(scene) = state.scene.as_ref() {
        if let Some(&idx) = scene.net_id_map.get(&entity_id) {
            resolved_cfg_id = i64::from(scene.entities[idx].config.config_id);
            net_id_opt = Some(entity_id);
        } else {
            net_id_opt = scene.find_entity_net_id_by_config_id(req.EntityId);
        }
    }
    let state_name_opt = state_name_from_tag_id(state, map_id, resolved_cfg_id, req.StateTagId);
    if entity_id != 0 {
        if let (Some(scene), Some(net_id)) = (state.scene.as_mut(), net_id_opt)
            && let Some(upd) = scene.update_entity_state_tag_id(net_id, req.StateTagId)
        {
            quest_actions::emit_entity_state_tag_update_checked(
                scene,
                Some(&state.assets),
                &mut state.conn,
                net_id,
                upd,
            );
        }
        if state.scene.is_some()
            && state_name_opt.as_deref() == Some("关卡.Common.状态.激活")
        {
            let mut tp_dirty = false;
            let mut newly_unlocked_tps = Vec::new();
            let tp_indices: Vec<u32> = state
                .assets
                .indexes
                .teleporter_indices_for_map(map_id)
                .to_vec();
            for tp_idx in tp_indices {
                let tp = &state.assets.tables.teleporter.items[tp_idx as usize];
                let cfg_match = tp.TeleportEntityConfigId == resolved_cfg_id
                    || tp.TeleportEntityConfigId == entity_id;
                if !cfg_match {
                    continue;
                }
                if state.player_components.map.info.unlock_teleporter(tp.Id) {
                    tp_dirty = true;
                    newly_unlocked_tps.push(tp.Id);
                }
                if tp.Type == 1
                    && tp.FogId > 0
                    && map_helper::unlock_fog_and_siblings(
                        &mut state.conn,
                        &state.assets,
                        &mut state.player_components.map,
                        tp.FogId,
                    )
                {
                    tp_dirty = true;
                }
            }
            if tp_dirty {
                state
                    .player_components
                    .map
                    .save(&state.fs)
                    .map_err(|e| e.to_string())?;
                if !newly_unlocked_tps.is_empty() {
                    let _ = state.conn.push_msg(
                        TeleportUpdateNotify::MSG_ID,
                        &TeleportUpdateNotify {
                            Ids: newly_unlocked_tps,
                        },
                    );
                }
            }
        }
    }
    if resolved_cfg_id != 0
        && let Some(sname) = &state_name_opt
    {
        state
            .player_components
            .quest
            .info
            .set_entity_state(resolved_cfg_id, sname);
        state
            .player_components
            .quest
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
        if let Ok(cid32) = i32::try_from(resolved_cfg_id)
            && cid32 > 0
        {
            let mut events = EventQueue::new();
            {
                let mut qctx = state.quest_context(&mut events);
                let _ = quest_engine::on_entity_state_changed(&mut qctx, cid32, sname);
            }
            drain_event_queue_best_effort(&mut events, state);
        }
    }
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("ent_state:{resolved_cfg_id}"),
        i64::from(req.StateTagId),
    )
    .map_err(|e| e.to_string())?;
    Ok(ChangeEntityStateResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_change_entity_role_request(
    state: &mut State,
    req: ChangeEntityRoleRequest,
) -> Result<ChangeEntityRoleResponse, String> {
    if req.Id != 0 {
        let mut resolved_cfg_id = req.Id;
        if let Some(scene) = state.scene.as_mut() {
            let net_id_opt = if scene.net_id_map.contains_key(&req.Id) {
                resolved_cfg_id = scene
                    .find_config_id_by_net_id(req.Id)
                    .map(i64::from)
                    .unwrap_or(req.Id);
                Some(req.Id)
            } else if let Ok(cid32) = i32::try_from(req.Id) {
                scene.find_entity_net_id_by_config_id(cid32)
            } else {
                None
            };
            if let Some(net_id) = net_id_opt
                && let Some(storage) = scene.entity_mut(net_id)
            {
                storage.visible = true;
            }
        }
        let target_player = if req.PlayerId != 0 {
            req.PlayerId
        } else {
            state.player_id
        };
        pb_kv::persist_int(
            &state.fs,
            &mut state.player_components.progress,
            &format!("ent_role:{resolved_cfg_id}"),
            i64::from(target_player),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(ChangeEntityRoleResponse {
        Success: true,
        PlayerId: if req.PlayerId != 0 {
            req.PlayerId
        } else {
            state.player_id
        },
        Id: req.Id,
    })
}

pub fn handle_entity_blackboard_request(
    state: &mut State,
    req: EntityBlackboardRequest,
) -> Result<EntityBlackboardResponse, String> {
    let mut entity_key_id = req.EntityId;
    if let Some(scene) = state.scene.as_ref()
        && let Some(cid) = scene.find_config_id_by_net_id(req.EntityId)
    {
        entity_key_id = i64::from(cid);
    }
    let info = &mut state.player_components.progress.info;
    for p in &req.Params {
        if p.Key.is_empty() {
            continue;
        }
        let param_key = String::from_utf8_lossy(&p.Key);
        let key = format!("ent_bb:{entity_key_id}:{param_key}");
        match &p.Value {
            Some(BlackboardParamValueOneof::IntValue(v)) => {
                info.set_key_int(&key, i64::from(*v))
            }
            Some(BlackboardParamValueOneof::LongValue(v)) => info.set_key_int(&key, *v),
            Some(BlackboardParamValueOneof::BooleanValue(v)) => {
                info.set_key_int(&key, i64::from(*v))
            }
            Some(BlackboardParamValueOneof::StringValue(s)) => {
                info.set_key_str(&key, &String::from_utf8_lossy(s))
            }
            Some(BlackboardParamValueOneof::FloatValue(f)) => {
                info.set_key_int(&key, safe_float_to_i64(*f * 1000.0))
            }
            _ => {
                info.mark_claim(&key);
            }
        }
    }
    if let Some(scene) = state.scene.as_mut()
        && let Some(&idx) = scene.net_id_map.get(&req.EntityId)
    {
        if scene.entities[idx].fsm.is_some() {
            scene.fsm_wakes.mark_dirty(req.EntityId);
        }
        if scene.entities[idx].config.entity_type == EntityType::SceneItem
        {
            let config_id = scene.entities[idx].config.config_id;
            let mut scene_item = scene.entities[idx]
                .world_item
                .scene_item
                .unwrap_or(SceneItemComponent {
                    pos_sender: scene.player_id,
                    manipulatable_state: None,
                });
            for p in &req.Params {
                if p.Key.as_slice() != b"ManipulatableState" {
                    continue;
                }
                let item_state = match &p.Value {
                    Some(BlackboardParamValueOneof::IntValue(v)) => Some(*v),
                    Some(BlackboardParamValueOneof::LongValue(v)) => i32::try_from(*v).ok(),
                    Some(BlackboardParamValueOneof::BooleanValue(v)) => Some(i32::from(*v)),
                    None => Some(1),
                    _ => None,
                };
                if let Some(st) = item_state {
                    scene_item.manipulatable_state = Some(st);
                    if config_id > 0 {
                        info.set_key_int(&format!("item_bb:{config_id}:0"), i64::from(st));
                    }
                }
            }
            scene.entities[idx].world_item.scene_item = Some(scene_item);
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(EntityBlackboardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_update_scene_item_blackboard_request(
    state: &mut State,
    req: UpdateSceneItemBlackboardRequest,
) -> Result<UpdateSceneItemBlackboardResponse, String> {
    let Some(scene) = state.scene.as_mut() else {
        return Ok(UpdateSceneItemBlackboardResponse {
            ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
        });
    };
    let Some(&entity_index) = scene.net_id_map.get(&req.EntityId) else {
        return Ok(UpdateSceneItemBlackboardResponse {
            ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
        });
    };
    let entity_config = scene.entities[entity_index].config;
    if entity_config.entity_type != EntityType::SceneItem
        || entity_config.config_id <= 0
    {
        return Ok(UpdateSceneItemBlackboardResponse {
            ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
        });
    }
    let info = &mut state.player_components.progress.info;
    for p in &req.Params {
        let key_enum = p.Key.unwrap_or_default();
        let bb_k = key_enum.to_i32();
        let key = format!("item_bb:{}:{bb_k}", entity_config.config_id);
        let val = match &p.Value {
            Some(SceneItemBlackboardParamValueOneof::IntValue(v)) => i64::from(*v),
            Some(SceneItemBlackboardParamValueOneof::LongValue(v)) => *v,
            Some(SceneItemBlackboardParamValueOneof::BooleanValue(v)) => i64::from(*v),
            Some(SceneItemBlackboardParamValueOneof::StringValue(s)) => {
                info.set_key_str(&key, &String::from_utf8_lossy(s));
                continue;
            }
            Some(SceneItemBlackboardParamValueOneof::FloatValue(f)) => safe_float_to_i64(*f),
            _ => {
                if bb_k == 0 {
                    1
                } else {
                    i64::from(bb_k)
                }
            }
        };
        info.set_key_int(&key, val);
    }
    let mut scene_item = scene.entities[entity_index]
        .world_item
        .scene_item
        .unwrap_or(SceneItemComponent {
            pos_sender: scene.player_id,
            manipulatable_state: None,
        });
    for p in &req.Params {
        if p.Key.unwrap_or_default().to_i32() != 0 {
            continue;
        }
        scene_item.manipulatable_state = match &p.Value {
            Some(SceneItemBlackboardParamValueOneof::IntValue(v)) => Some(*v),
            Some(SceneItemBlackboardParamValueOneof::LongValue(v)) => i32::try_from(*v).ok(),
            Some(SceneItemBlackboardParamValueOneof::BooleanValue(v)) => Some(i32::from(*v)),
            None => Some(1),
            _ => scene_item.manipulatable_state,
        };
    }
    scene.entities[entity_index].world_item.scene_item = Some(scene_item);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(UpdateSceneItemBlackboardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_adsorb_request(
    state: &mut State,
    req: EntityAdsorbRequest,
) -> Result<EntityAdsorbResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("wp:adsorb:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    let ppos_opt = state
        .scene
        .as_ref()
        .and_then(|s| s.instance.players.first().map(|p| p.location));
    let entity_id = i64::from(req.EntityId);
    if let Some(ppos) = ppos_opt
        && persist_world_entity_transform(state, entity_id, ppos, None)?
    {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
        let _ = state.conn.push_msg(
            MovePackageNotify::MSG_ID,
            &MovePackageNotify {
                MovingEntities: vec![MovingEntityData {
                    EntityId: entity_id,
                    Originator: entity_id,
                    MoveInfos: vec![MoveReplaySample {
                        Location: Some(Vector {
                            X: ppos[0],
                            Y: ppos[1],
                            Z: ppos[2],
                        }),
                        ..Default::default()
                    }],
                    ForcePush: true,
                }],
            },
        );
    }
    flush_quest_condition_changed(state);
    Ok(EntityAdsorbResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_battle_info_request(
    state: &mut State,
    req: EntityBattleInfoRequest,
) -> Result<EntityBattleInfoResponse, String> {
    let mut attrs = Vec::new();
    let mut buffs_pb = None;
    if let Some(scene) = state.scene.as_ref()
        && let Some(&idx) = scene.net_id_map.get(&req.EntityId)
    {
        if let Some(attr) = &scene.entities[idx].attribute {
            for (ai, a) in attr.attributes.iter().enumerate() {
                if a.current == 0 && a.base == 0 {
                    continue;
                }
                let atype = EAttributeType::from_i32_or_default(ai as i32);
                attrs.push(GameplayAttributeData {
                    AttributeType: Some(atype),
                    BaseValue: a.base,
                    CurrentValue: a.current,
                });
            }
        }
        if let Some(buff_comp) = &scene.entities[idx].buffs {
            buffs_pb = Some(buff_comp.to_proto());
        }
    }
    Ok(EntityBattleInfoResponse {
        ErrorCode: Some(ErrorCode::Success),
        EntityId: req.EntityId,
        Attributes: attrs,
        FightBuffComponentPb: buffs_pb,
        ..Default::default()
    })
}

pub fn handle_entity_beam_receive_request(
    state: &mut State,
    req: EntityBeamReceiveRequest,
) -> Result<EntityBeamReceiveResponse, String> {
    let is_satisfied = req
        .ReceiveType
        .map(|rt| rt.to_i32() != 1) // StopAction
        .unwrap_or(true);
    let _ = state.conn.push_msg(
        BeamReceiveNotify::MSG_ID,
        &BeamReceiveNotify {
            EntityId: req.EntityId,
            IsSatisfied: is_satisfied,
        },
    );
    Ok(EntityBeamReceiveResponse {
        ReceiveType: req.ReceiveType,
        ErrorCode: Some(ErrorCode::Success),
        EntityId: req.EntityId,
    })
}

pub fn handle_entity_camera_alert_request(
    state: &mut State,
    req: EntityCameraAlertRequest,
) -> Result<EntityCameraAlertResponse, String> {
    if let Some(scene) = state.scene.as_ref() {
        let config_id = scene
            .find_config_id_by_net_id(req.EntityId)
            .or_else(|| i32::try_from(req.EntityId).ok())
            .unwrap_or(0);
        if config_id > 0
            && let Some(config) = state
                .assets
                .find_level_entity(scene.instance_id, i64::from(config_id))
            && config.AreaId > 0
        {
            pb_kv::persist_int(
                &state.fs,
                &mut state.player_components.progress,
                &format!("alert:area:{}", config.AreaId),
                i64::from(req.IsAlert),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    flush_quest_condition_changed(state);
    Ok(EntityCameraAlertResponse {
        EntityId: req.EntityId,
        ErrorCode: Some(ErrorCode::Success),
        IsAlert: req.IsAlert,
    })
}

pub fn handle_entity_patrol_add_tag_request(
    state: &mut State,
    req: EntityPatrolAddTagRequest,
) -> Result<EntityPatrolAddTagResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("patrol:tag:{}", req.EntityId),
        i64::from(req.TagId),
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut()
        && let Some(&idx) = scene.net_id_map.get(&req.EntityId)
        && req.TagId != 0
    {
        let tags = scene.entities[idx]
            .tag
            .get_or_insert_with(TagComponent::default);
        if !tags.entity_common_tags.contains(&req.TagId) {
            tags.entity_common_tags.push(req.TagId);
        }
        tags.set_tag_count(req.TagId, 1);
        if tags_handler::has_client_level_tag_component(scene, Some(&state.assets), req.EntityId) {
            let _ = state.conn.push_msg(
                EntityCommonTagNotify::MSG_ID,
                &EntityCommonTagNotify {
                    Id: req.EntityId,
                    Tags: vec![CommonTagData {
                        TagId: req.TagId,
                        RemoveTagIds: true,
                    }],
                },
            );
        }
        scene.mark_fsm_dirty(req.EntityId, wake_reason::TAG);
    }
    Ok(EntityPatrolAddTagResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_patrol_change_dir_request(
    state: &mut State,
    req: EntityPatrolChangeDirRequest,
) -> Result<EntityPatrolChangeDirResponse, String> {
    let dir_val = i64::from(req.Dir);
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("patrol:dir:{}", req.EntityId), dir_val);
    if let Some(scene) = state.scene.as_mut() {
        if let Some(cfg_id) = scene.find_config_id_by_net_id(req.EntityId) {
            state
                .player_components
                .progress
                .info
                .set_key_int(&format!("patrol:dir:{cfg_id}"), dir_val);
        }
        if let Some(&idx) = scene.net_id_map.get(&req.EntityId) {
            scene.entities[idx].world_item.patrol_dir = Some(req.Dir);
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(EntityPatrolChangeDirResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_random_interact_request(
    state: &mut State,
    req: EntityRandomInteractRequest,
) -> Result<EntityRandomInteractResponse, String> {
    let Some(scene) = state.scene.as_ref() else {
        return Ok(EntityRandomInteractResponse {
            ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            Interacting: false,
        });
    };
    if scene.active_flow_inc_id != 0 {
        return Ok(EntityRandomInteractResponse {
            ErrorCode: Some(ErrorCode::ErrInteracTreeSuspend),
            Interacting: false,
        });
    }
    let mut events = EventQueue::new();
    let handled = {
        let mut qctx = state.quest_context(&mut events);
        quest_engine::on_random_interact(&mut qctx, req.EntityId, req.OptionIndex)
            .map_err(|e| e.to_string())?
    };
    drain_event_queue_best_effort(&mut events, state);
    if !handled {
        return Ok(EntityRandomInteractResponse {
            ErrorCode: Some(ErrorCode::ErrInteractOptionGuidInvalid),
            Interacting: false,
        });
    }
    Ok(EntityRandomInteractResponse {
        ErrorCode: Some(ErrorCode::Success),
        Interacting: false,
    })
}

pub fn handle_entity_send_event_request(
    state: &mut State,
    req: EntitySendEventRequest,
) -> Result<EntitySendEventResponse, String> {
    let event_key = String::from_utf8_lossy(&req.EventKey).into_owned();
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("ent_evt:{}:{event_key}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    let mut cfg_id = req.EntityId;
    if let Some(scene) = state.scene.as_mut() {
        if let Some(cid) = scene.find_config_id_by_net_id(req.EntityId) {
            cfg_id = i64::from(cid);
        }
        if let Some(&idx) = scene.net_id_map.get(&req.EntityId)
            && scene.entities[idx].fsm.is_some()
        {
            scene.fsm_wakes.mark_dirty(req.EntityId);
        }
    }
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        if let Ok(cid32) = i32::try_from(cfg_id)
            && cid32 > 0
        {
            let _ = quest_engine::on_entity_state_changed(&mut qctx, cid32, &event_key);
        }
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(EntitySendEventResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_walking_pattern_finish_request(
    state: &mut State,
    req: EntityWalkingPatternFinishRequest,
) -> Result<EntityWalkingPatternFinishResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("walk_pat:fin:{}", req.EntityId),
        i64::from(req.Point),
    )
    .map_err(|e| e.to_string())?;
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        if let Some(mut lp_ctx) = level_play::LevelPlayContext::from_quest_ctx(&mut qctx) {
            let cfg_id = lp_ctx
                .scene
                .find_config_id_by_net_id(req.EntityId)
                .unwrap_or(req.EntityId as i32);
            let _ = level_play::on_condition_event(
                &mut lp_ctx,
                LevelPlayEventKind::WalkingPattern,
                cfg_id,
                req.Point,
            );
        }
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(EntityWalkingPatternFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_submit_walking_pattern_point_request(
    state: &mut State,
    req: SubmitWalkingPatternPointRequest,
) -> Result<SubmitWalkingPatternPointResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("walk_pat:pt:{}", req.TreeIncId),
        i64::from(req.Point),
    )
    .map_err(|e| e.to_string())?;
    if req.NodeId > 0 {
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            if let Some(mut lp_ctx) = level_play::LevelPlayContext::from_quest_ctx(&mut qctx) {
                let _ = level_play::on_submit_level_play_node(
                    &mut lp_ctx,
                    req.TreeOwnerId,
                    req.TreeIncId,
                    req.NodeId,
                );
            }
        }
        drain_event_queue_best_effort(&mut events, state);
    }
    Ok(SubmitWalkingPatternPointResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_monster_patrol_point_reached_request(
    state: &mut State,
    req: MonsterPatrolPointReachedRequest,
) -> Result<MonsterPatrolPointReachedResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("patrol:pt:{}", req.EntityId),
        i64::from(req.Index),
    )
    .map_err(|e| e.to_string())?;
    Ok(MonsterPatrolPointReachedResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_vfx_npc_patrol_finish_request(
    state: &mut State,
    req: VfxNpcPatrolFinishRequest,
) -> Result<VfxNpcPatrolFinishResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("vfx_patrol:fin:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(VfxNpcPatrolFinishResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_npc_trace_failed_request(
    state: &mut State,
    req: NpcTraceFailedRequest,
) -> Result<NpcTraceFailedResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("npc_trace:fail:{}", req.TreeIncId),
        i64::from(req.NodeId),
    )
    .map_err(|e| e.to_string())?;
    Ok(NpcTraceFailedResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

fn read_json_f32(val: &Value) -> Option<f32> {
    val.as_f64()
        .map(|f| f as f32)
        .or_else(|| val.as_i64().map(|i| i as f32))
        .or_else(|| val.as_str().and_then(|s| s.parse::<f32>().ok()))
}

fn read_json_pos3_offsets(obj: &serde_json::Map<String, Value>) -> [f32; 3] {
    [
        obj.get("X").and_then(read_json_f32).unwrap_or(0.0),
        obj.get("Y").and_then(read_json_f32).unwrap_or(0.0),
        obj.get("Z").and_then(read_json_f32).unwrap_or(0.0),
    ]
}

fn resolve_spline_endpoint_pos(
    state: &State,
    instance_id: i32,
    spline_cfg_id: i64,
) -> Option<[f32; 3]> {
    let lcfg = state.assets.find_level_entity(instance_id, spline_cfg_id)?;
    let base = entity_world_pos(lcfg.as_ref()).unwrap_or([0.0; 3]);
    let rot = entity_world_rot(lcfg.as_ref());
    let mut components = lcfg.ComponentsData.clone();
    if let Some(tpl) = state.assets.find_template_config(&lcfg.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut components);
    }
    let spline = components.SplineComponent?;
    let pts = spline.Option.as_ref()?.as_object()?.get("Points")?.as_array()?;
    let last_obj = pts.last()?.as_object()?;
    let pos_obj = last_obj.get("Position")?.as_object()?;
    let off = gravity::rotate_local_offset_ue(read_json_pos3_offsets(pos_obj), rot);
    Some([base[0] + off[0], base[1] + off[1], base[2] + off[2]])
}

pub fn handle_quest_npc_move_over_request(
    state: &mut State,
    req: QuestNpcMoveOverRequest,
) -> Result<QuestNpcMoveOverResponse, String> {
    let move_target = if let Some(scene) = state.scene.as_mut()
        && let Some(&index) = scene.net_id_map.get(&req.EntityId)
    {
        let config_id = scene.entities[index].config.config_id;
        let inst_id = scene.instance_id;
        if let Some(mtp) = scene.entities[index].world_item.move_to_point.take() {
            Some(mtp.target_pos)
        } else if let Some(ms) = scene.entities[index].world_item.move_spline.take() {
            resolve_spline_endpoint_pos(state, inst_id, i64::from(ms.spline_entity_id))
        } else if config_id > 0 {
            let spline_cfg_id = state
                .player_components
                .progress
                .info
                .get_key_int(&format!("quest_spline:{config_id}"));
            if spline_cfg_id > 0 {
                resolve_spline_endpoint_pos(state, inst_id, spline_cfg_id)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    if let Some(pos) = move_target {
        let _ = persist_world_entity_transform(state, req.EntityId, pos, None)?;
    }
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("quest_npc:over:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(QuestNpcMoveOverResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_progress_bar_finish_request(
    state: &mut State,
    req: ProgressBarFinishRequest,
) -> Result<ProgressBarFinishResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("prog_bar:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(ProgressBarFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_set_init_tag_request(
    state: &mut State,
    req: SetInitTagRequest,
) -> Result<SetInitTagResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("init_tag:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut() {
        let nid_opt = if scene.net_id_map.contains_key(&req.EntityId) {
            Some(req.EntityId)
        } else {
            i32::try_from(req.EntityId)
                .ok()
                .and_then(|cid| scene.find_entity_net_id_by_config_id(cid))
        };
        if let Some(nid) = nid_opt
            && let Some(upd) = scene.update_entity_state_tag(
                &state.assets.tables.gameplay_tag_remap,
                nid,
                "关卡.Common.状态.常态",
            )
        {
            quest_actions::emit_entity_state_tag_update_checked(
                scene,
                Some(&state.assets),
                &mut state.conn,
                nid,
                upd,
            );
        }
    }
    Ok(SetInitTagResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_set_baozi_state_request(
    state: &mut State,
    req: SetBaoziStateRequest,
) -> Result<SetBaoziStateResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("baozi:{}", req.BaoziEntityId),
        i64::from(req.IsActive),
    )
    .map_err(|e| e.to_string())?;
    Ok(SetBaoziStateResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_set_fan_number_of_turns_request(
    state: &mut State,
    req: SetFanNumberOfTurnsRequest,
) -> Result<SetFanNumberOfTurnsResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("fan_turns:{}", req.EntityId),
        i64::from(req.NumberOfTurns),
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut() {
        let idx_opt = scene.net_id_map.get(&req.EntityId).copied().or_else(|| {
            let cid = i32::try_from(req.EntityId).ok()?;
            let nid = scene.find_entity_net_id_by_config_id(cid)?;
            scene.net_id_map.get(&nid).copied()
        });
        if let Some(idx) = idx_opt {
            scene.entities[idx].world_item.fan_turns = Some(req.NumberOfTurns);
        }
    }
    Ok(SetFanNumberOfTurnsResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_set_fan_state_request(
    state: &mut State,
    req: SetFanStateRequest,
) -> Result<SetFanStateResponse, String> {
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("fan_state:{}", req.FanEntityId), i64::from(req.IsActive));
    if let Some(scene) = state.scene.as_mut() {
        let nid_opt = if scene.net_id_map.contains_key(&req.FanEntityId) {
            Some(req.FanEntityId)
        } else {
            i32::try_from(req.FanEntityId)
                .ok()
                .and_then(|cid| scene.find_entity_net_id_by_config_id(cid))
        };
        if let Some(nid) = nid_opt {
            if let Some(cfg_id) = scene.find_config_id_by_net_id(nid) {
                state
                    .player_components
                    .progress
                    .info
                    .set_key_int(&format!("fan_state:{cfg_id}"), i64::from(req.IsActive));
            }
            let state_name = if req.IsActive != 0 {
                "关卡.Common.状态.激活"
            } else {
                "关卡.Common.状态.常态"
            };
            if let Some(upd) = scene.update_entity_state_tag(
                &state.assets.tables.gameplay_tag_remap,
                nid,
                state_name,
            ) {
                quest_actions::emit_entity_state_tag_update_checked(
                    scene,
                    Some(&state.assets),
                    &mut state.conn,
                    nid,
                    upd,
                );
            }
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(SetFanStateResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_elevator_state_change_request(
    state: &mut State,
    req: ElevatorStateChangeRequest,
) -> Result<ElevatorStateChangeResponse, String> {
    let Some((instance_id, config_id)) = state
        .scene
        .as_ref()
        .and_then(|s| Some((s.instance_id, s.find_config_id_by_net_id(req.EntityId)?)))
    else {
        return Ok(ElevatorStateChangeResponse {
            ErrorCode: Some(ErrorCode::ErrElevatorEntityNotExit),
        });
    };
    if config_id <= 0 {
        return Ok(ElevatorStateChangeResponse {
            ErrorCode: Some(ErrorCode::ErrElevatorConfigNotExit),
        });
    }
    let _ = instance_id;
    let prev_location = state
        .scene
        .as_ref()
        .and_then(|s| s.net_id_map.get(&req.EntityId).copied())
        .and_then(|idx| state.scene.as_ref()?.entities[idx].world_item.lift)
        .map(|l| l.location)
        .unwrap_or(0);
    // The client never sets 关卡.电梯.* itself; it only reports Forward (moving up),
    // Reverse (moving down) and End (arrived), so the server owns the entity state.
    // `State` is a proto3 enum: the client omits the default `End` (0) on the wire, so a
    // missing field means the lift arrived.
    let elevator_state_name = match req.State {
        Some(sonoro_proto::enums::ElevatorState::Forward) => Some("关卡.电梯.上升"),
        Some(sonoro_proto::enums::ElevatorState::Reverse) => Some("关卡.电梯.下降"),
        Some(sonoro_proto::enums::ElevatorState::End) | None => Some("关卡.电梯.停靠"),
    };
    if let Some(scene) = state.scene.as_mut()
        && let Some(&idx) = scene.net_id_map.get(&req.EntityId)
    {
        scene.entities[idx].world_item.lift = Some(EntityLiftComponent {
            location: req.TagState,
        });
        if let Some(state_name) = elevator_state_name {
            scene.update_entity_state_tag(
                &state.assets.tables.gameplay_tag_remap,
                req.EntityId,
                state_name,
            );
        }
    }
    let info = &mut state.player_components.progress.info;
    info.set_key_int(&format!("elevator:{config_id}"), i64::from(req.TagState));
    info.set_key_int(&format!("lift_target:{config_id}"), i64::from(req.TagState));
    if let Some(state_name) = elevator_state_name {
        state
            .player_components
            .quest
            .info
            .set_entity_state(i64::from(config_id), state_name);
        state
            .player_components
            .quest
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    if req.State != Some(sonoro_proto::enums::ElevatorState::End)
        && req.TagState > 0
        && prev_location != req.TagState
    {
        let _ = state.conn.push_msg(
            ElevatorMoveNotify::MSG_ID,
            &ElevatorMoveNotify {
                EntityId: req.EntityId,
                Location: req.TagState,
                ..Default::default()
            },
        );
    }
    Ok(ElevatorStateChangeResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_hardness_mode_changed_request(
    state: &mut State,
    req: HardnessModeChangedRequest,
) -> Result<HardnessModeChangedResponse, String> {
    if let Some(scene) = state.scene.as_mut()
        && let Some(handle) = scene.entity_handle(req.EntityId)
        && let Some(attr) = scene.entities[handle.index].attribute.as_mut()
    {
        attr.hardness_mode_id = req.HardnessModeId;
    }
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("hardness:{}", req.EntityId),
        i64::from(req.HardnessModeId),
    )
    .map_err(|e| e.to_string())?;
    Ok(HardnessModeChangedResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_scene_item_state_request(
    state: &mut State,
    req: SceneItemStateRequest,
) -> Result<SceneItemStateResponse, String> {
    let cfg_id = state
        .scene
        .as_ref()
        .and_then(|s| s.find_config_id_by_net_id(req.EntityId).map(i64::from))
        .unwrap_or(req.EntityId);
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("scene_item_state:{cfg_id}"),
        i64::from(req.OperateType),
    )
    .map_err(|e| e.to_string())?;
    Ok(SceneItemStateResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_scene_item_move_spline_request(
    state: &mut State,
    req: SceneItemMoveSplineRequest,
) -> Result<SceneItemMoveSplineResponse, String> {
    let (instance_id, index, config_id) = {
        let Some(scene) = state.scene.as_ref() else {
            return Ok(SceneItemMoveSplineResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            });
        };
        let Some(&index) = scene.net_id_map.get(&req.EntityId) else {
            return Ok(SceneItemMoveSplineResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            });
        };
        let config_id = scene.entities[index].config.config_id;
        if config_id <= 0 {
            return Ok(SceneItemMoveSplineResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            });
        }
        (scene.instance_id, index, config_id)
    };
    if scene_item_movement_disabled(state, instance_id, config_id) {
        return Ok(SceneItemMoveSplineResponse {
            ErrorCode: Some(ErrorCode::ErrSceneItemOperate),
        });
    }

    let status = req.MoveStatus.unwrap_or(MoveStatus::Moving);
    let spline_active = status == MoveStatus::Moving || status == MoveStatus::Switch;
    if spline_active && req.SplineEntityId <= 0 {
        return Ok(SceneItemMoveSplineResponse {
            ErrorCode: Some(ErrorCode::ErrSplineEntityIdNotExist),
        });
    }
    if let Some(runtime) = &req.RuntimeData {
        if let Some(pos) = &runtime.CurPos
            && !finite_vec3([pos.X, pos.Y, pos.Z])
        {
            return Ok(SceneItemMoveSplineResponse {
                ErrorCode: Some(ErrorCode::ErrSceneItemState),
            });
        }
        if let Some(SceneItemSplineRuntimeDataDistanceOneof::DistanceAlongPath(val)) =
            &runtime.Distance
            && !val.is_finite()
        {
            return Ok(SceneItemMoveSplineResponse {
                ErrorCode: Some(ErrorCode::ErrSceneItemState),
            });
        }
        if let Some(SceneItemSplineRuntimeDataRotOneof::CurRot(rot)) = &runtime.Rot
            && !finite_vec3([rot.Roll, rot.Pitch, rot.Yaw])
        {
            return Ok(SceneItemMoveSplineResponse {
                ErrorCode: Some(ErrorCode::ErrSceneItemState),
            });
        }
    }

    if let Some(scene) = state.scene.as_mut() {
        scene.entities[index].world_item.move_spline = if spline_active {
            Some(MoveSplineComponent {
                move_status: Some(status),
                spline_entity_id: req.SplineEntityId,
                runtime_data: req.RuntimeData.clone(),
                move_spline_config: req.MoveSplineConfig.clone(),
            })
        } else {
            None
        };
    }

    if let Some(runtime) = &req.RuntimeData
        && let Some(pos) = &runtime.CurPos
    {
        let rotation = runtime.Rot.as_ref().map(|r| match r {
            SceneItemSplineRuntimeDataRotOneof::CurRot(v) => v.clone(),
        });
        let _ =
            persist_world_entity_transform(state, req.EntityId, [pos.X, pos.Y, pos.Z], rotation)?;
    }

    let info = &mut state.player_components.progress.info;
    info.set_key_int(
        &format!("scene_item_spline_cfg:{config_id}"),
        i64::from(req.SplineEntityId),
    );
    info.set_key_int(
        &format!("scene_item_spline_status:{config_id}"),
        i64::from(status.to_i32()),
    );
    info.set_key_int(
        &format!("scene_item_spline_active:{config_id}"),
        i64::from(spline_active),
    );
    if let Some(cfg) = &req.MoveSplineConfig {
        spline_state::persist_move_spline_config(info, config_id, Some(cfg));
    }
    info.set_key_int(&format!("quest_spline_active:{config_id}"), 0);
    info.set_key_int(&format!("vehicle:spline_active:{config_id}"), 0);
    info.set_key_int(&format!("scene_item_move_target_active:{config_id}"), 0);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(SceneItemMoveSplineResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_scene_item_move_target_request(
    state: &mut State,
    req: SceneItemMoveTargetRequest,
) -> Result<SceneItemMoveTargetResponse, String> {
    let Some(scene_ref) = state.scene.as_ref() else {
        return Ok(SceneItemMoveTargetResponse {
            ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
        });
    };
    let Some(mi) = &req.MoveInfo else {
        return Ok(SceneItemMoveTargetResponse {
            ErrorCode: Some(ErrorCode::ErrSceneItemState),
        });
    };
    let Some(loc) = &mi.Location else {
        return Ok(SceneItemMoveTargetResponse {
            ErrorCode: Some(ErrorCode::ErrSceneItemState),
        });
    };
    let target_pos = [loc.X, loc.Y, loc.Z];
    if !finite_vec3(target_pos)
        || !mi.MoveTime.is_finite()
        || !mi.StayTime.is_finite()
        || !mi.MaxSpeed.is_finite()
        || !mi.Acceleration.is_finite()
    {
        return Ok(SceneItemMoveTargetResponse {
            ErrorCode: Some(ErrorCode::ErrSceneItemState),
        });
    }
    let (instance_id, index, config_id) = {
        let Some(&index) = scene_ref.net_id_map.get(&mi.EntityId) else {
            return Ok(SceneItemMoveTargetResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            });
        };
        let config_id = scene_ref.entities[index].config.config_id;
        if config_id <= 0 {
            return Ok(SceneItemMoveTargetResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            });
        }
        (scene_ref.instance_id, index, config_id)
    };
    if scene_item_movement_disabled(state, instance_id, config_id) {
        return Ok(SceneItemMoveTargetResponse {
            ErrorCode: Some(ErrorCode::ErrSceneItemOperate),
        });
    }
    if let Some(scene) = state.scene.as_mut() {
        scene.entities[index].world_item.move_to_point = Some(MoveToPointComponent {
            target_pos,
            move_type: 0,
            is_exact: false,
            move_time: mi.MoveTime,
            stay_time: mi.StayTime,
            max_speed: mi.MaxSpeed,
            acceleration: mi.Acceleration,
        });
    }
    let info = &mut state.player_components.progress.info;
    let fields = [
        ("x", target_pos[0]),
        ("y", target_pos[1]),
        ("z", target_pos[2]),
        ("move_time", mi.MoveTime),
        ("stay_time", mi.StayTime),
        ("max_speed", mi.MaxSpeed),
        ("acceleration", mi.Acceleration),
    ];
    for (field, value) in fields {
        info.set_key_int(
            &format!("scene_item_move_target:{config_id}:{field}"),
            i64::from(value.to_bits()),
        );
    }
    info.set_key_int(&format!("scene_item_move_target_active:{config_id}"), 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(SceneItemMoveTargetResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_scene_item_sequence_frame_start_request(
    state: &mut State,
    req: SceneItemSequenceFrameStartRequest,
) -> Result<SceneItemSequenceFrameStartResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("seq_frame:start:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(SceneItemSequenceFrameStartResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_scene_item_sequence_frame_end_request(
    state: &mut State,
    req: SceneItemSequenceFrameEndRequest,
) -> Result<SceneItemSequenceFrameEndResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("seq_frame:end:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(SceneItemSequenceFrameEndResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_scene_item_stop_request(
    state: &mut State,
    req: SceneItemStopRequest,
) -> Result<SceneItemStopResponse, String> {
    let mut changed = false;
    for si in &req.StopInfos {
        if let Some(scene) = state.scene.as_mut()
            && let Some(&index) = scene.net_id_map.get(&si.EntityId)
        {
            let config_id = scene.entities[index].config.config_id;
            scene.entities[index].world_item.move_spline = None;
            scene.entities[index].world_item.move_to_point = None;
            if config_id > 0 {
                let info = &mut state.player_components.progress.info;
                info.set_key_int(&format!("scene_item_spline_active:{config_id}"), 0);
                info.set_key_int(
                    &format!("scene_item_spline_status:{config_id}"),
                    i64::from(MoveStatus::Stop.to_i32()),
                );
                info.set_key_int(&format!("scene_item_move_target_active:{config_id}"), 0);
                info.set_key_int(&format!("quest_spline_active:{config_id}"), 0);
                info.set_key_int(&format!("vehicle:spline_active:{config_id}"), 0);
                changed = true;
            }
        }
        if let Some(loc) = &si.Location
            && persist_world_entity_transform(state, si.EntityId, [loc.X, loc.Y, loc.Z], None)?
        {
            changed = true;
        }
    }
    if changed {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(SceneItemStopResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_move_scene_item_reset_position_request(
    state: &mut State,
    req: MoveSceneItemResetPositionRequest,
) -> Result<MoveSceneItemResetPositionResponse, String> {
    let reset_pos = if let Some(scene) = state.scene.as_mut()
        && let Some(&idx) = scene.net_id_map.get(&req.EntityId)
    {
        let cfg_id = scene.entities[idx].config.config_id;
        let inst_id = scene.instance_id;
        scene.entities[idx].world_item.move_spline = None;
        scene.entities[idx].world_item.move_to_point = None;
        if cfg_id > 0 {
            let info = &mut state.player_components.progress.info;
            info.set_key_int(&format!("scene_item_spline_active:{cfg_id}"), 0);
            info.set_key_int(
                &format!("scene_item_spline_status:{cfg_id}"),
                i64::from(MoveStatus::Stop.to_i32()),
            );
            info.set_key_int(&format!("scene_item_move_target_active:{cfg_id}"), 0);
            info.set_key_int(&format!("quest_spline_active:{cfg_id}"), 0);
            info.set_key_int(&format!("vehicle:spline_active:{cfg_id}"), 0);
        }
        state
            .assets
            .find_level_entity(inst_id, i64::from(cfg_id))
            .and_then(|ecfg| entity_world_pos(ecfg.as_ref()))
    } else {
        None
    };
    if let Some(pos) = reset_pos {
        let _ = persist_world_entity_transform(state, req.EntityId, pos, None)?;
    }
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("item_reset:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(MoveSceneItemResetPositionResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_move_placement_request(
    state: &mut State,
    req: MovePlacementRequest,
) -> Result<MovePlacementResponse, String> {
    if let Some(pr) = &req.PosRot
        && let Some(loc) = &pr.Pos
        && persist_world_entity_transform(
            state,
            req.PlacementEntityId,
            [loc.X, loc.Y, loc.Z],
            pr.Rot.clone(),
        )?
    {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("placement:{}", req.PlacementEntityId),
        req.TargetBoardEntityId,
    )
    .map_err(|e| e.to_string())?;
    let _ = state.conn.push_msg(
        MovePlacementNotify::MSG_ID,
        &MovePlacementNotify {
            TargetBoardEntityId: req.TargetBoardEntityId,
            PlacementEntityId: req.PlacementEntityId,
            PosRot: req.PosRot,
            ..Default::default()
        },
    );
    flush_quest_condition_changed(state);
    Ok(MovePlacementResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_spline_move_request(
    state: &mut State,
    req: SplineMoveRequest,
) -> Result<SplineMoveResponse, String> {
    if let Some(loc) = &req.Pos
        && persist_world_entity_transform(state, req.EntityId, [loc.X, loc.Y, loc.Z], None)?
    {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("spline:path:{}", req.EntityId),
        i64::from(req.PathId),
    )
    .map_err(|e| e.to_string())?;
    Ok(SplineMoveResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_spline_position_update_request(
    state: &mut State,
    req: SplinePositionUpdateRequest,
) -> Result<SplinePositionUpdateResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("spline:idx:{}", req.EntityId),
        i64::from(req.SplineIdx),
    )
    .map_err(|e| e.to_string())?;
    Ok(SplinePositionUpdateResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_stop_move_spline_request(
    state: &mut State,
    req: StopMoveSplineRequest,
) -> Result<StopMoveSplineResponse, String> {
    state.player_components.progress.info.set_key_int(
        &format!("spline:stop:{}", req.EntityId),
        i64::from(req.SplineEntityId),
    );
    let end_pos_opt = if let Some(scene) = state.scene.as_ref() {
        let spline_cfg_id = scene
            .find_config_id_by_net_id(i64::from(req.SplineEntityId))
            .unwrap_or(req.SplineEntityId);
        resolve_spline_endpoint_pos(state, scene.instance_id, i64::from(spline_cfg_id))
    } else {
        None
    };
    if let Some(end_pos) = end_pos_opt {
        let _ = persist_world_entity_transform(state, req.EntityId, end_pos, None)?;
    }
    if let Some(scene) = state.scene.as_mut()
        && let Some(&index) = scene.net_id_map.get(&req.EntityId)
    {
        let config_id = scene.entities[index].config.config_id;
        if config_id > 0 {
            let info = &mut state.player_components.progress.info;
            let active_matches = (info.get_key_int(&format!("vehicle:spline_active:{config_id}"))
                == 1
                && info.get_key_int(&format!("vehicle:spline:{config_id}"))
                    == i64::from(req.SplineEntityId))
                || (info.get_key_int(&format!("quest_spline_active:{config_id}")) == 1
                    && info.get_key_int(&format!("quest_spline:{config_id}"))
                        == i64::from(req.SplineEntityId))
                || (info.get_key_int(&format!("scene_item_spline_active:{config_id}")) == 1
                    && info.get_key_int(&format!("scene_item_spline_cfg:{config_id}"))
                        == i64::from(req.SplineEntityId));
            if active_matches {
                info.set_key_int(&format!("vehicle:spline_active:{config_id}"), 0);
                info.set_key_int(&format!("quest_spline_active:{config_id}"), 0);
                info.set_key_int(&format!("scene_item_spline_active:{config_id}"), 0);
                info.set_key_int(&format!("scene_item_move_target_active:{config_id}"), 0);
                info.set_key_int(
                    &format!("scene_item_spline_status:{config_id}"),
                    i64::from(MoveStatus::Stop.to_i32()),
                );
                scene.entities[index].world_item.move_spline = None;
                scene.entities[index].world_item.move_to_point = None;
            }
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(StopMoveSplineResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_gravity_flip_interact_request(
    state: &mut State,
    req: GravityFlipInteractRequest,
) -> Result<GravityFlipInteractResponse, String> {
    let Some(target) = req.TargetGravityFlipType else {
        return Ok(GravityFlipInteractResponse {
            ErrorCode: Some(ErrorCode::GravityFlipTypeNotExist),
            ..Default::default()
        });
    };
    let raw_eid = req.GravityFlipEntityId;
    if raw_eid <= 0 {
        return Ok(GravityFlipInteractResponse {
            ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            ..Default::default()
        });
    }
    let (instance_id, net_id, config_id) = {
        let Some(scene) = state.scene.as_ref() else {
            return Ok(GravityFlipInteractResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
                ..Default::default()
            });
        };
        if let Some(&idx) = scene.net_id_map.get(&raw_eid) {
            (
                scene.instance_id,
                raw_eid,
                scene.entities[idx].config.config_id,
            )
        } else if let Some(cid32) = i32::try_from(raw_eid).ok()
            && let Some(nid) = scene.find_entity_net_id_by_config_id(cid32)
        {
            (scene.instance_id, nid, cid32)
        } else {
            return Ok(GravityFlipInteractResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
                ..Default::default()
            });
        }
    };

    let cur_val = i64::from(target.to_i32());
    let grav_vec: [f32; 3] = match target {
        GravityFlipType::GravityUp => [0.0, 0.0, 1.0],
        GravityFlipType::GravityDown => [0.0, 0.0, -1.0],
        GravityFlipType::GravityLeft => [-1.0, 0.0, 0.0],
        GravityFlipType::GravityRight => [1.0, 0.0, 0.0],
    };
    if let Some(scene) = state.scene.as_mut() {
        if let Some(&idx) = scene.net_id_map.get(&net_id) {
            scene.entities[idx].world_item.gravity_flip = Some(cur_val as i32);
        }
        scene.set_current_role_gravity(grav_vec);
    }
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("gravity:{config_id}"),
        cur_val,
    )
    .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    let _ = state.conn.push_msg(
        GravityFlipUpdateTypeNotify::MSG_ID,
        &GravityFlipUpdateTypeNotify {
            NewGravityFlipType: Some(target),
            EntityId: net_id,
        },
    );

    if let Some(lcfg) = state
        .assets
        .find_level_entity(instance_id, i64::from(config_id))
    {
        let mut components = lcfg.ComponentsData.clone();
        if let Some(tpl) = state.assets.find_template_config(&lcfg.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut components);
        }
        if let Some(gfc) = components.GravityFlipComponent {
            let target_1based = target.to_i32() + 1;
            for entry in gfc.Config {
                if entry.Type == target_1based
                    && entry.LocationEntityId > 0
                {
                    let loc_id = entry.LocationEntityId;
                    if let Some(loc_cfg) = state.assets.find_level_entity(instance_id, loc_id)
                        && let Some(pos) = entity_world_pos(loc_cfg.as_ref())
                    {
                        let rot = entity_world_rot(loc_cfg.as_ref());
                        let map_id = state.assets.resolve_map_config_id(instance_id);
                        let mut events = EventQueue::new();
                        {
                            let mut qctx = state.quest_context(&mut events);
                            let _ = quest_actions::update_player_scene_state(&mut qctx, pos, Some(rot));
                        }
                        drain_event_queue_best_effort(&mut events, state);
                        let _ = state.conn.push_msg(
                            TeleportNotify::MSG_ID,
                            &TeleportNotify {
                                MapId: map_id,
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
                                Gravity: Some(Vector {
                                    X: grav_vec[0],
                                    Y: grav_vec[1],
                                    Z: grav_vec[2],
                                }),
                                Reason: Some(TeleportReason::Action),
                                ..Default::default()
                            },
                        );
                    }
                    break;
                }
            }
        }
    }
    Ok(GravityFlipInteractResponse {
        CurGravityFlipType: Some(target),
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_gravity_hook_lock_point_change_gravity_request(
    state: &mut State,
    req: GravityHookLockPointChangeGravityRequest,
) -> Result<GravityHookLockPointChangeGravityResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("gravity_hook:chg:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    let resolved_grav = if let Some(scene) = state.scene.as_ref() {
        let idx_opt = scene.net_id_map.get(&req.EntityId).copied().or_else(|| {
            let cid = i32::try_from(req.EntityId).ok()?;
            let nid = scene.find_entity_net_id_by_config_id(cid)?;
            scene.net_id_map.get(&nid).copied()
        });
        if let Some(idx) = idx_opt {
            let cfg_id = i64::from(scene.entities[idx].config.config_id);
            let rot = scene.entities[idx]
                .position
                .map(|p| p.rotation)
                .unwrap_or([0.0; 3]);
            Some(gravity::resolve_hook_gravity(
                &state.assets,
                scene.instance_id,
                cfg_id,
                rot,
            ))
        } else if req.EntityId > 0 {
            Some(gravity::resolve_hook_gravity(
                &state.assets,
                scene.instance_id,
                req.EntityId,
                [0.0; 3],
            ))
        } else {
            None
        }
    } else {
        None
    };
    if let Some(grav) = resolved_grav
        && grav.configured
    {
        if let Some(scene) = state.scene.as_mut() {
            scene.set_current_role_gravity(grav.dir);
        }
        flush_quest_condition_changed(state);
    }
    Ok(GravityHookLockPointChangeGravityResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn resolve_hook_interact_config(
    state: &State,
    map_id: i32,
    cfg_id: i64,
) -> Option<HookInteractConfig> {
    let level_cfg = state.assets.find_level_entity(map_id, cfg_id)?;
    let mut components = level_cfg.ComponentsData.clone();
    if let Some(tpl_cfg) = state.assets.find_template_config(&level_cfg.BlueprintType) {
        tpl_cfg.ComponentsData.merge_into(&mut components);
    }
    for entry in components.extra.into_iter().chain(components.template_extra) {
        if let ExtraEntry::HookLockPoint(hlp) = entry
            && let Some(cfg) = hlp.HookInteractConfig
        {
            return Some(cfg);
        }
    }
    None
}

fn handle_hook_lock_point_action(
    state: &mut State,
    entity_id: i64,
    action_type: HookInteractActionType,
) -> Result<(), String> {
    let Some((instance_id, cfg_id, player_id)) = state.scene.as_ref().and_then(|scene| {
        let &idx = scene.net_id_map.get(&entity_id)?;
        let cfg_id = scene.entities[idx].config.config_id;
        if cfg_id <= 0 {
            return None;
        }
        Some((scene.instance_id, cfg_id, scene.player_id))
    }) else {
        return Ok(());
    };
    let Some(hook_cfg) = resolve_hook_interact_config(state, instance_id, i64::from(cfg_id)) else {
        return Ok(());
    };
    let actions_opt = match action_type {
        HookInteractActionType::Hooked => hook_cfg.HookActions,
        HookInteractActionType::ExitEndpoint => hook_cfg.FinishActions,
        HookInteractActionType::ExitMidway => hook_cfg.ExitHookActions,
    };
    let Some(actions) = actions_opt else {
        return Ok(());
    };
    if actions.is_empty() {
        return Ok(());
    }
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        quest_engine::triggers::execute_entity_actions(&mut qctx, entity_id, cfg_id, &actions)
            .map_err(|e| e.to_string())?;
    }
    drain_event_queue_best_effort(&mut events, state);
    let inc_id = state
        .scene
        .as_mut()
        .map(|s| s.alloc_action_inc_id())
        .unwrap_or(0);
    let _ = state.conn.push_msg(
        ActionNotify::MSG_ID,
        &ActionNotify {
            GameCtx: Some(GameCtxPb {
                CtxType: Some(GameCtxType::HookLockPointActionCtx),
                CtxInfo: Some(GameCtxPbCtxInfoOneof::HookLockPointAction(
                    HookLockPointActionCtxPb {
                        EntityCtx: Some(EntityCtxPb {
                            ConfigId: cfg_id,
                            IncId: entity_id,
                        }),
                        InteractionType: Some(action_type),
                    },
                )),
            }),
            TotalCount: actions.len() as i32,
            PlayerId: player_id,
            StartIndex: 0,
            EndIndex: (actions.len() - 1) as i32,
            NeedFinishReq: false,
            IncId: inc_id,
        },
    );
    Ok(())
}

pub fn handle_hook_lock_point_request(
    state: &mut State,
    req: HookLockPointRequest,
) -> Result<HookLockPointResponse, String> {
    let target_id = state
        .scene
        .as_ref()
        .and_then(|s| s.find_config_id_by_net_id(req.EntityId).map(i64::from))
        .unwrap_or(req.EntityId);
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("hook_point:active:{target_id}"), 1);
    if target_id != req.EntityId {
        state
            .player_components
            .progress
            .info
            .set_key_int(&format!("hook_point:active:{}", req.EntityId), 1);
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    handle_hook_lock_point_action(state, req.EntityId, HookInteractActionType::Hooked)?;
    Ok(HookLockPointResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_hook_lock_point_finish_request(
    state: &mut State,
    req: HookLockPointFinishRequest,
) -> Result<HookLockPointFinishResponse, String> {
    let target_id = state
        .scene
        .as_ref()
        .and_then(|s| s.find_config_id_by_net_id(req.EntityId).map(i64::from))
        .unwrap_or(req.EntityId);
    state
        .player_components
        .progress
        .info
        .mark_claim(&format!("hook_point:fin:{target_id}"));
    if target_id != req.EntityId {
        state
            .player_components
            .progress
            .info
            .mark_claim(&format!("hook_point:fin:{}", req.EntityId));
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    handle_hook_lock_point_action(state, req.EntityId, HookInteractActionType::ExitEndpoint)?;
    Ok(HookLockPointFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_hook_lock_point_exit_request(
    state: &mut State,
    req: HookLockPointExitRequest,
) -> Result<HookLockPointExitResponse, String> {
    let target_id = state
        .scene
        .as_ref()
        .and_then(|s| s.find_config_id_by_net_id(req.EntityId).map(i64::from))
        .unwrap_or(req.EntityId);
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("hook_point:active:{target_id}"), 0);
    if target_id != req.EntityId {
        state
            .player_components
            .progress
            .info
            .set_key_int(&format!("hook_point:active:{}", req.EntityId), 0);
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    handle_hook_lock_point_action(state, req.EntityId, HookInteractActionType::ExitMidway)?;
    Ok(HookLockPointExitResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn resolve_and_grant_entity_collect_or_capture(
    state: &mut State,
    is_capture: bool,
    net_id: i64,
    out_removes: &mut Vec<EntityRemoveInfo>,
) -> Result<i32, String> {
    let mut cfg_id = 0i64;
    let mut map_id = 8i32;
    let mut vision_id = 0i32;
    if let Some(scene) = state.scene.as_ref() {
        map_id = scene.instance_id;
        if let Some(&idx) = scene.net_id_map.get(&net_id) {
            if let Some(mc) = &scene.entities[idx].world_item.monster_capture {
                if mc.entity_id > 0 {
                    cfg_id = i64::from(mc.entity_id);
                }
                if mc.monster_id > 0 {
                    vision_id = mc.monster_id;
                }
            } else {
                cfg_id = i64::from(scene.entities[idx].config.config_id);
            }
        } else if let Some(cid) = scene.find_config_id_by_net_id(net_id) {
            cfg_id = i64::from(cid);
        }
    }
    if cfg_id <= 0 && net_id > 0 && state.assets.find_level_entity(map_id, net_id).is_some() {
        cfg_id = net_id;
    }

    let mut reward_id = 0i32;
    let mut tb_type = 0i32;
    if cfg_id > 0
        && let Some(resolved) = level_play::resolve_entity_config(&state.assets, map_id, cfg_id)
    {
        if let Some(rc) = &resolved.components.RewardComponent {
            reward_id = rc.RewardId.unwrap_or(0);
        }
        if vision_id <= 0
            && let Some(vc) = &resolved.components.VisionCaptureComponent
        {
            vision_id = vc.VisionCaptureId.unwrap_or(0);
        }
        if let Some(tb) = &resolved.components.TreasureBoxComponent {
            tb_type = tb.TypeId.unwrap_or(0);
        }
    }

    let calabash_lvl = if is_capture {
        condition::current_calabash_level(
            &state.assets,
            &state.player_components.basic,
            &state.player_components.echo,
        )
    } else {
        0
    };
    {
        let mut rctx = state.reward_context();
        if is_capture {
            let _ = reward::grant_echo_capture_only(&mut rctx, vision_id, calabash_lvl)
                .map_err(|e| e.to_string())?;
        } else if tb_type > 0 {
            let _ = reward::grant_treasure_box_reward(&mut rctx, tb_type, reward_id, 7000)
                .map_err(|e| e.to_string())?;
        } else if reward_id > 0 {
            let res = reward::grant_drop_package(&mut rctx, reward_id, 1)
                .map_err(|e| e.to_string())?;
            if res.granted_count == 0 {
                reward::grant_items(
                    &mut rctx,
                    0,
                    true,
                    &[RewardItem {
                        item_id: 2,
                        count: 150,
                    }],
                    1,
                )
                .map_err(|e| e.to_string())?;
            }
        } else {
            reward::grant_items(
                &mut rctx,
                0,
                true,
                &[RewardItem {
                    item_id: 2,
                    count: 150,
                }],
                1,
            )
            .map_err(|e| e.to_string())?;
        }
    }

    if let Some(scene) = state.scene.as_mut()
        && scene.net_id_map.contains_key(&net_id)
    {
        if is_capture {
            // Keep capture phantom entity valid until CaptureBatchEntityResponse / CaptureEntityResponse
            // arrives so client ExecuteCapture() plays the gourd absorb animation cleanly (~3s).
            scene.queue_entity_removal_at(
                net_id,
                sonoro_gamesv_logic::scene::now_millis() + 3500,
                sonoro_proto::enums::ERemoveEntityType::RemoveTypeCapture,
            );
        } else {
            scene.remove(&state.fs, net_id).map_err(|e| e.to_string())?;
            out_removes.push(EntityRemoveInfo {
                EntityId: net_id,
                Type: Some(sonoro_proto::enums::ERemoveEntityType::RemoveTypeNormal),
            });
        }
    }
    Ok(cfg_id.clamp(0, i64::from(i32::MAX)) as i32)
}

pub fn handle_hook_lock_batch_collect_request(
    state: &mut State,
    req: HookLockBatchCollectRequest,
) -> Result<HookLockBatchCollectResponse, String> {
    let mut success_cfgs = Vec::new();
    let mut remove_infos = Vec::new();
    let mut dirty = false;

    if !req.BatchIds.is_empty() {
        for bid in req.BatchIds {
            let key = format!("hook_collect:{bid}");
            if state.player_components.progress.info.has_claim(&key) {
                continue;
            }
            let cfg32 =
                resolve_and_grant_entity_collect_or_capture(state, false, bid, &mut remove_infos)?;
            state.player_components.progress.info.mark_claim(&key);
            dirty = true;
            success_cfgs.push(cfg32);
        }
    } else if req.EntityId > 0 {
        let key = format!("hook_collect:{}", req.EntityId);
        if !state.player_components.progress.info.has_claim(&key) {
            let cfg32 = resolve_and_grant_entity_collect_or_capture(
                state,
                false,
                req.EntityId,
                &mut remove_infos,
            )?;
            state.player_components.progress.info.mark_claim(&key);
            dirty = true;
            success_cfgs.push(cfg32);
        }
    }
    if dirty {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    if !remove_infos.is_empty() {
        let _ = state.conn.push_msg(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                IsRemove: true,
                RemoveInfos: remove_infos,
            },
        );
    }
    let main_cfg = state
        .scene
        .as_ref()
        .and_then(|s| {
            s.find_config_id_by_net_id(req.EntityId).or_else(|| {
                state
                    .assets
                    .find_level_entity(s.instance_id, req.EntityId)
                    .map(|_| req.EntityId.clamp(0, i64::from(i32::MAX)) as i32)
            })
        })
        .unwrap_or(0);
    Ok(HookLockBatchCollectResponse {
        ErrorCode: Some(ErrorCode::Success),
        EntityConfigId: main_cfg,
        SuccessConfigIds: success_cfgs,
    })
}

pub fn handle_connector_can_connect_request(
    state: &mut State,
    req: ConnectorCanConnectRequest,
) -> Result<ConnectorCanConnectResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("connector:can:{}", req.EntityId),
        i64::from(req.IsEnter),
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut()
        && let Some(&idx) = scene.net_id_map.get(&req.EntityId)
    {
        scene.entities[idx].world_item.connector_initiated = Some(req.IsEnter);
    }
    flush_quest_condition_changed(state);
    Ok(ConnectorCanConnectResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_connector_connect_entity_request(
    state: &mut State,
    req: ConnectorConnectEntityRequest,
) -> Result<ConnectorConnectEntityResponse, String> {
    if let Some(ci) = req.ConnectEntityInfo {
        pb_kv::persist_claim(
            &state.fs,
            &mut state.player_components.progress,
            &format!("connector:link:{}", ci.SrcEntityId),
        )
        .map_err(|e| e.to_string())?;
        let _ = state.conn.push_msg(
            ConnectorConnectEntityNotify::MSG_ID,
            &ConnectorConnectEntityNotify {
                ConnectEntityInfos: vec![ci],
            },
        );
    }
    Ok(ConnectorConnectEntityResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_be_controlled_request(
    state: &mut State,
    req: BeControlledRequest,
) -> Result<BeControlledResponse, String> {
    let ctrl_val = i64::from(req.IsControl);
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("be_ctrl:active:{}", req.EntityId), ctrl_val);
    if let Some(scene) = state.scene.as_mut() {
        if let Some(cfg_id) = scene.find_config_id_by_net_id(req.EntityId) {
            state
                .player_components
                .progress
                .info
                .set_key_int(&format!("be_ctrl:active:{cfg_id}"), ctrl_val);
        }
        let cur_role_nid = scene.current_role_entity_id();
        if let Some(&idx) = scene.net_id_map.get(&req.EntityId) {
            scene.entities[idx].world_item.be_controlled_player_id =
                req.IsControl.then_some(cur_role_nid);
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(BeControlledResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_be_controlled_show_request(
    state: &mut State,
    req: BeControlledShowRequest,
) -> Result<BeControlledShowResponse, String> {
    let show_val = i64::from(req.IsShow);
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("be_ctrl:show:{}", req.EntityId), show_val);
    if let Some(scene) = state.scene.as_mut() {
        if let Some(cfg_id) = scene.find_config_id_by_net_id(req.EntityId) {
            state
                .player_components
                .progress
                .info
                .set_key_int(&format!("be_ctrl:show:{cfg_id}"), show_val);
        }
        if let Some(&idx) = scene.net_id_map.get(&req.EntityId) {
            scene.entities[idx].world_item.be_controlled_show = req.IsShow;
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(BeControlledShowResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_be_controlled_entity_hover_request(
    state: &mut State,
    req: BeControlledEntityHoverRequest,
) -> Result<BeControlledEntityHoverResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("be_ctrl:hover:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(BeControlledEntityHoverResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_time_flow_push(
    state: &mut State,
    push: EntityTimeFlowPush,
) -> Result<(), String> {
    if push.TimeDilation.is_finite() {
        let milli = (push.TimeDilation.clamp(-1000.0, 1000.0) * 1000.0) as i64;
        pb_kv::persist_int(
            &state.fs,
            &mut state.player_components.progress,
            &format!("ent:time_flow:{}", push.EntityId),
            milli,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn handle_scene_mode_change_finish_request(
    state: &mut State,
    _req: SceneModeChangeFinishRequest,
) -> Result<SceneModeChangeFinishResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        "scene:mode_fin:1",
    )
    .map_err(|e| e.to_string())?;
    Ok(SceneModeChangeFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_be_controlled_throw_request(
    state: &mut State,
    req: BeControlledThrowRequest,
) -> Result<BeControlledThrowResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("be_ctrl:throw:{}", req.EntityId),
        i64::from(req.ThrowType),
    )
    .map_err(|e| e.to_string())?;
    Ok(BeControlledThrowResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_be_controlled_destroy_request(
    state: &mut State,
    req: BeControlledDestroyRequest,
) -> Result<BeControlledDestroyResponse, String> {
    if let Some(scene) = state.scene.as_mut() {
        let mut cpack = Vec::new();
        if scene.set_entity_visibility(req.EntityId, false, &mut cpack) && !cpack.is_empty() {
            let _ = state.conn.push_msg(
                sonoro_proto::pb::CombatReceivePackNotify::MSG_ID,
                &sonoro_proto::pb::CombatReceivePackNotify { Data: cpack },
            );
        }
    }
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("be_ctrl:destroy:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(BeControlledDestroyResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_combine_entities_request(
    state: &mut State,
    req: CombineEntitiesRequest,
) -> Result<CombineEntitiesResponse, String> {
    if let Some(scene) = state.scene.as_mut()
        && let Some(info) = req.CombinePartInfo
        && let (Some(pos), Some(rot)) = (info.OffsetPos, info.OffsetRotate)
    {
        scene
            .register_combine_relation(
                info.CombinerEntityId,
                sonoro_gamesv_logic::scene::CombineRelation {
                    target_entity_id: req.TargetEntity,
                    part_index: info.PartId,
                    position: pos,
                    rotation: rot,
                },
            )
            .map_err(str::to_string)?;
    }
    Ok(CombineEntitiesResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_dissolve_combine_relation_request(
    state: &mut State,
    req: DissolveCombineRelationRequest,
) -> Result<DissolveCombineRelationResponse, String> {
    if let Some(scene) = state.scene.as_mut() {
        if scene
            .detach_combine_relation(req.EntityA, req.EntityB)
            .is_some()
        {
            scene.signal_fsm_dissolve_combine(req.EntityA);
        } else if scene
            .detach_combine_relation(req.EntityB, req.EntityA)
            .is_some()
        {
            scene.signal_fsm_dissolve_combine(req.EntityB);
        }
    }
    Ok(DissolveCombineRelationResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_attach_entity_request(
    state: &mut State,
    req: AttachEntityRequest,
) -> Result<AttachEntityResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("attach_ent:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    if let Some(pos) = &req.RelatePos
        && finite_vec3([pos.X, pos.Y, pos.Z])
    {
        let rot = req.RelateRot.clone().unwrap_or_default();
        if finite_vec3([rot.Roll, rot.Pitch, rot.Yaw]) {
            let _ = persist_world_entity_transform(state, req.EntityId, [pos.X, pos.Y, pos.Z], Some(rot))?;
        }
    }
    flush_quest_condition_changed(state);
    Ok(AttachEntityResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_hold_hand_request(
    state: &mut State,
    req: HoldHandRequest,
) -> Result<HoldHandResponse, String> {
    let hold_val = i64::from(req.IsHold);
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("hold_hand:{}", req.EntityId), hold_val);
    if let Some(scene) = state.scene.as_mut() {
        let role_id = scene.current_role_entity_id();
        if role_id > 0 {
            state
                .player_components
                .progress
                .info
                .set_key_int(&format!("hold_hand:{role_id}"), hold_val);
            if let Some(&ridx) = scene.net_id_map.get(&role_id) {
                scene.entities[ridx].world_item.hold_hand_target_id =
                    req.IsHold.then_some(req.TargetEntityId);
                scene.entities[ridx].world_item.hold_hand_type = req.HandType;
            }
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    if !req.IsHold {
        let _ = state.conn.push_msg(
            StopHoldHandNotify::MSG_ID,
            &StopHoldHandNotify {
                EntityId: req.EntityId,
                ActionType: req.ActionType,
                TargetEntityId: req.TargetEntityId,
            },
        );
    }
    flush_quest_condition_changed(state);
    Ok(HoldHandResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_sit_chair_request(
    state: &mut State,
    req: SitChairRequest,
) -> Result<SitChairResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("sit_chair:{}", req.EntityId),
        i64::from(req.IsSitDown),
    )
    .map_err(|e| e.to_string())?;
    let chair_pos = if req.IsSitDown
        && let Some(scene) = state.scene.as_ref()
        && let Some(&idx) = scene.net_id_map.get(&req.EntityId)
    {
        scene.entities[idx].position
    } else {
        None
    };
    if let Some(pos) = chair_pos {
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            let _ = quest_actions::update_player_scene_state(&mut qctx, pos.location, Some(pos.rotation));
        }
        drain_event_queue_best_effort(&mut events, state);
    }
    flush_quest_condition_changed(state);
    Ok(SitChairResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_sneak_request(state: &mut State, req: SneakRequest) -> Result<SneakResponse, String> {
    if let Some(scene) = state.scene.as_mut() {
        scene.active_sneak_id = if req.IsStart { req.NodeId.max(1) } else { 0 };
        if !req.IsStart && req.TreeOwnerId > 0 {
            scene.fail_timers.cancel(req.TreeOwnerId, req.NodeId);
        }
    }
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("sneak:node:{}", req.NodeId),
        i64::from(req.IsStart),
    )
    .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(SneakResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_sneak_finish_request(
    state: &mut State,
    req: SneakFinishRequest,
) -> Result<SneakFinishResponse, String> {
    if let Some(scene) = state.scene.as_mut() {
        scene.active_sneak_id = 0;
    }
    let ft = req.FinishType.unwrap_or(SneakFinishType::SneakSuccess);
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("sneak:finish:{}", req.SneakId),
        i64::from(ft.to_i32()),
    )
    .map_err(|e| e.to_string())?;
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        if ft == SneakFinishType::SneakFail {
            let _ = quest_engine::on_sneak_failed(&mut qctx, 0, 0);
        } else {
            let _ = quest_engine::on_condition_state_changed(&mut qctx);
        }
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(SneakFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_sneak_time_request(
    state: &mut State,
    req: SneakTimeRequest,
) -> Result<SneakTimeResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("sneak:time:{}", req.TreeIncId),
    )
    .map_err(|e| e.to_string())?;
    Ok(SneakTimeResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_timeline_trace_start_request(
    state: &mut State,
    req: TimelineTraceStartRequest,
) -> Result<TimelineTraceStartResponse, String> {
    let mut entity_ids = Vec::new();
    let mut cp_count = 0usize;
    let map_id = state.scene.as_ref().map(|s| s.instance_id).unwrap_or(8);
    let cfg_id = state
        .scene
        .as_ref()
        .and_then(|s| s.find_config_id_by_net_id(req.EntityId).map(i64::from))
        .unwrap_or(req.EntityId);
    if let Some(resolved) = level_play::resolve_entity_config(&state.assets, map_id, cfg_id) {
        for entry in &resolved.components.extra {
            if let ExtraEntry::TimelineTrackControlComponent(ttc) = entry {
                let idx = req.Index.max(0) as usize;
                if let Some(group) = ttc.ControlGroups.get(idx) {
                    for cfg in &group.ControlConfigs {
                        cp_count = cp_count.max(cfg.ControlPoints.len());
                        if let Some(target_cfg_id) = cfg.EntityId
                            && target_cfg_id > 0
                        {
                            let target_net_id = state
                                .scene
                                .as_ref()
                                .and_then(|s| {
                                    i32::try_from(target_cfg_id)
                                        .ok()
                                        .and_then(|cid32| s.find_entity_net_id_by_config_id(cid32))
                                })
                                .unwrap_or(target_cfg_id);
                            entity_ids.push(target_net_id);
                        }
                    }
                }
                break;
            }
        }
    }
    if entity_ids.is_empty() && req.EntityId > 0 {
        entity_ids.push(req.EntityId);
    }
    if cp_count == 0 {
        cp_count = 2;
    }
    let point_datas = (0..cp_count)
        .map(|i| ControlPointData {
            ControlPointIndex: i as i32,
            LeftEnable: i > 0,
            RightEnable: i + 1 < cp_count,
        })
        .collect();
    let info = &mut state.player_components.progress.info;
    let max_cp = (cp_count - 1) as i64;
    let cp_key = format!("timeline_cp:{}", req.EntityId);
    info.set_key_int(&format!("timeline_cp_max:{}", req.EntityId), max_cp);
    let cur_cp = info.get_key_int(&cp_key).clamp(0, max_cp);
    info.set_key_int(&cp_key, cur_cp);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(TimelineTraceStartResponse {
        Index: req.Index,
        ControlPoint: cur_cp as i32,
        EntityIds: entity_ids,
        PointDatas: point_datas,
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_timeline_trace_control_request(
    state: &mut State,
    req: TimelineTraceControlRequest,
) -> Result<TimelineTraceControlResponse, String> {
    let key = format!("timeline_cp:{}", req.EntityId);
    let max_key = format!("timeline_cp_max:{}", req.EntityId);
    let saved_max = state.player_components.progress.info.get_key_int(&max_key);
    let max_cp = if saved_max > 0 { saved_max } else { 5 };
    let cur = state
        .player_components
        .progress
        .info
        .get_key_int(&key)
        .clamp(0, max_cp);
    if (req.Forward && cur >= max_cp) || (!req.Forward && cur <= 0) {
        return Ok(TimelineTraceControlResponse {
            ControlPoint: cur as i32,
            Code: Some(ErrorCode::ErrTimelineMove),
        });
    }
    let next_cp = if req.Forward { cur + 1 } else { cur - 1 };
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &key,
        next_cp,
    )
    .map_err(|e| e.to_string())?;
    Ok(TimelineTraceControlResponse {
        ControlPoint: next_cp as i32,
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_timeline_trace_exit_request(
    state: &mut State,
    req: TimelineTraceExitRequest,
) -> Result<TimelineTraceExitResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("timeline_exit:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(TimelineTraceExitResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_remote_batch_collect_request(
    state: &mut State,
    req: RemoteBatchCollectRequest,
) -> Result<RemoteBatchCollectResponse, String> {
    let mut collected = Vec::new();
    let mut remove_infos = Vec::new();
    let mut dirty = false;
    for eid in req.EntityIds {
        let key = format!("remote_collect:{eid}");
        if !state.player_components.progress.info.has_claim(&key) {
            let _ =
                resolve_and_grant_entity_collect_or_capture(state, false, eid, &mut remove_infos)?;
            state.player_components.progress.info.mark_claim(&key);
            dirty = true;
        }
        collected.push(eid);
    }
    if dirty {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    if !remove_infos.is_empty() {
        let _ = state.conn.push_msg(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                IsRemove: true,
                RemoveInfos: remove_infos,
            },
        );
    }
    Ok(RemoteBatchCollectResponse {
        EntityIds: collected,
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_capture_entity_request(
    state: &mut State,
    req: CaptureEntityRequest,
) -> Result<CaptureEntityResponse, String> {
    let inst_id = state.scene.as_ref().map(|s| s.instance_id).unwrap_or(8);
    let key = format!("capture:{inst_id}:{}", req.Id);
    let still_present = state
        .scene
        .as_ref()
        .map(|s| s.net_id_map.contains_key(&req.Id))
        .unwrap_or(false);
    let mut remove_infos = Vec::new();
    if still_present && !state.player_components.progress.info.has_claim(&key) {
        let _ = resolve_and_grant_entity_collect_or_capture(
            state,
            true,
            req.Id,
            &mut remove_infos,
        )?;
        state.player_components.progress.info.mark_claim(&key);
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    if !remove_infos.is_empty() {
        let _ = state.conn.push_msg(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                IsRemove: true,
                RemoveInfos: remove_infos,
            },
        );
    }
    Ok(CaptureEntityResponse {
        ErrCode: Some(ErrorCode::Success),
        Id: req.Id,
    })
}

pub fn handle_capture_batch_entity_request(
    state: &mut State,
    req: CaptureBatchEntityRequest,
) -> Result<CaptureBatchEntityResponse, String> {
    let inst_id = state.scene.as_ref().map(|s| s.instance_id).unwrap_or(8);
    let mut ids = Vec::new();
    let mut remove_infos = Vec::new();
    let mut dirty = false;
    for eid in req.Ids {
        let still_present = state
            .scene
            .as_ref()
            .map(|s| s.net_id_map.contains_key(&eid))
            .unwrap_or(false);
        let key = format!("capture:{inst_id}:{eid}");
        if still_present && !state.player_components.progress.info.has_claim(&key) {
            let _ =
                resolve_and_grant_entity_collect_or_capture(state, true, eid, &mut remove_infos)?;
            state.player_components.progress.info.mark_claim(&key);
            dirty = true;
        }
        ids.push(eid);
    }
    if dirty {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    if !remove_infos.is_empty() {
        let _ = state.conn.push_msg(
            EntityRemoveNotify::MSG_ID,
            &EntityRemoveNotify {
                IsRemove: true,
                RemoveInfos: remove_infos,
            },
        );
    }
    Ok(CaptureBatchEntityResponse {
        ErrCode: Some(ErrorCode::Success),
        Ids: ids,
    })
}

pub fn handle_pick_up_drop_item_request(
    state: &mut State,
    req: PickUpDropItemRequest,
) -> Result<PickUpDropItemResponse, String> {
    let key = format!("drop_pickup:{}", req.DropEntityId);
    if !state.player_components.progress.info.has_claim(&key) {
        let mut granted_any = false;
        if let Some(flow) = &req.ItemGetFlow
            && flow.ItemID > 0
            && flow.ItemCount > 0
        {
            let mut rctx = state.reward_context();
            let res = reward::grant_items(
                &mut rctx,
                0,
                true,
                &[RewardItem {
                    item_id: flow.ItemID,
                    count: flow.ItemCount,
                }],
                1,
            )
            .map_err(|e| e.to_string())?;
            granted_any = res.granted_count > 0;
        }
        if !granted_any
            && let Some(scene) = state.scene.as_ref()
        {
            let cfg_id = scene
                .find_config_id_by_net_id(req.DropEntityId)
                .map(i64::from)
                .unwrap_or(req.DropEntityId);
            if let Some(resolved) =
                level_play::resolve_entity_config(&state.assets, scene.instance_id, cfg_id)
            {
                let reward_id = resolved
                    .components
                    .RewardComponent
                    .as_ref()
                    .and_then(|rc| rc.RewardId)
                    .unwrap_or(0);
                let tb_type = resolved
                    .components
                    .TreasureBoxComponent
                    .as_ref()
                    .and_then(|tb| tb.TypeId)
                    .unwrap_or(0);
                let mut rctx = state.reward_context();
                if tb_type > 0 {
                    let res = reward::grant_treasure_box_reward(&mut rctx, tb_type, reward_id, 7000)
                        .map_err(|e| e.to_string())?;
                    granted_any = res.granted_count > 0;
                } else if reward_id > 0 {
                    let res = reward::grant_drop_package(&mut rctx, reward_id, 1)
                        .map_err(|e| e.to_string())?;
                    granted_any = res.granted_count > 0;
                }
            }
        }
        if !granted_any {
            let mut rctx = state.reward_context();
            reward::grant_items(
                &mut rctx,
                0,
                true,
                &[RewardItem {
                    item_id: 2,
                    count: 150,
                }],
                1,
            )
            .map_err(|e| e.to_string())?;
        }
        if let Some(scene) = state.scene.as_mut()
            && scene.net_id_map.contains_key(&req.DropEntityId)
        {
            scene
                .remove(&state.fs, req.DropEntityId)
                .map_err(|e| e.to_string())?;
            let _ = state.conn.push_msg(
                EntityRemoveNotify::MSG_ID,
                &EntityRemoveNotify {
                    IsRemove: true,
                    RemoveInfos: vec![EntityRemoveInfo {
                        EntityId: req.DropEntityId,
                        ..Default::default()
                    }],
                },
            );
        }
        pb_kv::persist_claim(&state.fs, &mut state.player_components.progress, &key)
            .map_err(|e| e.to_string())?;
    }
    Ok(PickUpDropItemResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_pilot_throw_operation_request(
    state: &mut State,
    req: PilotThrowOperationRequest,
) -> Result<PilotThrowOperationResponse, String> {
    let op_val = req.OpType.map(|t| i64::from(t.to_i32())).unwrap_or(0);
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "pilot_throw:op:1",
        op_val,
    )
    .map_err(|e| e.to_string())?;
    Ok(PilotThrowOperationResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_common_organ_reset_time_request(
    state: &mut State,
    req: CommonOrganResetTimeRequest,
) -> Result<CommonOrganResetTimeResponse, String> {
    let reset_time = state
        .player_components
        .progress
        .info
        .get_key_int(&format!("organ_reset:{}", req.EntityId))
        .max(0) as i32;
    Ok(CommonOrganResetTimeResponse {
        EntityId: req.EntityId,
        ErrorCode: Some(ErrorCode::Success),
        ResetTime: reset_time,
    })
}

pub fn handle_summon2_request(
    state: &mut State,
    req: Summon2Request,
) -> Result<Summon2Response, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("wp:summon2:{}", req.SummonerEntityId),
        i64::from(req.Version),
    )
    .map_err(|e| e.to_string())?;
    Ok(Summon2Response {
        Version: req.Version,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_summon_random_request(
    state: &mut State,
    req: SummonRandomRequest,
) -> Result<SummonRandomResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("summon_rand:{}", req.SummonerId),
    )
    .map_err(|e| e.to_string())?;
    Ok(SummonRandomResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_player_enter_pollution_request(
    state: &mut State,
    req: PlayerEnterPollutionRequest,
) -> Result<PlayerEnterPollutionResponse, String> {
    let rty = req.RangeType.map(|t| i64::from(t.to_i32())).unwrap_or(0);
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("pollution:mgr:{}", req.ManagerEntityId),
        rty,
    )
    .map_err(|e| e.to_string())?;
    Ok(PlayerEnterPollutionResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_enrichment_area_request(
    state: &mut State,
    req: EnrichmentAreaRequest,
) -> Result<EnrichmentAreaResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("enrich:{}", req.ItemId),
    )
    .map_err(|e| e.to_string())?;
    Ok(EnrichmentAreaResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_add_decal_request(
    state: &mut State,
    req: AddDecalRequest,
) -> Result<AddDecalResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("decal:scene:{}", req.SceneId),
        i64::from(req.EffectId),
    )
    .map_err(|e| e.to_string())?;
    Ok(AddDecalResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

fn material_replace_exchange_ratio(assets: &sonoro_gamesv_data::Assets, item_id: i32) -> i32 {
    static RATIOS: std::sync::OnceLock<std::collections::HashMap<i32, i32>> =
        std::sync::OnceLock::new();
    let map = RATIOS.get_or_init(|| {
        let mut out = std::collections::HashMap::new();
        for entry in assets.read_raw_json_array("MaterialReplace") {
            let Some(obj) = entry.as_object() else {
                continue;
            };
            let Some(id) = obj
                .get("ItemId")
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
            else {
                continue;
            };
            let param = obj
                .get("ExchangeParam")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, i64::from(i32::MAX)) as i32;
            out.insert(id, param);
        }
        out
    });
    map.get(&item_id).copied().unwrap_or(1)
}

pub fn handle_batch_material_replace_request(
    state: &mut State,
    req: BatchMaterialReplaceRequest,
) -> Result<BatchMaterialReplaceResponse, String> {
    for ri in &req.ReplaceItems {
        if ri.TargetReplaceItemId <= 0 || ri.TargetNum <= 0 {
            return Ok(BatchMaterialReplaceResponse {
                Code: Some(ErrorCode::RequestParamError),
            });
        }
        let mut costs = Vec::new();
        let mut total_consume = 0i64;
        for (&id, &count) in &ri.ConsumeItems {
            if id > 0 && count > 0 {
                costs.push(CostItem { id, count });
                total_consume += i64::from(count);
            }
        }
        if costs.is_empty() || total_consume <= 0 {
            return Ok(BatchMaterialReplaceResponse {
                Code: Some(ErrorCode::RequestParamError),
            });
        }
        let ratio = i64::from(material_replace_exchange_ratio(
            &state.assets,
            ri.TargetReplaceItemId,
        ));
        let max_grant = (total_consume / ratio).clamp(0, i64::from(i32::MAX)) as i32;
        let grant_num = ri.TargetNum.min(max_grant);
        if grant_num <= 0 {
            return Ok(BatchMaterialReplaceResponse {
                Code: Some(ErrorCode::RequestParamError),
            });
        }
        let ok = consume::consume_costs(
            &mut state.conn,
            &state.fs,
            &mut state.player_components.basic,
            &mut state.player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?;
        if !ok {
            return Ok(BatchMaterialReplaceResponse {
                Code: Some(ErrorCode::ErrItemNotEnough),
            });
        }
        let mut rctx = state.reward_context();
        reward::grant_items(
            &mut rctx,
            0,
            true,
            &[RewardItem {
                item_id: ri.TargetReplaceItemId,
                count: grant_num,
            }],
            1,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(BatchMaterialReplaceResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_material_replace_request(
    state: &mut State,
    req: MaterialReplaceRequest,
) -> Result<MaterialReplaceResponse, String> {
    if req.ConsumeItemId > 0 && req.ConsumeItemNum > 0 && req.TargetReplaceItemId > 0 {
        let costs = [CostItem {
            id: req.ConsumeItemId,
            count: req.ConsumeItemNum,
        }];
        let ok = consume::consume_costs(
            &mut state.conn,
            &state.fs,
            &mut state.player_components.basic,
            &mut state.player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?;
        if !ok {
            return Ok(MaterialReplaceResponse {
                Code: Some(ErrorCode::ErrItemNotEnough),
            });
        }
        let mut rctx = state.reward_context();
        reward::grant_items(
            &mut rctx,
            0,
            true,
            &[RewardItem {
                item_id: req.TargetReplaceItemId,
                count: req.ConsumeItemNum,
            }],
            1,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(MaterialReplaceResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_furniture_diy_save_request(
    state: &mut State,
    req: FurnitureDiySaveRequest,
) -> Result<FurnitureDiySaveResponse, String> {
    let score = (req.FurnitureDiySlotInfos.len() * 100) as i32;
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("diy_atm:{}:{}", req.ActivityId, req.AreaId),
        i64::from(score),
    )
    .map_err(|e| e.to_string())?;
    Ok(FurnitureDiySaveResponse {
        Atmosphere: score,
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_accept_entrust_request(
    state: &mut State,
    req: AcceptEntrustRequest,
) -> Result<AcceptEntrustResponse, String> {
    let star = state
        .assets
        .tables
        .track_moon_entrust
        .get_data_by_id(&req.EntrustId)
        .map(|c| c.Star.max(1))
        .unwrap_or(1);
    let gold = star * 1000;
    let wish = star * 100;
    let heat = star * 10;
    let bonus = star * 20;

    let info = &mut state.player_components.progress.info;
    info.mark_claim(&format!("entrust:{}", req.EntrustId));
    info.set_key_int(&format!("trackmoon:entrust:{}", req.EntrustId), 1);
    info.add_key_int("trackmoon:entrust_done", 1);
    info.add_key_int("trackmoon:gold", i64::from(gold));
    info.add_key_int("trackmoon:wish", i64::from(wish));
    info.add_key_int("trackmoon:popular", i64::from(heat));

    let mut role_settle = Vec::new();
    let mut role_goods = Vec::new();
    let mut first_role = 0i32;
    for rid in req.RoleIds {
        if first_role == 0 {
            first_role = rid;
        }
        let lvl = info.get_key_int(&format!("trackmoon:role_lvl:{rid}")).max(1) as i32;
        let get_prop = |pfx: &str| {
            let base = info
                .get_key_int(&format!("{pfx}:{rid}"))
                .max(100)
                .min(i64::from(i32::MAX - bonus)) as i32;
            base + bonus
        };
        role_settle.push(RoleProperSettle {
            RoleId: rid,
            PropertyA: get_prop("trackmoon:role_a"),
            PropertyB: get_prop("trackmoon:role_b"),
            PropertyC: get_prop("trackmoon:role_c"),
            Level: lvl,
        });
        role_goods.push(RoleGood {
            RoleId: rid,
            Result: Some(sonoro_proto::enums::RoleSettleResult::Good),
        });
    }
    if first_role > 0 {
        info.set_key_int("trackmoon:fav_role", i64::from(first_role));
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(AcceptEntrustResponse {
        AddEntrust: Some(EntrustInfo {
            EntrustId: req.EntrustId,
            IsVisiable: true,
            HistoryLevel: 1,
        }),
        ErrorCode: Some(ErrorCode::Success),
        RoleProperSettle: role_settle,
        TriggerIdeaRole: first_role,
        Wish: wish,
        Gold: gold,
        Heat: heat,
        TriggerIdea: first_role > 0,
        RoleGoods: role_goods,
        EvaluationLevel: 3,
    })
}

pub fn handle_ui_game_play_request(
    state: &mut State,
    req: UiGamePlayRequest,
) -> Result<UiGamePlayResponse, String> {
    let gp_key = String::from_utf8_lossy(&req.GamePlayKey).into_owned();
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("ui_gp:{gp_key}"),
    )
    .map_err(|e| e.to_string())?;
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_ui_gameplay_completed(&mut qctx, &gp_key);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(UiGamePlayResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_well_streaming_finis_request(
    state: &mut State,
    req: WellStreamingFinisRequest,
) -> Result<WellStreamingFinisResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("well_stream:fin:{}", req.EntityIncId),
    )
    .map_err(|e| e.to_string())?;
    Ok(WellStreamingFinisResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_read_display_info_request(
    state: &mut State,
    events: &mut EventQueue,
    req: ReadDisplayInfoRequest,
) -> Result<ReadDisplayInfoResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("display_read:{}", req.DisplayId),
    )
    .map_err(|e| e.to_string())?;
    let mut qctx = state.quest_context(events);
    let _ = quest_engine::events::on_information_view_read(&mut qctx, req.DisplayId);
    Ok(ReadDisplayInfoResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_loop_teleport_volume_action_request(
    state: &mut State,
    req: LoopTeleportVolumeActionRequest,
) -> Result<LoopTeleportVolumeActionResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("loop_tp:vol:{}", req.VolumeId),
    )
    .map_err(|e| e.to_string())?;
    let mut met_ids = Vec::new();
    let rows: Vec<_> = state
        .assets
        .tables
        .loop_teleport_volume_function
        .items
        .iter()
        .filter(|r| r.VolumeId == req.VolumeId)
        .cloned()
        .collect();
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let mut callbacks = quest_engine::action_callbacks();
        for row in rows {
            let ok = if let Some(cond) = &row.Conditions {
                let eval_ctx = qctx.make_eval_context();
                condition::eval_inline_condition_block(&eval_ctx, cond)
            } else {
                true
            };
            if !ok {
                continue;
            }
            met_ids.push(row.Id);
            if let Some(Value::Array(acts)) = &row.Actions {
                let _ = quest_actions::execute_action_list(&mut qctx, &mut callbacks, 0, 0, acts);
            }
        }
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(LoopTeleportVolumeActionResponse {
        ErrorCode: Some(ErrorCode::Success),
        ConditionMetIds: met_ids,
    })
}

pub fn handle_bt_forced_occupation_request(
    state: &mut State,
    req: BtForcedOccupationRequest,
) -> Result<BtForcedOccupationResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("bt:forced_occ:{}", req.TreeIncId),
    )
    .map_err(|e| e.to_string())?;
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_forced_occupation(&mut qctx, req.TreeIncId);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(BtForcedOccupationResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_action_operation_screen_success_request(
    state: &mut State,
    req: ActionOperationScreenSuccessRequest,
) -> Result<ActionOperationScreenSuccessResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("act:op_screen:{}", req.IncId),
    )
    .map_err(|e| e.to_string())?;
    Ok(ActionOperationScreenSuccessResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_action_reset_pos_pre_performance_end_request(
    state: &mut State,
    req: ActionResetPosPrePerformanceEndRequest,
) -> Result<ActionResetPosPrePerformanceEndResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("act:reset_pos_pre:{}", req.IncId),
        i64::from(req.Index),
    )
    .map_err(|e| e.to_string())?;
    Ok(ActionResetPosPrePerformanceEndResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_access_passerby_npc_spawner_request(
    state: &mut State,
    req: AccessPasserbyNpcSpawnerRequest,
) -> Result<AccessPasserbyNpcSpawnerResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("passerby_spawner:{}", req.EntityId),
        i64::from(req.SplineConfigId),
    )
    .map_err(|e| e.to_string())?;
    Ok(AccessPasserbyNpcSpawnerResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

// =========================================================================
// Vehicles, Weather, Teleport, Dungeon, LevelPlay & Quest
// =========================================================================

pub fn handle_vehicle_platform_get_on_request(
    state: &mut State,
    req: VehiclePlatformGetOnRequest,
) -> Result<VehiclePlatformGetOnResponse, String> {
    let on_val = i64::from(req.IsGetOn);
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("vehicle:platform:{}", req.EntityId), on_val);
    if let Some(scene) = state.scene.as_ref() {
        let cfg_id = i64::from(scene.find_config_id_by_net_id(req.EntityId).unwrap_or(0));
        if req.IsGetOn && cfg_id > 0 {
            state
                .player_components
                .progress
                .info
                .set_key_int("vehicle:cur", cfg_id);
        } else if !req.IsGetOn
            && cfg_id > 0
            && state
                .player_components
                .progress
                .info
                .get_key_int("vehicle:cur")
                == cfg_id
        {
            state
                .player_components
                .progress
                .info
                .set_key_int("vehicle:cur", 0);
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(VehiclePlatformGetOnResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn weather_switch_to_weather_id(switch_id: i32) -> i32 {
    match switch_id {
        1 => 1,
        2 => 30002,
        3 => 30003,
        4 => 30005,
        5 => 30007,
        6 => 30008,
        7 => 30009,
        _ => {
            if switch_id > 0 {
                switch_id
            } else {
                1
            }
        }
    }
}

pub fn handle_weather_control_info_request(
    state: &mut State,
    _req: WeatherControlInfoRequest,
) -> Result<WeatherControlInfoResponse, String> {
    let mut ids: Vec<i32> = (1..8).collect();
    for &wid in &state
        .player_components
        .quest
        .info
        .env
        .unlocked_weather_switches
    {
        if !ids.contains(&wid) {
            ids.push(wid);
        }
    }
    let cur = state
        .player_components
        .progress
        .info
        .get_key_int("weather:switch_id");
    if let Ok(cid) = i32::try_from(cur)
        && cid > 0
        && !ids.contains(&cid)
    {
        ids.push(cid);
    }
    Ok(WeatherControlInfoResponse {
        UnlockedSwitchIds: ids,
        IncId: 0,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_weather_control_request(
    state: &mut State,
    req: WeatherControlRequest,
) -> Result<WeatherControlResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "weather:switch_id",
        i64::from(req.WeatherSwitchId),
    )
    .map_err(|e| e.to_string())?;
    let weather_id = weather_switch_to_weather_id(req.WeatherSwitchId);
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        quest_engine::change_weather(&mut qctx, weather_id).map_err(|e| e.to_string())?;
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(WeatherControlResponse {
        ErrorCode: Some(ErrorCode::Success),
        WeatherId: weather_id,
    })
}

pub fn handle_weather_switch_info_request(
    state: &mut State,
    _req: WeatherSwitchInfoRequest,
) -> Result<WeatherSwitchInfoResponse, String> {
    let mut ids = state
        .player_components
        .quest
        .info
        .env
        .unlocked_weather_switches
        .clone();
    let cur = state
        .player_components
        .progress
        .info
        .get_key_int("weather:switch_id");
    if let Ok(cid) = i32::try_from(cur)
        && cid > 0
        && !ids.contains(&cid)
    {
        ids.push(cid);
    }
    Ok(WeatherSwitchInfoResponse {
        UnlockedSwitchIds: ids,
    })
}

pub fn handle_seamless_teleport_request(
    state: &mut State,
    req: SeamlessTeleportRequest,
) -> Result<SeamlessTeleportResponse, String> {
    if let Some(loc) = &req.Location {
        if !finite_vec3([loc.X, loc.Y, loc.Z]) {
            return Ok(SeamlessTeleportResponse {
                ErrorCode: Some(ErrorCode::ErrTeleportPositionIllegal),
            });
        }
        let rotation = req.Rotation.as_ref().map(|r| [r.Roll, r.Pitch, r.Yaw]);
        if let Some(v) = rotation
            && !finite_vec3(v)
        {
            return Ok(SeamlessTeleportResponse {
                ErrorCode: Some(ErrorCode::ErrTeleportPositionIllegal),
            });
        }
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            quest_actions::update_player_scene_state(&mut qctx, [loc.X, loc.Y, loc.Z], rotation)
                .map_err(|e| e.to_string())?;
        }
        drain_event_queue_best_effort(&mut events, state);
    }
    Ok(SeamlessTeleportResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_seamless_teleport_vehicle_request(
    state: &mut State,
    req: SeamlessTeleportVehicleRequest,
) -> Result<SeamlessTeleportVehicleResponse, String> {
    if let Some(loc) = &req.Location {
        let location = [loc.X, loc.Y, loc.Z];
        let player_rotation = req.Rotation.as_ref().map(|r| [r.Roll, r.Pitch, r.Yaw]);
        if !finite_vec3(location) || player_rotation.is_some_and(|v| !finite_vec3(v)) {
            return Ok(SeamlessTeleportVehicleResponse {
                ErrorCode: Some(ErrorCode::ErrTeleportPositionIllegal),
            });
        }
        let vehicle_net_id = {
            let Some(scene) = state.scene.as_ref() else {
                return Ok(SeamlessTeleportVehicleResponse {
                    ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
                });
            };
            let vnid = if scene.net_id_map.contains_key(&req.VehicleEntityId) {
                req.VehicleEntityId
            } else {
                i32::try_from(req.VehicleEntityId)
                    .ok()
                    .and_then(|cid| scene.find_entity_net_id_by_config_id(cid))
                    .unwrap_or(0)
            };
            let Some(&vidx) = scene.net_id_map.get(&vnid) else {
                return Ok(SeamlessTeleportVehicleResponse {
                    ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
                });
            };
            if scene.entities[vidx].vehicle.is_none() {
                return Ok(SeamlessTeleportVehicleResponse {
                    ErrorCode: Some(ErrorCode::ErrVehicleEntityTypeError),
                });
            }
            vnid
        };
        if !persist_world_entity_transform(
            state,
            vehicle_net_id,
            location,
            req.Rotation.clone(),
        )? {
            return Ok(SeamlessTeleportVehicleResponse {
                ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            });
        }
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
        {
            let mut events = EventQueue::new();
            {
                let mut qctx = state.quest_context(&mut events);
                quest_actions::update_player_scene_state(&mut qctx, location, player_rotation)
                    .map_err(|e| e.to_string())?;
            }
            drain_event_queue_best_effort(&mut events, state);
        }
        let _ = state.conn.push_msg(
            MovePackageNotify::MSG_ID,
            &MovePackageNotify {
                MovingEntities: vec![MovingEntityData {
                    EntityId: vehicle_net_id,
                    Originator: vehicle_net_id,
                    MoveInfos: vec![MoveReplaySample {
                        Location: Some(Vector {
                            X: location[0],
                            Y: location[1],
                            Z: location[2],
                        }),
                        Rotation: req.Rotation,
                        ..Default::default()
                    }],
                    ForcePush: true,
                }],
            },
        );
    }
    Ok(SeamlessTeleportVehicleResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_pass_portal_request(
    state: &mut State,
    req: PassPortalRequest,
) -> Result<PassPortalResponse, String> {
    if let Some(loc) = &req.Location {
        if !finite_vec3([loc.X, loc.Y, loc.Z]) {
            return Ok(PassPortalResponse {
                ErrorCode: Some(ErrorCode::ErrTeleportPositionIllegal),
            });
        }
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            quest_actions::update_player_scene_state(&mut qctx, [loc.X, loc.Y, loc.Z], None)
                .map_err(|e| e.to_string())?;
        }
        drain_event_queue_best_effort(&mut events, state);
    }
    let config_id = state
        .scene
        .as_ref()
        .and_then(|s| s.find_config_id_by_net_id(req.EntityId));
    if let Some(cid) = config_id {
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            quest_engine::on_portal_passed(&mut qctx, cid).map_err(|e| e.to_string())?;
        }
        drain_event_queue_best_effort(&mut events, state);
    }
    Ok(PassPortalResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_awake_dynamic_portal_request(
    state: &mut State,
    req: AwakeDynamicPortalRequest,
) -> Result<AwakeDynamicPortalResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("dyn_portal:awake:{}", req.EntityId),
        i64::from(req.IsAwake),
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut() {
        let nid_opt = if scene.net_id_map.contains_key(&req.EntityId) {
            Some(req.EntityId)
        } else {
            i32::try_from(req.EntityId)
                .ok()
                .and_then(|cid| scene.find_entity_net_id_by_config_id(cid))
        };
        if let Some(nid) = nid_opt {
            let state_name = if req.IsAwake {
                "关卡.Common.状态.激活"
            } else {
                "关卡.Common.状态.常态"
            };
            if let Some(upd) = scene.update_entity_state_tag(
                &state.assets.tables.gameplay_tag_remap,
                nid,
                state_name,
            ) {
                quest_actions::emit_entity_state_tag_update_checked(
                    scene,
                    Some(&state.assets),
                    &mut state.conn,
                    nid,
                    upd,
                );
            }
        }
    }
    flush_quest_condition_changed(state);
    Ok(AwakeDynamicPortalResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_inst_entity_teleport_request(
    state: &mut State,
    req: InstEntityTeleportRequest,
) -> Result<InstEntityTeleportResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("inst:ent_tp:{}", req.InstId),
        i64::from(req.EntityConfigId),
    )
    .map_err(|e| e.to_string())?;
    let cur_inst = state
        .scene
        .as_ref()
        .map_or(state.player_components.scene.last_scene_info.instance_id, |s| s.instance_id);
    let target_inst = if req.InstId > 0 { req.InstId } else { cur_inst };
    let target_map = state.assets.resolve_map_config_id(target_inst);
    let tp_data = if req.EntityConfigId > 0 {
        state
            .assets
            .find_level_entity(target_map, i64::from(req.EntityConfigId))
            .or_else(|| {
                state
                    .assets
                    .find_level_entity(target_inst, i64::from(req.EntityConfigId))
            })
            .and_then(|cfg| {
                let pos = entity_world_pos(cfg.as_ref())?;
                let rot = entity_world_rot(cfg.as_ref());
                Some((target_inst, target_map, pos, rot))
            })
    } else {
        None
    };
    if let Some((inst_id, map_id, pos, rot)) = tp_data {
        let mut events = EventQueue::new();
        role_helper::dismount_active_vehicle(
            &mut state.conn,
            state.scene.as_mut(),
            &mut state.player_components.progress,
            &state.assets,
            &state.fs,
            state.player_id,
        );
        if inst_id == cur_inst && state.scene.is_some() {
            {
                let mut qctx = state.quest_context(&mut events);
                quest_actions::update_player_scene_state(&mut qctx, pos, Some(rot))
                    .map_err(|e| e.to_string())?;
            }
            drain_event_queue_best_effort(&mut events, state);
            let _ = state.conn.push_msg(
                TeleportNotify::MSG_ID,
                &TeleportNotify {
                    MapId: map_id,
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
        } else {
            super::teleport::enqueue_scene_transfer(state, &mut events, inst_id, pos, rot[2])?;
            drain_event_queue_best_effort(&mut events, state);
        }
    }
    flush_quest_condition_changed(state);
    Ok(InstEntityTeleportResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_flow_teleport_to_plot_pos_request(
    state: &mut State,
    req: FlowTeleportToPlotPosRequest,
) -> Result<FlowTeleportToPlotPosResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("flow:tp_plot:{}", req.FlowIncId),
    )
    .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(FlowTeleportToPlotPosResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_quest_fast_return_pos_request(
    state: &mut State,
    req: QuestFastReturnPosRequest,
) -> Result<QuestFastReturnPosResponse, String> {
    if req.NodeId > 0 {
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            let _ = quest_engine::on_submit_node(
                &mut qctx,
                req.TreeOwnerId.max(0),
                req.TreeIncId,
                req.NodeId,
            );
        }
        drain_event_queue_best_effort(&mut events, state);
    }
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("quest:fast_ret:{}", req.TreeIncId),
        i64::from(req.NodeId),
    )
    .map_err(|e| e.to_string())?;
    Ok(QuestFastReturnPosResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_fast_return_dungeon_request(
    state: &mut State,
    req: FastReturnDungeonRequest,
) -> Result<FastReturnDungeonResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("wp:fast_ret_node:{}", req.TreeIncId),
        i64::from(req.NodeId),
    )
    .map_err(|e| e.to_string())?;
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        if req.NodeId > 0 {
            let _ = quest_engine::on_submit_node(
                &mut qctx,
                req.TreeOwnerId.max(0),
                req.TreeIncId,
                req.NodeId,
            );
        }
        if !req.ContinueInst {
            quest_actions::leave_current_dungeon(&mut qctx, 0).map_err(|e| e.to_string())?;
        }
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(FastReturnDungeonResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_un_opened_area_pullback_request(
    state: &mut State,
    req: UnOpenedAreaPullbackRequest,
) -> Result<UnOpenedAreaPullbackResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("wp:unopen_pullback:{}", req.TransferEffectId),
        1,
    )
    .map_err(|e| e.to_string())?;
    let pullback = state.scene.as_ref().and_then(|scene| {
        let player = scene.instance.players.first()?;
        let safe_loc = scene.last_spatial_check_loc;
        let cur_loc = player.location;
        let dx = cur_loc[0] - safe_loc[0];
        let dy = cur_loc[1] - safe_loc[1];
        let dz = cur_loc[2] - safe_loc[2];
        ((safe_loc[0] != 0.0 || safe_loc[1] != 0.0 || safe_loc[2] != 0.0)
            && (dx * dx + dy * dy + dz * dz > 1.0))
            .then_some((scene.instance_id, safe_loc))
    });
    if let Some((inst_id, safe_loc)) = pullback {
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            quest_actions::update_player_scene_state(&mut qctx, safe_loc, None)
                .map_err(|e| e.to_string())?;
        }
        drain_event_queue_best_effort(&mut events, state);
        let _ = state.conn.push_msg(
            TeleportNotify::MSG_ID,
            &TeleportNotify {
                MapId: inst_id,
                Pos: Some(Vector {
                    X: safe_loc[0],
                    Y: safe_loc[1],
                    Z: safe_loc[2],
                }),
                Reason: Some(TeleportReason::Action),
                ..Default::default()
            },
        );
    }
    flush_quest_condition_changed(state);
    Ok(UnOpenedAreaPullbackResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_revive_request(
    state: &mut State,
    _req: ReviveRequest,
) -> Result<ReviveResponse, String> {
    let mut events = EventQueue::new();
    if let Some(scene) = state.scene.as_mut() {
        role_helper::revive_and_heal_formation(
            &mut state.conn,
            scene,
            &mut state.player_components.role,
            &mut events,
            &state.fs,
        );
        let _ = state.conn.push_msg(
            PlayerReviveNotify::MSG_ID,
            &PlayerReviveNotify {
                PlayerId: scene.player_id,
                ReviveType: 1,
                TeleportId: scene.instance_id,
                ..Default::default()
            },
        );
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(ReviveResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_low_memory_change_scene_finish_request(
    state: &mut State,
    events: &mut EventQueue,
    _req: LowMemoryChangeSceneFinishRequest,
) -> Result<LowMemoryChangeSceneFinishResponse, String> {
    if state.scene.is_some() {
        let mut qctx = state.quest_context(events);
        let _ = quest_engine::events::on_program_special_process_finish(&mut qctx);
        let _ = quest_engine::on_scene_loading_finish(&mut qctx);
    }
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        "scene:low_mem_fin:1",
    )
    .map_err(|e| e.to_string())?;
    Ok(LowMemoryChangeSceneFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_inst_entrance_detail_request(
    state: &mut State,
    req: InstEntranceDetailRequest,
) -> Result<InstEntranceDetailResponse, String> {
    let pc = &state.player_components.progress.info;
    let qc = &state.player_components.quest.info;
    let mut list = Vec::new();
    if let Some(entrance) = state
        .assets
        .tables
        .instance_dungeon_entrance
        .get_data_by_id(&req.EntranceId)
    {
        for &inst_id in &entrance.InstanceDungeonList {
            let done = qc.completed_dungeons.contains(&inst_id);
            let arch = match pc.get_key_int(&format!("inst:archive:{inst_id}")) {
                1 => ArchiveType::NormalArchive,
                2 => ArchiveType::OverdueVersion,
                3 => ArchiveType::OverdueTime,
                4 => ArchiveType::OverdueChangeSex,
                _ => ArchiveType::NoneArchive,
            };
            list.push(InstDetailEntry {
                InstId: inst_id,
                ResetTime: pc.get_key_int(&format!("inst:reset:{inst_id}")).max(0),
                Count: i32::from(done),
                ArchiveType: Some(arch),
            });
            if list.len() >= 32 {
                break;
            }
        }
    } else {
        for inst in &state.assets.tables.instance_dungeon.items {
            let done = qc.completed_dungeons.contains(&inst.Id);
            if done || inst.Id == req.EntranceId {
                list.push(InstDetailEntry {
                    InstId: inst.Id,
                    ResetTime: 0,
                    Count: i32::from(done),
                    ArchiveType: Some(ArchiveType::NoneArchive),
                });
                if list.len() >= 32 {
                    break;
                }
            }
        }
    }
    Ok(InstEntranceDetailResponse {
        ErrorCode: Some(ErrorCode::Success),
        LimitEndTime: pc
            .get_key_int(&format!("inst:limit_end:{}", req.EntranceId))
            .max(0),
        EntranceId: req.EntranceId,
        InstDetailEntryList: list,
    })
}

pub fn handle_invite_rechallenge_request(
    state: &mut State,
    _req: InviteRechallengeRequest,
) -> Result<InviteRechallengeResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "inst:rechallenge_invite:1",
        1,
    )
    .map_err(|e| e.to_string())?;
    Ok(InviteRechallengeResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_receive_rechallenge_request(
    state: &mut State,
    req: ReceiveRechallengeRequest,
) -> Result<ReceiveRechallengeResponse, String> {
    let res_val = req.Result.map(|r| i64::from(r.to_i32())).unwrap_or(0);
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "inst:rechallenge_res:1",
        res_val,
    )
    .map_err(|e| e.to_string())?;
    Ok(ReceiveRechallengeResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_level_sequence_frame_event_action_request(
    state: &mut State,
    req: LevelSequenceFrameEventActionRequest,
) -> Result<LevelSequenceFrameEventActionResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("level_seq:evt:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    let entity_config_id = state
        .scene
        .as_ref()
        .and_then(|s| s.find_config_id_by_net_id(req.EntityId))
        .or_else(|| i32::try_from(req.EntityId).ok())
        .unwrap_or(0);
    let key = String::from_utf8_lossy(&req.Key).into_owned();
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        quest_engine::on_level_sequence_mark_reached(&mut qctx, entity_config_id, &key)
            .map_err(|e| e.to_string())?;
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(LevelSequenceFrameEventActionResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_loading_level_area_request(
    state: &mut State,
    _req: LoadingLevelAreaRequest,
) -> Result<LoadingLevelAreaResponse, String> {
    let configs = state
        .assets
        .tables
        .loading_level_area
        .items
        .iter()
        .filter(|row| row.BeginTime > 0 || row.EndTime > 0 || row.IsLimitShow)
        .map(|row| LoadingAreaConfig {
            Id: row.Id,
            BeginTime: row.BeginTime,
            EndTime: row.EndTime,
        })
        .collect();
    Ok(LoadingLevelAreaResponse {
        LimitConfigs: configs,
    })
}

pub fn handle_lst_battle_record_request(
    state: &mut State,
    _req: LstBattleRecordRequest,
) -> Result<LstBattleRecordResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("battle:record_req", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(LstBattleRecordResponse {
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_quest_ready_for_give_up_request(
    state: &mut State,
    req: QuestReadyForGiveUpRequest,
) -> Result<QuestReadyForGiveUpResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("quest:ready_giveup:{}", req.QuestId),
    )
    .map_err(|e| e.to_string())?;
    Ok(QuestReadyForGiveUpResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

// =========================================================================
// Formation, Roles, Shop, Cooking & Inventory
// =========================================================================

pub fn handle_update_fight_role_request(
    state: &mut State,
    req: UpdateFightRoleRequest,
) -> Result<UpdateFightRoleResponse, String> {
    let role_map = &state.player_components.role.role_map;
    let mut roles: [Option<FormationRole>; 3] = [None, None, None];
    let mut count = 0usize;
    for rid in req.RoleIds {
        if rid <= 0 || !role_map.contains_key(&rid) || count >= 3 {
            continue;
        }
        roles[count] = Some(FormationRole {
            role_id: rid,
            entity_id: 0,
            on_stage_without_control: false,
        });
        count += 1;
    }
    let mut events = EventQueue::new();
    if count > 0
        && let Some(scene) = state.scene.as_mut()
        && let Ok(idx) = usize::try_from(scene.formation_info.cur_formation)
        && let Some(form) = scene.formation_info.formations.get_mut(idx)
    {
        form.roles = roles.clone();
        form.cur_role = if req.CurRoleId != 0 {
            req.CurRoleId
        } else {
            roles[0].as_ref().map(|f| f.role_id).unwrap_or(0)
        };
        scene.save(&state.fs).map_err(|e| e.to_string())?;
        events.enqueue(Event::UpdateFormations {
            player_id: state.player_id,
            formation_id: scene.formation_info.cur_formation + 1,
            role_ids: Vec::new(),
        });
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(UpdateFightRoleResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_reorder_formation_request(
    state: &mut State,
    req: ReorderFormationRequest,
) -> Result<ReorderFormationResponse, String> {
    let mut events = EventQueue::new();
    if let Some(scene) = state.scene.as_mut()
        && !scene.formation_info.formations.is_empty()
    {
        let max_idx = (scene.formation_info.formations.len() - 1) as i32;
        let src_idx = (req.FormationId - 1).clamp(0, max_idx) as usize;
        let dst_idx = (req.TargetId - 1).clamp(0, max_idx) as usize;
        if src_idx != dst_idx {
            scene.formation_info.formations.swap(src_idx, dst_idx);
            scene.save(&state.fs).map_err(|e| e.to_string())?;
            events.enqueue(Event::UpdateFormations {
                player_id: state.player_id,
                formation_id: scene.formation_info.cur_formation + 1,
                role_ids: Vec::new(),
            });
        }
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(ReorderFormationResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_scene_group_data_and_cur_formation_entity_request(
    state: &mut State,
    _req: SceneGroupDataAndCurFormationEntityRequest,
) -> Result<SceneGroupDataAndCurFormationEntityResponse, String> {
    let saved_group = state
        .player_components
        .progress
        .info
        .get_key_int("scene_group:active");
    let cur_group = if saved_group > 0 {
        saved_group as i32
    } else {
        1
    };
    let mut groups = Vec::new();
    let mut cur_role_entities = Vec::new();
    let now_ms = energy::now_seconds() * 1000;
    let player_id = state.player_id;
    let assets = state.assets.clone();
    if let Some(scene) = state.scene.as_mut()
        && let Ok(idx) = usize::try_from(scene.formation_info.cur_formation)
        && let Some(form) = scene.formation_info.formations.get(idx).cloned()
    {
        let mut role_infos = Vec::new();
        for r in form.roles.iter().flatten() {
            role_infos.push(SceneRoleInformation {
                RoleId: r.role_id,
                EntityId: r.entity_id,
                OnStageOutOfControl: r.on_stage_without_control,
            });
            if r.entity_id > 0
                && let Ok(epb) = entity_proto::build_optional(
                    &assets,
                    scene,
                    Some(&mut state.buff_timers),
                    r.entity_id,
                    now_ms,
                )
            {
                cur_role_entities.push(epb);
            }
        }
        let living_status = role_helper::formation_living_status(scene, &form);
        groups.push(PlayerSceneGroupDataList {
            PlayerId: player_id,
            PlayerSceneGroupData: vec![PlayerSceneGroupData {
                Group: cur_group,
                FightRoleInfos: role_infos,
                CurRole: form.cur_role,
                LivingStatus: Some(living_status),
                ..Default::default()
            }],
            CurrentGroup: cur_group,
        });
    }
    Ok(SceneGroupDataAndCurFormationEntityResponse {
        ErrorCode: Some(ErrorCode::Success),
        PlayerSceneGroupDataList: groups,
        CurFormationRoleEntitys: cur_role_entities,
    })
}

pub fn handle_update_scene_group_data_request(
    state: &mut State,
    req: UpdateSceneGroupDataRequest,
) -> Result<UpdateSceneGroupDataResponse, String> {
    let grp = req.TargetGroup.map(|g| i64::from(g.to_i32())).unwrap_or(0);
    let info = &mut state.player_components.progress.info;
    info.set_key_int("scene_group:active", grp);
    for (idx, &eid) in req.EntityIds.iter().enumerate() {
        info.set_key_int(&format!("scene_group:{grp}:{idx}"), eid);
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(UpdateSceneGroupDataResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_switch_battle_mode_request(
    state: &mut State,
    req: SwitchBattleModeRequest,
) -> Result<SwitchBattleModeResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "battle:client_mode",
        i64::from(req.Client),
    )
    .map_err(|e| e.to_string())?;
    Ok(SwitchBattleModeResponse { Client: req.Client })
}

pub fn handle_resonant_chain_optional_limit_request(
    _state: &mut State,
    _req: ResonantChainOptionalLimitRequest,
) -> Result<ResonantChainOptionalLimitResponse, String> {
    Ok(ResonantChainOptionalLimitResponse {
        Limit: 10,
        ..Default::default()
    })
}

pub fn handle_food_process_request(
    state: &mut State,
    req: FoodProcessRequest,
) -> Result<FoodProcessResponse, String> {
    let count = req.CookCount;
    if count <= 0 {
        return Ok(FoodProcessResponse {
            Code: Some(ErrorCode::ErrCookingCount),
            Id: req.Id,
            ..Default::default()
        });
    }
    let Some(cp) = state
        .assets
        .tables
        .cook_processed
        .get_data_by_id(&req.Id)
    else {
        return Ok(FoodProcessResponse {
            Code: Some(ErrorCode::ErrCookingProcessNotFound),
            Id: req.Id,
            ..Default::default()
        });
    };
    let final_item_id = cp.FinalItemId;
    let mut costs = Vec::new();
    if !cp.ConsumeItemsId.is_empty() {
        for entry in &cp.ConsumeItemsId {
            if entry.ItemId <= 0 || entry.Count <= 0 {
                continue;
            }
            let Some(scaled) = entry.Count.checked_mul(count) else {
                return Ok(FoodProcessResponse {
                    Code: Some(ErrorCode::ErrCookingCount),
                    Id: req.Id,
                    ..Default::default()
                });
            };
            costs.push(CostItem {
                id: entry.ItemId,
                count: scaled,
            });
        }
    } else {
        for it in &req.PrimaryFood {
            if it.ItemId <= 0 || it.ItemNum <= 0 {
                continue;
            }
            let Some(scaled) = it.ItemNum.checked_mul(count) else {
                return Ok(FoodProcessResponse {
                    Code: Some(ErrorCode::ErrCookingCount),
                    Id: req.Id,
                    ..Default::default()
                });
            };
            costs.push(CostItem {
                id: it.ItemId,
                count: scaled,
            });
        }
    }
    drop(cp);
    if !costs.is_empty() {
        let ok = consume::consume_costs(
            &mut state.conn,
            &state.fs,
            &mut state.player_components.basic,
            &mut state.player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?;
        if !ok {
            return Ok(FoodProcessResponse {
                Code: Some(ErrorCode::ErrItemNotEnough),
                Id: req.Id,
                ..Default::default()
            });
        }
    }
    let mut final_food = Vec::new();
    if final_item_id > 0 {
        let mut rctx = state.reward_context();
        reward::grant_items(
            &mut rctx,
            req.Id,
            false,
            &[RewardItem {
                item_id: final_item_id,
                count,
            }],
            2,
        )
        .map_err(|e| e.to_string())?;
        final_food.push(SingleItemInfo {
            ItemId: final_item_id,
            ItemNum: count,
        });
    }
    Ok(FoodProcessResponse {
        Code: Some(ErrorCode::Success),
        FinalFood: final_food,
        Id: req.Id,
        ..Default::default()
    })
}

fn hide_discarded_weapon(state: &mut State, entity_id: i64) {
    if let Some(scene) = state.scene.as_mut() {
        let mut cpack = Vec::new();
        if scene.set_entity_visibility(entity_id, false, &mut cpack) && !cpack.is_empty() {
            let _ = state.conn.push_msg(
                sonoro_proto::pb::CombatReceivePackNotify::MSG_ID,
                &sonoro_proto::pb::CombatReceivePackNotify { Data: cpack },
            );
        }
    }
}

pub fn handle_discard_weapon_request(
    state: &mut State,
    req: DiscardWeaponRequest,
) -> Result<DiscardWeaponResponse, String> {
    hide_discarded_weapon(state, req.EntityId);
    Ok(DiscardWeaponResponse { DiscardSucc: true })
}

pub fn handle_discard_weapon_push(
    state: &mut State,
    push: DiscardWeaponPush,
) -> Result<(), String> {
    hide_discarded_weapon(state, push.EntityId);
    Ok(())
}

// =========================================================================
// Map, Explore, Skins, Music, Profile, Settings & Misc Sync
// =========================================================================

pub fn handle_last_big_scene_mini_map_info_request(
    state: &mut State,
    _req: LastBigSceneMiniMapInfoRequest,
) -> Result<LastBigSceneMiniMapInfoResponse, String> {
    let mut px = 0.0f32;
    let mut py = 0.0f32;
    let mut pz = 0.0f32;
    if let Some(scene) = state.scene.as_ref() {
        for player in &scene.instance.players {
            if player.id == state.player_id {
                px = player.location[0];
                py = player.location[1];
                pz = player.location[2];
                break;
            }
        }
    }
    Ok(LastBigSceneMiniMapInfoResponse {
        Code: Some(ErrorCode::Success),
        PosX: px,
        PosY: py,
        PosZ: pz,
        PosA: 0.0,
    })
}

fn dist_sq3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

pub fn handle_navigation_find_path_request(
    state: &mut State,
    req: NavigationFindPathRequest,
) -> Result<NavigationFindPathResponse, String> {
    let mut path = Vec::new();
    if let Some(s) = &req.StartPos {
        path.push(s.clone());
    }
    if let (Some(s), Some(e)) = (&req.StartPos, &req.EndPos) {
        let start3 = [s.X, s.Y, s.Z];
        let end3 = [e.X, e.Y, e.Z];
        let map_id = state
            .assets
            .resolve_map_config_id(if req.MapId > 0 { req.MapId } else { 8 });
        let match_r2 = 1500.0f32 * 1500.0f32;
        for &idx in state.assets.indexes.spawnable_indices_for_map(map_id) {
            let Some(lcfg) = state.assets.get_level_entity_by_idx(idx) else {
                continue;
            };
            let Some(sc) = &lcfg.ComponentsData.SplineComponent else {
                continue;
            };
            let Some(pts) = sc
                .Option
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|o| o.get("Points"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            if pts.len() < 2 {
                continue;
            }
            let base = entity_world_pos(lcfg.as_ref()).unwrap_or([0.0; 3]);
            let rot = entity_world_rot(lcfg.as_ref());
            let (Some(fp), Some(lp)) = (
                pts.first()
                    .and_then(Value::as_object)
                    .and_then(|o| o.get("Position"))
                    .and_then(Value::as_object),
                pts.last()
                    .and_then(Value::as_object)
                    .and_then(|o| o.get("Position"))
                    .and_then(Value::as_object),
            ) else {
                continue;
            };
            let off0 = gravity::rotate_local_offset_ue(read_json_pos3_offsets(fp), rot);
            let off1 = gravity::rotate_local_offset_ue(read_json_pos3_offsets(lp), rot);
            let p0 = [base[0] + off0[0], base[1] + off0[1], base[2] + off0[2]];
            let p1 = [base[0] + off1[0], base[1] + off1[1], base[2] + off1[2]];
            let forward = dist_sq3(start3, p0) <= match_r2 && dist_sq3(end3, p1) <= match_r2;
            let reverse = dist_sq3(start3, p1) <= match_r2 && dist_sq3(end3, p0) <= match_r2;
            if !forward && !reverse {
                continue;
            }
            let iter: Box<dyn Iterator<Item = &Value>> = if forward {
                Box::new(pts.iter())
            } else {
                Box::new(pts.iter().rev())
            };
            for item in iter {
                let Some(pv) = item
                    .as_object()
                    .and_then(|o| o.get("Position"))
                    .and_then(Value::as_object)
                else {
                    continue;
                };
                let off = gravity::rotate_local_offset_ue(read_json_pos3_offsets(pv), rot);
                path.push(Vector {
                    X: base[0] + off[0],
                    Y: base[1] + off[1],
                    Z: base[2] + off[2],
                });
            }
            break;
        }
    }
    if let Some(e) = req.EndPos {
        path.push(e);
    }
    Ok(NavigationFindPathResponse {
        ErrorCode: Some(ErrorCode::Success),
        PosList: path,
    })
}

fn save_and_push_treasure_slots(state: &mut State, slots: &[i64]) -> Result<(), String> {
    let str_buf: String = slots
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let pb_slots: Vec<TreasureBoxSlotInfo> = slots
        .iter()
        .map(|&id| TreasureBoxSlotInfo {
            DetectionSlotId: id,
            MarkId: 0,
        })
        .collect();
    state
        .player_components
        .progress
        .info
        .set_key_str("treasure_slots", &str_buf);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let _ = state.conn.push_msg(
        TreasureBoxSlotAllInfoNotify::MSG_ID,
        &TreasureBoxSlotAllInfoNotify {
            AllTreasureBoxSlotInfo: pb_slots,
        },
    );
    Ok(())
}

fn read_treasure_slots(state: &State, skip_id: Option<i64>) -> Vec<i64> {
    let raw = state
        .player_components
        .progress
        .info
        .get_key_str("treasure_slots");
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .filter_map(|tok| tok.parse::<i64>().ok())
        .filter(|&v| v > 0 && skip_id != Some(v))
        .collect()
}

pub fn handle_add_treasure_box_slot_request(
    state: &mut State,
    _req: AddTreasureBoxSlotRequest,
) -> Result<AddTreasureBoxSlotResponse, String> {
    let mut slots = read_treasure_slots(state, None);
    let slot_id = state
        .player_components
        .progress
        .info
        .add_key_int("treasure_slot:next", 1);
    slots.push(slot_id);
    save_and_push_treasure_slots(state, &slots)?;
    Ok(AddTreasureBoxSlotResponse {
        Code: Some(ErrorCode::Success),
        TreasureBoxSlotInfo: Some(TreasureBoxSlotInfo {
            DetectionSlotId: slot_id,
            MarkId: 0,
        }),
    })
}

pub fn handle_remove_treasure_box_slot_request(
    state: &mut State,
    req: RemoveTreasureBoxSlotRequest,
) -> Result<RemoveTreasureBoxSlotResponse, String> {
    let slots = read_treasure_slots(state, Some(req.DetectionSlotId));
    save_and_push_treasure_slots(state, &slots)?;
    Ok(RemoveTreasureBoxSlotResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_explore_activity_task_reward_request(
    state: &mut State,
    req: ExploreActivityTaskRewardRequest,
) -> Result<ExploreActivityTaskRewardResponse, String> {
    let key = format!("explore_task:{}", req.TaskId);
    if state.player_components.progress.info.mark_claim(&key) {
        let mut rctx = state.reward_context();
        reward::grant_table_reward(
            &mut rctx,
            "ExploreActivityTask",
            i64::from(req.TaskId),
            false,
            true,
            1,
            &[
                RewardItem {
                    item_id: 3,
                    count: 20,
                },
                RewardItem {
                    item_id: 2,
                    count: 10000,
                },
            ],
            None,
        )
        .map_err(|e| e.to_string())?;
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(ExploreActivityTaskRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_mul_fishing_illustrated_reward_request(
    state: &mut State,
    req: MulFishingIllustratedRewardRequest,
) -> Result<MulFishingIllustratedRewardResponse, String> {
    for &rid in &req.RewardId {
        if state
            .player_components
            .progress
            .info
            .mark_claim(&format!("fish_ill:{rid}"))
        {
            let mut rctx = state.reward_context();
            reward::grant_table_reward(
                &mut rctx,
                "FishingHandBook",
                i64::from(rid),
                false,
                true,
                1,
                &[RewardItem {
                    item_id: 3,
                    count: 20,
                }],
                None,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(MulFishingIllustratedRewardResponse {
        RewardId: req.RewardId,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_online_motor_activity_level_request(
    state: &mut State,
    req: OnlineMotorActivityLevelRequest,
) -> Result<OnlineMotorActivityLevelResponse, String> {
    let rtype = req.RewardType.map(|t| t.to_i32()).unwrap_or(0);
    for &rid in &req.RewardIds {
        if state
            .player_components
            .progress
            .info
            .mark_claim(&format!("online_motor:{rtype}:{rid}"))
        {
            let mut rctx = state.reward_context();
            reward::grant_table_reward(
                &mut rctx,
                "OnlineMotorActivity",
                i64::from(rid),
                false,
                true,
                1,
                &[RewardItem {
                    item_id: 3,
                    count: 40,
                }],
                None,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(OnlineMotorActivityLevelResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_player_create_psn_session_request(
    state: &mut State,
    req: PlayerCreatePsnSessionRequest,
) -> Result<PlayerCreatePsnSessionResponse, String> {
    if !req.PsnSessionId.is_empty() {
        pb_kv::persist_str(
            &state.fs,
            &mut state.player_components.progress,
            "psn:session_id",
            &String::from_utf8_lossy(&req.PsnSessionId),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(PlayerCreatePsnSessionResponse::default())
}

pub fn handle_player_psn_info_get_request(
    state: &mut State,
    req: PlayerPsnInfoGetRequest,
) -> Result<PlayerPsnInfoGetResponse, String> {
    if !req.PsnOnlineId.is_empty() {
        pb_kv::persist_str(
            &state.fs,
            &mut state.player_components.progress,
            "psn:online_id",
            &String::from_utf8_lossy(&req.PsnOnlineId),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(PlayerPsnInfoGetResponse {
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_player_xbox_info_get_request(
    state: &mut State,
    req: PlayerXboxInfoGetRequest,
) -> Result<PlayerXboxInfoGetResponse, String> {
    if !req.XboxOnlineId.is_empty() {
        pb_kv::persist_str(
            &state.fs,
            &mut state.player_components.progress,
            "xbox:online_id",
            &String::from_utf8_lossy(&req.XboxOnlineId),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(PlayerXboxInfoGetResponse {
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_psn_setting_update_request(
    state: &mut State,
    req: PsnSettingUpdateRequest,
) -> Result<PsnSettingUpdateResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "psn:only_play",
        i64::from(req.MatchPsnUser),
    )
    .map_err(|e| e.to_string())?;
    Ok(PsnSettingUpdateResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_xbox_setting_update_request(
    state: &mut State,
    req: XboxSettingUpdateRequest,
) -> Result<XboxSettingUpdateResponse, String> {
    let info = &mut state.player_components.progress.info;
    info.set_key_int("setting:xbox_match", i64::from(req.MatchXboxUser));
    info.set_key_int(
        "setting:xbox_block_ugc",
        i64::from(req.XboxBlockUserCreatedContentViewing),
    );
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(XboxSettingUpdateResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_common_touch_ui_setting_update_request(
    state: &mut State,
    req: CommonTouchUiSettingUpdateRequest,
) -> Result<CommonTouchUiSettingUpdateResponse, String> {
    for btn in &req.MobileButtonSettings {
        pb_kv::save_msg(
            &state.fs,
            &mut state.player_components.progress,
            &format!("ui:touch_btn:{}", btn.Id),
            btn,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(CommonTouchUiSettingUpdateResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_mobile_button_setting_update_request(
    state: &mut State,
    req: MobileButtonSettingUpdateRequest,
) -> Result<MobileButtonSettingUpdateResponse, String> {
    for btn in &req.MobileButtonSettings {
        pb_kv::save_msg(
            &state.fs,
            &mut state.player_components.progress,
            &format!("ui:mobile_btn:{}", btn.Id),
            btn,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(MobileButtonSettingUpdateResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_chat_mute_player_request(
    state: &mut State,
    req: ChatMutePlayerRequest,
) -> Result<ChatMutePlayerResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("chat_mute:{}", req.TargetUID),
        i64::from(req.Mute),
    )
    .map_err(|e| e.to_string())?;
    Ok(ChatMutePlayerResponse {
        RemoveMutes: if !req.Mute {
            vec![req.TargetUID]
        } else {
            Vec::new()
        },
        ErrorCode: Some(ErrorCode::Success),
    })
}

// =========================================================================
// Client Pushes & UDP Movement Broadcast
// =========================================================================

pub fn handle_entity_simply_move_info_package_push(
    state: &mut State,
    push: EntitySimplyMoveInfoPackagePush,
) -> Result<(), String> {
    if state.scene.is_none() {
        return Ok(());
    }
    let mut changed = false;
    for mv in push.MoveInfos {
        let Some(loc) = mv.Location else {
            continue;
        };
        let pos = [loc.X, loc.Y, loc.Z];
        if persist_world_entity_transform(state, mv.EntityId, pos, mv.Rotation)? {
            changed = true;
        }
        if clear_reached_scene_item_move_target(state, mv.EntityId, pos) {
            changed = true;
        }
    }
    if changed {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn handle_move_scene_item_push(
    state: &mut State,
    push: MoveSceneItemPush,
) -> Result<(), String> {
    if state.scene.is_none() {
        return Ok(());
    }
    let Some(combat_common) = push.CombatCommon else {
        return Ok(());
    };
    let Some(mv) = push.MoveInfos.into_iter().last() else {
        return Ok(());
    };
    let Some(loc) = mv.Location else {
        return Ok(());
    };
    let pos = [loc.X, loc.Y, loc.Z];
    let mut changed =
        persist_world_entity_transform(state, combat_common.EntityId, pos, mv.Rotation)?;
    if clear_reached_scene_item_move_target(state, combat_common.EntityId, pos) {
        changed = true;
    }
    if changed {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn clear_reached_scene_item_move_target(
    state: &mut State,
    net_id: i64,
    location: [f32; 3],
) -> bool {
    if !finite_vec3(location) {
        return false;
    }
    let Some(config_id) = state
        .scene
        .as_ref()
        .and_then(|s| s.net_id_map.get(&net_id).map(|&idx| s.entities[idx].config.config_id))
    else {
        return false;
    };
    if config_id <= 0 {
        return false;
    }
    let active_key = format!("scene_item_move_target_active:{config_id}");
    let info = &mut state.player_components.progress.info;
    if info.get_key_int(&active_key) != 1 {
        return false;
    }
    let mut target = [0.0f32; 3];
    for (i, field) in ["x", "y", "z"].into_iter().enumerate() {
        let Ok(raw) = u32::try_from(info.get_key_int(&format!("scene_item_move_target:{config_id}:{field}")))
        else {
            return false;
        };
        target[i] = f32::from_bits(raw);
        if !target[i].is_finite() {
            return false;
        }
    }
    if dist_sq3(location, target) > 1.0 {
        return false;
    }
    info.set_key_int(&active_key, 0);
    true
}

fn persist_world_entity_transform(
    state: &mut State,
    net_id: i64,
    location: [f32; 3],
    rotation_opt: Option<Rotator>,
) -> Result<bool, String> {
    if !finite_vec3(location) {
        return Ok(false);
    }
    if let Some(rot) = &rotation_opt
        && !finite_vec3([rot.Roll, rot.Pitch, rot.Yaw])
    {
        return Ok(false);
    }
    let (config_id, saved_pos) = {
        let Some(scene) = state.scene.as_mut() else {
            return Ok(false);
        };
        let Some(&index) = scene.net_id_map.get(&net_id) else {
            return Ok(false);
        };
        let config = scene.entities[index].config;
        if config.config_id <= 0
            || config.entity_type == EntityType::Player
            || config.entity_type == EntityType::PlayerEntity
            || scene.entities[index].player_id.is_some()
        {
            return Ok(false);
        }
        let yaw_opt = rotation_opt.as_ref().map(|r| r.Yaw);
        if !scene.set_entity_position(net_id, location, yaw_opt) {
            return Ok(false);
        }
        if let Some(rot) = &rotation_opt
            && let Some(position) = scene.entities[index].position.as_mut()
        {
            position.rotation = [rot.Roll, rot.Pitch, rot.Yaw];
        }
        scene
            .save_entity_by_net_id(&state.fs, net_id)
            .map_err(|e| e.to_string())?;
        (config.config_id, scene.entities[index].position)
    };
    let Some(position) = saved_pos else {
        return Ok(true);
    };
    let parts = [
        ("x", position.location[0]),
        ("y", position.location[1]),
        ("z", position.location[2]),
        ("roll", position.rotation[0]),
        ("pitch", position.rotation[1]),
        ("yaw", position.rotation[2]),
    ];
    let info = &mut state.player_components.progress.info;
    for (part, value) in parts {
        info.set_key_int(&format!("world_pos:{config_id}:{part}"), i64::from(value.to_bits()));
    }
    Ok(true)
}

fn finite_vec3(value: [f32; 3]) -> bool {
    value[0].is_finite() && value[1].is_finite() && value[2].is_finite()
}

fn scene_item_movement_disabled(state: &State, instance_id: i32, config_id: i32) -> bool {
    let Some(config) = state
        .assets
        .find_level_entity(instance_id, i64::from(config_id))
    else {
        return false;
    };
    let mut components = config.ComponentsData.clone();
    if let Some(template) = state.assets.find_template_config(&config.BlueprintType) {
        template.ComponentsData.merge_into(&mut components);
    }
    components
        .SceneItemMovementComponent
        .and_then(|m| m.Disabled)
        .unwrap_or(false)
}

pub fn handle_character_move_to_point_end_push(
    state: &mut State,
    push: CharacterMoveToPointEndPush,
) -> Result<(), String> {
    let pos_opt = state
        .scene
        .as_ref()
        .and_then(|s| s.net_id_map.get(&push.EntityId).and_then(|&i| s.entities[i].position));
    let Some(pos) = pos_opt else {
        return Ok(());
    };
    if persist_world_entity_transform(
        state,
        push.EntityId,
        pos.location,
        Some(Rotator {
            Roll: pos.rotation[0],
            Pitch: pos.rotation[1],
            Yaw: pos.rotation[2],
        }),
    )? {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn handle_follower_chair_presentation_push(
    state: &mut State,
    push: FollowerChairPresentationPush,
) -> Result<(), String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("chair:pres:{}", push.EntityId),
        i64::from(push.IsShow),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn handle_hold_weapon_push(
    state: &mut State,
    push: HoldWeaponPush,
) -> Result<(), String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("role:hold_weapon:{}", push.EntityId),
        push.WeaponEntityId,
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut() {
        scene.mark_fsm_dirty(push.EntityId, wake_reason::BLACKBOARD);
    }
    flush_quest_condition_changed(state);
    Ok(())
}

pub fn handle_photograph_push(state: &mut State, _push: PhotographPush) -> Result<(), String> {
    let mut events = EventQueue::new();
    let handled = {
        let mut qctx = state.quest_context(&mut events);
        if let Some(mut lp_ctx) = level_play::LevelPlayContext::from_quest_ctx(&mut qctx) {
            let _ = level_play::on_submit_photo_level_play_node(&mut lp_ctx, 0, 0, 0);
            true
        } else {
            false
        }
    };
    drain_event_queue_best_effort(&mut events, state);
    if !handled {
        state
            .player_components
            .progress
            .info
            .add_key_int("explore:photo_cnt", 1);
    }
    let _ = state.player_components.progress.save(&state.fs);
    flush_quest_condition_changed(state);
    Ok(())
}

pub fn handle_play_entity_montage_push(
    state: &mut State,
    push: PlayEntityMontagePush,
) -> Result<(), String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "ent:montage_hash",
        i64::from(push.MontageHashCode),
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut() {
        let dirty_nids: Vec<i64> = scene
            .entities
            .iter()
            .filter(|e| e.fsm.is_some())
            .map(|e| e.net_id)
            .collect();
        for nid in dirty_nids {
            scene.mark_fsm_dirty(nid, wake_reason::CLIENT_PASS);
        }
    }
    Ok(())
}

pub fn handle_road_network_entity_update_cur_road_push(
    state: &mut State,
    push: RoadNetworkEntityUpdateCurRoadPush,
) -> Result<(), String> {
    let info = &mut state.player_components.progress.info;
    info.set_key_int(&format!("road:cur:{}", push.EntityId), i64::from(push.CurRoadId));
    info.set_key_int(
        &format!("road:idx:{}", push.EntityId),
        i64::from(push.CurRoadIndex),
    );
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(())
}

pub fn handle_test_spawn_template_entity_push(
    state: &mut State,
    push: TestSpawnTemplateEntityPush,
) -> Result<(), String> {
    if push.ConfigId > 0 {
        pb_kv::persist_int(
            &state.fs,
            &mut state.player_components.progress,
            &format!("debug:test_spawn:{}", push.ConfigId),
            i64::from(push.Level),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn handle_player_net_state_back_server_push(
    _state: &mut State,
    _push: PlayerNetStateBackServerPush,
) -> Result<(), String> {
    Ok(())
}

pub fn handle_exit_game_push(_state: &mut State, _push: ExitGamePush) -> Result<(), String> {
    tracing::debug!(target: "world_puzzle", "client sent ExitGamePush");
    Ok(())
}

pub fn handle_udp_move_package_push(
    state: &mut State,
    push: UDPMovePackagePush,
) -> Result<(), String> {
    let sender_id = state.player_id;
    let assets = state.assets.clone();
    let mut events = EventQueue::new();
    let mut nexus_check_locs: Vec<[f32; 3]> = Vec::new();
    let mut move_quest_positions: Vec<[f32; 3]> = Vec::new();
    if let Some(scene) = state.scene.as_mut() {
        let has_active_quests = !state.player_components.quest.info.active_quests.is_empty();
        for data in &push.MovingEntities {
            let Some(&idx) = scene.net_id_map.get(&data.EntityId) else {
                continue;
            };
            if scene.entities[idx].position.is_none() {
                continue;
            }
            let is_occupied_player_vehicle = scene.entities[idx]
                .vehicle
                .as_ref()
                .map(|veh| {
                    veh.occupied
                        && scene.entities[idx]
                            .player_id
                            .map(|p| p == sender_id)
                            .unwrap_or(true)
                })
                .unwrap_or(false);
            let controlled_by_player = scene.entities[idx].direct_control
                || is_occupied_player_vehicle;
            let Some(last_move) = data.MoveInfos.last() else {
                continue;
            };
            let Some(loc) = &last_move.Location else {
                continue;
            };
            let next_location = [loc.X, loc.Y, loc.Z];
            if !finite_vec3(next_location) {
                continue;
            }
            if let Some(rot) = &last_move.Rotation
                && !finite_vec3([rot.Roll, rot.Pitch, rot.Yaw])
            {
                continue;
            }
            if controlled_by_player {
                scene.player_movement_mode = last_move.MovementMode;
                for player in &mut scene.instance.players {
                    if player.id == sender_id {
                        player.location = next_location;
                        let dx = player.location[0] - scene.last_spatial_check_loc[0];
                        let dy = player.location[1] - scene.last_spatial_check_loc[1];
                        let dz = player.location[2] - scene.last_spatial_check_loc[2];
                        if dx * dx + dy * dy + dz * dz >= 2500.0 * 2500.0 {
                            scene.last_spatial_check_loc = player.location;
                            events.enqueue(Event::SpatialSpawnsCheck {
                                player_id: sender_id,
                            });
                        }
                        if has_active_quests
                            && let Some(pos) = scene.entities[idx].position
                            && dist_sq3(player.location, pos.location) >= 100.0 * 100.0
                        {
                            move_quest_positions.push(player.location);
                        }
                    }
                }
            }
            if let Some(pos) = scene.entities[idx].position.as_mut() {
                pos.location = next_location;
                if let Some(rot) = &last_move.Rotation {
                    let rvec = [rot.Roll, rot.Pitch, rot.Yaw];
                    if controlled_by_player {
                        for player in &mut scene.instance.players {
                            if player.id == sender_id {
                                player.rotation = rvec;
                            }
                        }
                    }
                    pos.rotation = rvec;
                }
                events.enqueue(Event::EntityMovement {
                    player_id: sender_id,
                    entity_net_id: data.EntityId,
                    pos: pos.location,
                    rot: pos.rotation,
                });
            }
            if controlled_by_player {
                nexus_check_locs.push(next_location);
            }
        }
    }
    for loc in nexus_check_locs {
        if let Some(scene) = state.scene.as_mut() {
            role_helper::heal_formation_if_near_nexus(
                &mut state.conn,
                scene,
                &assets,
                &mut state.player_components.role,
                &mut events,
                &state.fs,
                loc,
            );
        }
    }
    for pos in move_quest_positions {
        let mut qctx = state.quest_context(&mut events);
        quest_engine::on_player_move(&mut qctx, pos[0], pos[1], pos[2])
            .map_err(|e| e.to_string())?;
    }
    drain_event_queue_best_effort(&mut events, state);
    if let Some(coop) = &state.coop_service {
        coop.broadcast_move_except(sender_id, push.MovingEntities);
    }
    Ok(())
}

pub fn handle_interactive_update_request(
    state: &mut State,
    req: InteractiveUpdateRequest,
) -> Result<InteractiveUpdateResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("interact:upd:{}", req.InteractiveId),
    )
    .map_err(|e| e.to_string())?;
    flush_quest_condition_changed(state);
    Ok(InteractiveUpdateResponse {
        Code: Some(ErrorCode::Success),
        InteractiveId: req.InteractiveId,
    })
}

pub fn handle_scene_item_set_pos_sender_request(
    state: &mut State,
    req: SceneItemSetPosSenderRequest,
) -> Result<SceneItemSetPosSenderResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("scene_item:pos_sender:{}", req.EntityId),
        i64::from(req.IsControl),
    )
    .map_err(|e| e.to_string())?;
    Ok(SceneItemSetPosSenderResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}
