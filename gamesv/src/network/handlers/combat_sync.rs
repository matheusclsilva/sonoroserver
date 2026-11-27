use sonoro_gamesv_data::tables::entity_components::extra::ExtraEntry;
use sonoro_gamesv_data::tables::buff::FormationPolicy;
use sonoro_gamesv_logic::component::entity::small::EntityType;
use sonoro_gamesv_logic::event_queue::EventQueue;
use sonoro_gamesv_logic::events::{BuffAdditionEntry, Event};
use sonoro_gamesv_logic::fsm::types::wake_reason;
use sonoro_gamesv_logic::helpers::{
    attributes::{self, AttributeChangeType, AttributeValueType},
    consume::{self, CostItem},
    destructible,
    energy, level_play::{self, LevelPlayContext},
    quest_actions, quest_engine, reward, role as role_helper,
};
use sonoro_proto::enums::{BuffOperateType, EAttributeType, ErrorCode, LivingStatus};
use sonoro_proto::pb::{
    ANStartPush, ActivateBuffPush, ActiveBuffPush, ActorVisiblePush, AnimationGameplayTagPush,
    AnimationStateChangedPush, AnimationStateChangedRequest, AnimationStateChangedResponse,
    AnimationStateInitPush, ApplyBuffS2cResponsePush, ApplyGameplayEffectPush,
    AtomicProcessAddBuffRequest, AtomicProcessAddBuffResponse, AtomicProcessRemoveBuffRequest,
    AtomicProcessRemoveBuffResponse, AtomicProcessTeleportRequest, AtomicProcessTeleportResponse,
    AvoidBlockClientInitDoneRequest, AvoidBlockClientInitDoneResponse,
    AvoidBlockSubmitOverNodeRequest, AvoidBlockSubmitOverNodeResponse, BeamCastHitPlayerRequest,
    BeamCastHitPlayerResponse, BeforeHitPush, BoneVisibleChangePush, BuffEffectExecutePush,
    BuffEffectPush, BuffItem, BuffItemRequest, BuffItemResponse, BuffStackCountPush,
    BulletPatternPush, CaughtPush, ChangeStateConfirmPush, ChangeStateRequest,
    ChangeStateResponse, ChangeSummonEntityStateRequest, ChangeSummonEntityStateResponse,
    CheckClientPush, CheckUseSoundBoxSkillRequest, CheckUseSoundBoxSkillResponse,
    ClientBasicInfoRequest, ClientBasicInfoResponse, ClientCurrentRoleReportPush,
    CombatCommon, CombatNotifyData, CombatNotifyDataMessageOneof, CombatPushDataMessageOneof,
    CombatReceiveData, CombatReceiveDataMessageOneof, CombatReceivePackNotify,
    CombatRequestDataMessageOneof, CombatResponseDataMessageOneof, CreateBulletPush,
    CreateBulletRequest, CreateBulletResponse, DErrorResult, DamageExecuteRequest,
    DamageExecuteResponse,
    DeadeyeShootHitRequest, DeadeyeShootHitResponse, DestroyBulletPush, DetectionEntityPosInfo,
    DodgeInfoPush, DrownEndTeleportPush, DrownPush, EndSkillPush, EnterViewDirectionPush,
    EntityBuffProducerOperateRequest, EntityBuffProducerOperateResponse,
    EntityBuffProducerRequest, EntityBuffProducerResponse, EntityIsVisiblePush,
    EntityIsVisibleRequest, EntityIsVisibleResponse, EntityLivingStatusNotify,
    EntityStaticHookMovePush, EntityStaticHookMoveRequest, EntityStaticHookMoveResponse,
    ExecuteQtePush, ExecuteQteRequest, ExecuteQteResponse, ExitViewDirectionPush,
    ExploreSkillActionRequest, ExploreSkillActionResponse, FragileChangeRequest,
    FragileChangeResponse, FsmConditionPassPush, FsmStateBehaviorRequest,
    FsmStateBehaviorResponse, GameplayCuePush, GameplayTagPush, GmEntityFsmGroupInfoRequest,
    GmEntityFsmGroupInfoResponse, GmEntityFsmMachineInfo, GmEntityFsmStateInfo, HitEndPush,
    HitRequest, HitResponse, InterruptSkillInDelayPush, LogicStateInitPush, MaterialPush,
    ModifyBulletParamsPush, MonsterBoomPush, MonsterDrownPush, NewLinkBurstPush,
    OrderApplyBuffRequest, OrderApplyBuffResponse, OrderRemoveBuffByTagsRequest,
    OrderRemoveBuffByTagsResponse, OrderRemoveBuffRequest, OrderRemoveBuffResponse,
    PassiveSkillAddPush, PassiveSkillRemovePush, QuickHackOpenPush, QuickHackRamVerifyPush,
    QuickHackSkillRequest, QuickHackSkillResponse, RTimeStopAnimPush, RTimeStopPush,
    RefreshBuffDurationPush, RemoveBuffByIdS2cResponsePush, RemoveBuffS2cResponsePush,
    RemoveGameplayEffectPush, RoleGoDownPush, RoleSkinTrialUiEndPush, RoleTrialUiEndPush,
    SecFBRoundEndFlowReportRequest, SecFBRoundEndFlowReportResponse, SecFBRoundStartFlowRequest,
    SecFBRoundStartFlowResponse, SecRoleFightFlowRequest, SecRoleFightFlowResponse,
    SecWorldInfoFlowRequest, SecWorldInfoFlowResponse, SecWorldSEndFlowRequest,
    SecWorldSEndFlowResponse, SecWorldStartFlowRequest, SecWorldStartFlowResponse,
    SetClientGlobalVarRequest, SetClientGlobalVarResponse,
    SimpleCombatEntityDieRequest, SimpleCombatEntityDieResponse,
    SimpleCombatEntitySyncAttributePush, SimpleTrackReportMsg, SimpleTrackReportRequest,
    SimpleTrackReportResponse, SkillRequest, SkillResponse, SlowTimePush,
    SwitchLogicStatePush, SwitchRoleRequest, SwitchRoleResponse, TargetGearHitRequest,
    TargetGearHitResponse, ThrowDamageNotify, ThrowDamageRequest, ThrowDamageResponse,
    ToughCalcExtraRatioChangePush, UseSkillFailPush, UseSkillRequest, UseSkillResponse,
    UseSoundBoxSkillRequest, UseSoundBoxSkillResponse, VarDefinePbValueOneof, Vector,
    VisionTriggerPush, EntityRemoveInfo, EntityRemoveNotify,
};
use sonoro_proto::wire::PbEnum;

use super::State;

trait CombatSuccessFallback: Default {
    fn success_fallback() -> Self {
        Self::default()
    }
}

macro_rules! impl_combat_success_error_code {
    ($($ty:ty),* $(,)?) => {
        $(
            impl CombatSuccessFallback for $ty {
                #[allow(clippy::needless_update)]
                fn success_fallback() -> Self {
                    Self {
                        ErrorCode: Some(ErrorCode::Success),
                        ..Default::default()
                    }
                }
            }
        )*
    };
}

impl CombatSuccessFallback for ChangeStateResponse {}

impl_combat_success_error_code!(
    AnimationStateChangedResponse,
    CreateBulletResponse,
    DamageExecuteResponse,
    EntityIsVisibleResponse,
    EntityStaticHookMoveResponse,
    FsmStateBehaviorResponse,
    HitResponse,
    OrderApplyBuffResponse,
    OrderRemoveBuffResponse,
    OrderRemoveBuffByTagsResponse,
    SkillResponse,
    UseSkillResponse,
    SwitchRoleResponse,
    FragileChangeResponse,
);

fn forward_combat_req<T, F>(
    state: &mut State,
    events: &mut EventQueue,
    common: Option<CombatCommon>,
    req: CombatRequestDataMessageOneof,
    extract: F,
) -> T
where
    T: CombatSuccessFallback,
    F: FnOnce(CombatResponseDataMessageOneof) -> Option<T>,
{
    let mut receive_pack = Vec::new();
    let resp_msg =
        super::combat::dispatch_combat_request(state, events, common, req, &mut receive_pack);
    if !receive_pack.is_empty() {
        let _ = state.conn.push_msg(
            CombatReceivePackNotify::MSG_ID,
            &CombatReceivePackNotify { Data: receive_pack },
        );
    }
    resp_msg
        .and_then(extract)
        .unwrap_or_else(T::success_fallback)
}

fn forward_combat_push(
    state: &mut State,
    events: &mut EventQueue,
    common: Option<CombatCommon>,
    push: CombatPushDataMessageOneof,
) {
    let mut receive_pack = Vec::new();
    super::combat::dispatch_combat_push(state, events, common, push, &mut receive_pack);
    if !receive_pack.is_empty() {
        let _ = state.conn.push_msg(
            CombatReceivePackNotify::MSG_ID,
            &CombatReceivePackNotify { Data: receive_pack },
        );
    }
}

pub fn handle_animation_state_changed_request(
    state: &mut State,
    events: &mut EventQueue,
    req: AnimationStateChangedRequest,
) -> Result<AnimationStateChangedResponse, String> {
    let common = req.CombatCommon.clone();
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::AnimationStateChangedRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::AnimationStateChangedResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_create_bullet_request(
    state: &mut State,
    events: &mut EventQueue,
    req: CreateBulletRequest,
) -> Result<CreateBulletResponse, String> {
    let common = req.CombatCommon.clone();
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::CreateBulletRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::CreateBulletResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_damage_execute_request(
    state: &mut State,
    events: &mut EventQueue,
    req: DamageExecuteRequest,
) -> Result<DamageExecuteResponse, String> {
    Ok(forward_combat_req(
        state,
        events,
        None,
        CombatRequestDataMessageOneof::DamageExecuteRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::DamageExecuteResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_entity_is_visible_request(
    state: &mut State,
    events: &mut EventQueue,
    req: EntityIsVisibleRequest,
) -> Result<EntityIsVisibleResponse, String> {
    let common = req.CombatCommon.clone().or_else(|| {
        (req.Id != 0).then_some(CombatCommon {
            EntityId: req.Id,
            ..Default::default()
        })
    });
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::EntityIsVisibleRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::EntityIsVisibleResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_entity_static_hook_move_request(
    state: &mut State,
    events: &mut EventQueue,
    req: EntityStaticHookMoveRequest,
) -> Result<EntityStaticHookMoveResponse, String> {
    let common = (req.EntityId != 0).then_some(CombatCommon {
        EntityId: req.EntityId,
        ..Default::default()
    });
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::EntityStaticHookMoveRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::EntityStaticHookMoveResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_fsm_state_behavior_request(
    state: &mut State,
    events: &mut EventQueue,
    req: FsmStateBehaviorRequest,
) -> Result<FsmStateBehaviorResponse, String> {
    Ok(forward_combat_req(
        state,
        events,
        None,
        CombatRequestDataMessageOneof::FsmStateBehaviorRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::FsmStateBehaviorResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_hit_request(
    state: &mut State,
    events: &mut EventQueue,
    req: HitRequest,
) -> Result<HitResponse, String> {
    let common = req.CombatCommon.clone();
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::HitRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::HitResponse(v) => Some(v),
            _ => None,
        },
    ))
}

fn common_from_id(id: i64) -> Option<CombatCommon> {
    (id != 0).then(|| CombatCommon {
        EntityId: id,
        ..Default::default()
    })
}

pub fn handle_order_apply_buff_request(
    state: &mut State,
    events: &mut EventQueue,
    req: OrderApplyBuffRequest,
) -> Result<OrderApplyBuffResponse, String> {
    let common = common_from_id(req.Id);
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::OrderApplyBuffRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::OrderApplyBuffResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_order_remove_buff_request(
    state: &mut State,
    events: &mut EventQueue,
    req: OrderRemoveBuffRequest,
) -> Result<OrderRemoveBuffResponse, String> {
    let common = common_from_id(req.Id);
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::OrderRemoveBuffRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::OrderRemoveBuffResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_order_remove_buff_by_tags_request(
    state: &mut State,
    events: &mut EventQueue,
    req: OrderRemoveBuffByTagsRequest,
) -> Result<OrderRemoveBuffByTagsResponse, String> {
    Ok(forward_combat_req(
        state,
        events,
        None,
        CombatRequestDataMessageOneof::OrderRemoveBuffByTagsRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::OrderRemoveBuffByTagsResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_skill_request(
    state: &mut State,
    events: &mut EventQueue,
    req: SkillRequest,
) -> Result<SkillResponse, String> {
    Ok(forward_combat_req(
        state,
        events,
        None,
        CombatRequestDataMessageOneof::SkillRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::SkillResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_use_skill_request(
    state: &mut State,
    events: &mut EventQueue,
    req: UseSkillRequest,
) -> Result<UseSkillResponse, String> {
    let mut skill_ids = Vec::new();
    if req.SkillSingleId > 0 {
        skill_ids.push(i64::from(req.SkillSingleId));
    }
    if let Some(info) = req.UseSkillInfo.as_ref()
        && info.SkillId > 0
    {
        skill_ids.push(info.SkillId);
    }
    let common = req.CombatCommon.clone();
    let resp = forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::UseSkillRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::UseSkillResponse(v) => Some(v),
            _ => None,
        },
    );
    if !skill_ids.is_empty() {
        let mut dirty = false;
        for sid in skill_ids {
            if state
                .player_components
                .progress
                .info
                .mark_claim(&format!("skill:used:{sid}"))
            {
                dirty = true;
            }
        }
        if dirty {
            let _ = state.player_components.progress.save(&state.fs);
        }
        if state.scene.is_some() {
            let mut qctx = state.quest_context(events);
            let _ = quest_engine::events::on_condition_state_changed(&mut qctx);
        }
    }
    Ok(resp)
}

pub fn handle_change_state_request(
    state: &mut State,
    events: &mut EventQueue,
    req: ChangeStateRequest,
) -> Result<ChangeStateResponse, String> {
    Ok(forward_combat_req(
        state,
        events,
        None,
        CombatRequestDataMessageOneof::ChangeStateRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::ChangeStateResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_switch_role_request(
    state: &mut State,
    events: &mut EventQueue,
    req: SwitchRoleRequest,
) -> Result<SwitchRoleResponse, String> {
    Ok(forward_combat_req(
        state,
        events,
        None,
        CombatRequestDataMessageOneof::SwitchRoleRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::SwitchRoleResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_fragile_change_request(
    state: &mut State,
    events: &mut EventQueue,
    req: FragileChangeRequest,
) -> Result<FragileChangeResponse, String> {
    let common = (req.EntityId != 0).then_some(CombatCommon {
        EntityId: req.EntityId,
        ..Default::default()
    });
    Ok(forward_combat_req(
        state,
        events,
        common,
        CombatRequestDataMessageOneof::FragileChangeRequest(req),
        |r| match r {
            CombatResponseDataMessageOneof::FragileChangeResponse(v) => Some(v),
            _ => None,
        },
    ))
}

pub fn handle_change_summon_entity_state_request(
    state: &mut State,
    req: ChangeSummonEntityStateRequest,
) -> Result<ChangeSummonEntityStateResponse, String> {
    if req.SummonEntityId != 0 {
        state.player_components.progress.info.set_key_int(
            &format!("summon_state:{}", req.SummonEntityId),
            i64::from(req.TagId),
        );
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(ChangeSummonEntityStateResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

macro_rules! fwd_push {
    ($fn_name:ident, $push_ty:ty, $variant:ident) => {
        pub fn $fn_name(state: &mut State, events: &mut EventQueue, msg: $push_ty) -> Result<(), String> {
            forward_combat_push(state, events, None, CombatPushDataMessageOneof::$variant(msg));
            Ok(())
        }
    };
}

macro_rules! fwd_push_common {
    ($fn_name:ident, $push_ty:ty, $variant:ident) => {
        pub fn $fn_name(state: &mut State, events: &mut EventQueue, msg: $push_ty) -> Result<(), String> {
            let common = msg.CombatCommon.clone();
            forward_combat_push(state, events, common, CombatPushDataMessageOneof::$variant(msg));
            Ok(())
        }
    };
}

macro_rules! fwd_push_id {
    ($fn_name:ident, $push_ty:ty, $variant:ident) => {
        pub fn $fn_name(state: &mut State, events: &mut EventQueue, msg: $push_ty) -> Result<(), String> {
            let common = common_from_id(msg.Id);
            forward_combat_push(state, events, common, CombatPushDataMessageOneof::$variant(msg));
            Ok(())
        }
    };
}

fwd_push!(handle_an_start_push, ANStartPush, ANStartPush);
pub fn handle_activate_buff_push(
    state: &mut State,
    events: &mut EventQueue,
    msg: ActivateBuffPush,
) -> Result<(), String> {
    forward_combat_push(
        state,
        events,
        None,
        CombatPushDataMessageOneof::ActiveBuffPush(ActiveBuffPush {
            Handle: msg.Handle,
            On: msg.On,
        }),
    );
    Ok(())
}
fwd_push!(handle_animation_gameplay_tag_push, AnimationGameplayTagPush, AnimationGameplayTagPush);
fwd_push_common!(handle_animation_state_changed_push, AnimationStateChangedPush, AnimationStateChangedPush);
fwd_push_common!(handle_animation_state_init_push, AnimationStateInitPush, AnimationStateInitPush);
fwd_push!(handle_apply_buff_s2c_response_push, ApplyBuffS2cResponsePush, ApplyBuffS2cResponsePush);
fwd_push_id!(handle_apply_gameplay_effect_push, ApplyGameplayEffectPush, ApplyGameplayEffectPush);
fwd_push!(handle_buff_effect_execute_push, BuffEffectExecutePush, BuffEffectExecutePush);
fwd_push!(handle_buff_effect_push, BuffEffectPush, BuffEffectPush);
fwd_push!(handle_buff_stack_count_push, BuffStackCountPush, BuffStackCountPush);
fwd_push!(handle_bullet_pattern_push, BulletPatternPush, BulletPatternPush);
fwd_push!(handle_client_current_role_report_push, ClientCurrentRoleReportPush, ClientCurrentRoleReportPush);
fwd_push!(handle_dodge_info_push, DodgeInfoPush, DodgeInfoPush);
fwd_push_common!(handle_end_skill_push, EndSkillPush, EndSkillPush);
fwd_push!(handle_fsm_condition_pass_push, FsmConditionPassPush, FsmConditionPassPush);
fwd_push!(handle_gameplay_cue_push, GameplayCuePush, GameplayCuePush);
fwd_push_common!(handle_hit_end_push, HitEndPush, HitEndPush);
fwd_push!(handle_interrupt_skill_in_delay_push, InterruptSkillInDelayPush, InterruptSkillInDelayPush);
pub fn handle_logic_state_init_push(
    state: &mut State,
    events: &mut EventQueue,
    msg: LogicStateInitPush,
) -> Result<(), String> {
    let common = msg.CombatCommon.clone().or_else(|| {
        (msg.EntityId != 0).then_some(CombatCommon {
            EntityId: msg.EntityId,
            ..Default::default()
        })
    });
    forward_combat_push(
        state,
        events,
        common,
        CombatPushDataMessageOneof::LogicStateInitPush(msg),
    );
    Ok(())
}
fwd_push!(handle_modify_bullet_params_push, ModifyBulletParamsPush, ModifyBulletParamsPush);
fwd_push!(handle_passive_skill_add_push, PassiveSkillAddPush, PassiveSkillAddPush);
fwd_push!(handle_passive_skill_remove_push, PassiveSkillRemovePush, PassiveSkillRemovePush);
fwd_push!(handle_quick_hack_open_push, QuickHackOpenPush, QuickHackOpenPush);
fwd_push!(handle_quick_hack_ram_verify_push, QuickHackRamVerifyPush, QuickHackRamVerifyPush);
fwd_push!(handle_r_time_stop_push, RTimeStopPush, RTimeStopPush);
fwd_push!(handle_refresh_buff_duration_push, RefreshBuffDurationPush, RefreshBuffDurationPush);
fwd_push!(handle_remove_buff_by_id_s2c_response_push, RemoveBuffByIdS2cResponsePush, RemoveBuffByIdS2cResponsePush);
fwd_push!(handle_remove_buff_s2c_response_push, RemoveBuffS2cResponsePush, RemoveBuffS2cResponsePush);
pub fn handle_remove_gameplay_effect_push(
    state: &mut State,
    events: &mut EventQueue,
    msg: RemoveGameplayEffectPush,
) -> Result<(), String> {
    let common = (msg.EntityId != 0).then_some(CombatCommon {
        EntityId: msg.EntityId,
        ..Default::default()
    });
    forward_combat_push(
        state,
        events,
        common,
        CombatPushDataMessageOneof::RemoveGameplayEffectPush(msg),
    );
    Ok(())
}
fwd_push!(handle_role_go_down_push, RoleGoDownPush, RoleGoDownPush);
fwd_push!(handle_switch_logic_state_push, SwitchLogicStatePush, SwitchLogicStatePush);
fwd_push!(handle_tough_calc_extra_ratio_change_push, ToughCalcExtraRatioChangePush, ToughCalcExtraRatioChangePush);
fwd_push!(handle_use_skill_fail_push, UseSkillFailPush, UseSkillFailPush);
fwd_push!(handle_change_state_confirm_push, ChangeStateConfirmPush, ChangeStateConfirmPush);
pub fn handle_actor_visible_push(
    state: &mut State,
    events: &mut EventQueue,
    msg: ActorVisiblePush,
) -> Result<(), String> {
    let common = (msg.Id != 0).then_some(CombatCommon {
        EntityId: msg.Id,
        ..Default::default()
    });
    forward_combat_push(state, events, common, CombatPushDataMessageOneof::ActorVisiblePush(msg));
    Ok(())
}
pub fn handle_entity_is_visible_push(
    state: &mut State,
    events: &mut EventQueue,
    msg: EntityIsVisiblePush,
) -> Result<(), String> {
    let common = msg.CombatCommon.clone().or_else(|| {
        (msg.Id != 0).then_some(CombatCommon {
            EntityId: msg.Id,
            ..Default::default()
        })
    });
    forward_combat_push(
        state,
        events,
        common,
        CombatPushDataMessageOneof::EntityIsVisiblePush(msg),
    );
    Ok(())
}
fwd_push!(handle_bone_visible_change_push, BoneVisibleChangePush, BoneVisibleChangePush);
fwd_push!(handle_caught_push, CaughtPush, CaughtPush);
fwd_push!(handle_drown_end_teleport_push, DrownEndTeleportPush, DrownEndTeleportPush);
fwd_push!(handle_drown_push, DrownPush, DrownPush);
fwd_push!(handle_enter_view_direction_push, EnterViewDirectionPush, EnterViewDirectionPush);
fwd_push!(handle_exit_view_direction_push, ExitViewDirectionPush, ExitViewDirectionPush);
pub fn handle_entity_static_hook_move_push(
    state: &mut State,
    events: &mut EventQueue,
    msg: EntityStaticHookMovePush,
) -> Result<(), String> {
    let common = (msg.EntityId != 0).then_some(CombatCommon {
        EntityId: msg.EntityId,
        ..Default::default()
    });
    forward_combat_push(
        state,
        events,
        common,
        CombatPushDataMessageOneof::EntityStaticHookMovePush(msg),
    );
    Ok(())
}
fwd_push!(handle_material_push, MaterialPush, MaterialPush);
fwd_push!(handle_monster_boom_push, MonsterBoomPush, MonsterBoomPush);
fwd_push!(handle_monster_drown_push, MonsterDrownPush, MonsterDrownPush);
fwd_push!(handle_new_link_burst_push, NewLinkBurstPush, NewLinkBurstPush);
fwd_push!(handle_vision_trigger_push, VisionTriggerPush, VisionTriggerPush);

pub fn handle_before_hit_push(
    state: &mut State,
    _events: &mut EventQueue,
    _msg: BeforeHitPush,
) -> Result<(), String> {
    if let Some(scene) = state.scene.as_mut() {
        let nid = scene.current_role_entity_id();
        if nid > 0 {
            scene.mark_fsm_dirty(nid, wake_reason::EVENT);
        }
    }
    Ok(())
}

pub fn handle_create_bullet_push(
    state: &mut State,
    _events: &mut EventQueue,
    msg: CreateBulletPush,
) -> Result<(), String> {
    if let Some(scene) = state.scene.as_mut() {
        if msg.TargetId > 0 {
            scene.mark_fsm_dirty(msg.TargetId, wake_reason::EVENT);
        }
        if let Some(cc) = msg.CombatCommon.as_ref()
            && cc.EntityId > 0
        {
            scene.mark_fsm_dirty(cc.EntityId, wake_reason::EVENT);
        }
    }
    Ok(())
}

pub fn handle_destroy_bullet_push(
    state: &mut State,
    _events: &mut EventQueue,
    msg: DestroyBulletPush,
) -> Result<(), String> {
    if let Some(scene) = state.scene.as_mut()
        && let Some(cc) = msg.CombatCommon.as_ref()
        && cc.EntityId > 0
    {
        scene.mark_fsm_dirty(cc.EntityId, wake_reason::SKILL_END);
    }
    Ok(())
}

pub fn handle_gameplay_tag_push(
    state: &mut State,
    events: &mut EventQueue,
    msg: GameplayTagPush,
) -> Result<(), String> {
    forward_combat_push(
        state,
        events,
        None,
        CombatPushDataMessageOneof::AnimationGameplayTagPush(AnimationGameplayTagPush {
            AddTagIds: msg.TagId,
            RemoveTagIds: msg.TagCount <= 0,
        }),
    );
    Ok(())
}

pub fn handle_r_time_stop_anim_push(
    _state: &mut State,
    _events: &mut EventQueue,
    _msg: RTimeStopAnimPush,
) -> Result<(), String> {
    Ok(())
}

pub fn handle_slow_time_push(
    _state: &mut State,
    _events: &mut EventQueue,
    _msg: SlowTimePush,
) -> Result<(), String> {
    Ok(())
}

pub fn handle_role_trial_ui_end_push(
    state: &mut State,
    events: &mut EventQueue,
    _msg: RoleTrialUiEndPush,
) -> Result<(), String> {
    let mut qctx = state.quest_context(events);
    let _ = quest_engine::events::on_condition_state_changed(&mut qctx);
    Ok(())
}

pub fn handle_role_skin_trial_ui_end_push(
    state: &mut State,
    events: &mut EventQueue,
    _msg: RoleSkinTrialUiEndPush,
) -> Result<(), String> {
    let mut qctx = state.quest_context(events);
    let _ = quest_engine::events::on_condition_state_changed(&mut qctx);
    Ok(())
}

pub fn handle_check_client_push(
    _state: &mut State,
    _events: &mut EventQueue,
    _msg: CheckClientPush,
) -> Result<(), String> {
    Ok(())
}

fn push_execute_qte_notify(
    state: &mut State,
    down_entity_id: i64,
    up_entity_id: i64,
    fnv_hash: i32,
) {
    let _ = state.conn.push_msg(
        CombatReceivePackNotify::MSG_ID,
        &CombatReceivePackNotify {
            Data: vec![CombatReceiveData {
                Message: Some(CombatReceiveDataMessageOneof::CombatNotifyData(
                    CombatNotifyData {
                        CombatCommon: Some(CombatCommon {
                            EntityId: up_entity_id,
                            ..Default::default()
                        }),
                        Message: Some(CombatNotifyDataMessageOneof::ExecuteQteNotify(
                            sonoro_proto::pb::ExecuteQteNotify {
                                DownEntityId: down_entity_id,
                                UpEntityId: up_entity_id,
                                FnvHash: fnv_hash,
                            },
                        )),
                    },
                )),
            }],
        },
    );
}

pub fn handle_execute_qte_push(
    state: &mut State,
    _events: &mut EventQueue,
    msg: ExecuteQtePush,
) -> Result<(), String> {
    push_execute_qte_notify(state, msg.DownEntityId, msg.UpEntityId, msg.FnvHash);
    Ok(())
}

pub fn handle_execute_qte_request(
    state: &mut State,
    req: ExecuteQteRequest,
) -> Result<ExecuteQteResponse, String> {
    push_execute_qte_notify(state, req.DownEntityId, req.UpEntityId, req.FnvHash);
    Ok(ExecuteQteResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_buff_item_request(
    state: &mut State,
    events: &mut EventQueue,
    req: BuffItemRequest,
) -> Result<BuffItemResponse, String> {
    let count = if req.Num > 0 { req.Num } else { 1 };
    let buff_cfg = state
        .assets
        .tables
        .buff_item
        .get_data_by_id(&req.ItemId)
        .map(|b| b.into_owned());
    let cd_sec = i64::from(buff_cfg.as_ref().map_or(10, |bi| bi.Cd.max(1)));
    let now_ms = energy::now_seconds() * 1000;
    let key = format!("buff_cd:{}", req.ItemId);
    if state.player_components.progress.info.get_key_int(&key) > now_ms {
        return Ok(BuffItemResponse {
            ErrorCode: Some(ErrorCode::ErrBuffItemCdLimit),
            RoleId: req.RoleId,
            ..Default::default()
        });
    }

    if req.ItemId > 0 {
        let costs = [CostItem {
            id: req.ItemId,
            count,
        }];
        let paid = consume::consume_costs(
            &mut state.conn,
            &state.fs,
            &mut state.player_components.basic,
            &mut state.player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?;
        if !paid {
            return Ok(BuffItemResponse {
                ErrorCode: Some(ErrorCode::ErrItemNotEnough),
                RoleId: req.RoleId,
                ..Default::default()
            });
        }
    }

    let cd_until = now_ms + cd_sec * 1000;
    state
        .player_components
        .progress
        .info
        .set_key_int(&key, cd_until);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;

    if let Some(bi) = buff_cfg
        && !bi.Buffs.is_empty()
    {
        let mut pack_data = Vec::new();
        let mut any_revived = false;
        let mut modified_role = None;
        let mut found_ent = None;

        if let Some(scene) = state.scene.as_mut() {
            let target_role = if req.RoleId > 0 {
                req.RoleId
            } else {
                usize::try_from(scene.formation_info.cur_formation)
                    .ok()
                    .and_then(|f_idx| scene.formation_info.formations.get(f_idx))
                    .map_or(0, |f| f.cur_role)
            };
            if target_role > 0
                && let Some(idx) = scene.entities.iter().position(|e| {
                    e.config.entity_type == EntityType::Player
                        && e.config.config_type == sonoro_gamesv_logic::component::entity::small::ConfigType::Character
                        && e.config.config_id == target_role
                })
            {
                let net_id = scene.entities[idx].net_id;
                let handle = scene.entity_handle(net_id);
                found_ent = handle;
                for &bid in &bi.Buffs {
                    let Some(b) = state.assets.tables.buff.get_data_by_id(&bid) else {
                        continue;
                    };
                    if b.ExtraEffectID == 101 {
                        if let Some(attr) = scene.entities[idx].attribute.as_mut() {
                            let life_max =
                                attributes::get_attr_value(attr, EAttributeType::LifeMax).max(1);
                            let cur_life = attributes::get_attr_value(attr, EAttributeType::Life);
                            let flat_hp = b
                                .ExtraEffectParameters
                                .first()
                                .and_then(|s| s.trim().parse::<i32>().ok())
                                .unwrap_or(0);
                            let pct_permyriad = b
                                .ExtraEffectParameters
                                .get(1)
                                .and_then(|s| s.trim().parse::<i32>().ok())
                                .unwrap_or(500);
                            let pct_hp =
                                ((i64::from(life_max) * i64::from(pct_permyriad)) / 10_000) as i32;
                            let heal_amt = (flat_hp + pct_hp).max(1);
                            if cur_life <= 0 {
                                let _ = attributes::revive_entity(attr, Some(heal_amt));
                                any_revived = true;
                                pack_data.push(CombatReceiveData {
                                    Message: Some(CombatReceiveDataMessageOneof::CombatNotifyData(
                                        CombatNotifyData {
                                            CombatCommon: Some(CombatCommon {
                                                EntityId: net_id,
                                                ..Default::default()
                                            }),
                                            Message: Some(
                                                CombatNotifyDataMessageOneof::EntityLivingStatusNotify(
                                                    EntityLivingStatusNotify {
                                                        Id: net_id,
                                                        LivingStatus: Some(LivingStatus::Alive),
                                                        ..Default::default()
                                                    },
                                                ),
                                            ),
                                        },
                                    )),
                                });
                                attributes::generate_attr_messages(
                                    &mut pack_data,
                                    net_id,
                                    attr,
                                    &[EAttributeType::Life],
                                );
                            } else {
                                let ch = attributes::change_attr(
                                    attr,
                                    EAttributeType::Life,
                                    AttributeChangeType::Delta,
                                    AttributeValueType::Current,
                                    heal_amt,
                                );
                                attributes::generate_attr_messages(
                                    &mut pack_data,
                                    net_id,
                                    attr,
                                    &ch,
                                );
                            }
                            let final_hp =
                                attributes::get_attr_value(attr, EAttributeType::Life).max(0);
                            modified_role = Some((target_role, final_hp));
                        }
                        scene.mark_fsm_dirty(net_id, wake_reason::ATTRIBUTE);
                    } else if (b.ExtraEffectID == 33 || b.ExtraEffectID == 34)
                        && let Some(attr) = scene.entities[idx].attribute.as_mut()
                    {
                            let stamina_amt = b
                                .ExtraEffectParameters
                                .first()
                                .and_then(|s| s.trim().parse::<f32>().ok())
                                .map_or(4000, |v| (v.round() as i32 * 100).max(1));
                            let ch = attributes::change_attr(
                                attr,
                                EAttributeType::Strength,
                                AttributeChangeType::Delta,
                                AttributeValueType::Current,
                                stamina_amt,
                            );
                            attributes::generate_attr_messages(&mut pack_data, net_id, attr, &ch);
                    }
                }
            }
        }

        if let Some((target_role, final_hp)) = modified_role
            && let Some(role_info) = state.player_components.role.role_map.get_mut(&target_role)
        {
            role_info.current_hp = Some(final_hp);
            events.enqueue(Event::RoleInfoModified {
                player_id: state.player_id,
                role_id: target_role,
            });
            let _ = role_helper::save_role_info(
                &state.fs,
                &state.player_components.role,
                target_role,
            );
        }
        if !pack_data.is_empty() {
            let _ = state.conn.push_msg(
                CombatReceivePackNotify::MSG_ID,
                &CombatReceivePackNotify { Data: pack_data },
            );
        }
        if any_revived {
            let formation_id = state
                .scene
                .as_ref()
                .map_or(0, |s| s.formation_info.cur_formation);
            events.enqueue(Event::UpdateFormations {
                player_id: state.player_id,
                formation_id,
                role_ids: Vec::new(),
            });
        }
        if let Some(ent) = found_ent {
            let entries: Vec<BuffAdditionEntry> = bi
                .Buffs
                .iter()
                .map(|&bid| BuffAdditionEntry {
                    id: bid,
                    stack_count: 1,
                    is_active: true,
                    ..Default::default()
                })
                .collect();
            events.enqueue(Event::BuffAddition {
                target: ent,
                instigator: ent,
                buffs: entries,
            });
        }
    }

    Ok(BuffItemResponse {
        BuffItemInfo: Some(BuffItem {
            ItemId: req.ItemId,
            CdTime: cd_until,
        }),
        RoleId: req.RoleId,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_atomic_process_add_buff_request(
    state: &mut State,
    events: &mut EventQueue,
    req: AtomicProcessAddBuffRequest,
) -> Result<AtomicProcessAddBuffResponse, String> {
    for buff_id in req.BuffIds {
        let key = format!("bt_buff:{}:{}:{buff_id}", req.TreeIncId, req.EntityId);
        state.player_components.progress.info.set_key_int(&key, 1);
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let mut qctx = state.quest_context(events);
    let _ = quest_engine::events::on_gameplay_state_changed(&mut qctx, "AtomicProcess");
    Ok(AtomicProcessAddBuffResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_atomic_process_remove_buff_request(
    state: &mut State,
    events: &mut EventQueue,
    req: AtomicProcessRemoveBuffRequest,
) -> Result<AtomicProcessRemoveBuffResponse, String> {
    for buff_id in req.BuffIds {
        let key = format!("bt_buff:{}:{}:{buff_id}", req.TreeIncId, req.EntityId);
        state.player_components.progress.info.set_key_int(&key, 0);
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let mut qctx = state.quest_context(events);
    let _ = quest_engine::events::on_gameplay_state_changed(&mut qctx, "AtomicProcess");
    Ok(AtomicProcessRemoveBuffResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_atomic_process_teleport_request(
    state: &mut State,
    events: &mut EventQueue,
    _req: AtomicProcessTeleportRequest,
) -> Result<AtomicProcessTeleportResponse, String> {
    let _ = state
        .player_components
        .progress
        .info
        .add_key_int("combat:atomic_tp", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let mut qctx = state.quest_context(events);
    let _ = quest_engine::events::on_gameplay_state_changed(&mut qctx, "AtomicProcess");
    Ok(AtomicProcessTeleportResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_avoid_block_client_init_done_request(
    state: &mut State,
    req: AvoidBlockClientInitDoneRequest,
) -> Result<AvoidBlockClientInitDoneResponse, String> {
    state.player_components.progress.info.set_key_int(
        &format!("avoid_block:init:{}", req.NodeId),
        req.TreeIncId,
    );
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(AvoidBlockClientInitDoneResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_avoid_block_submit_over_node_request(
    state: &mut State,
    req: AvoidBlockSubmitOverNodeRequest,
) -> Result<AvoidBlockSubmitOverNodeResponse, String> {
    let key = format!("avoid_block:score:{}", req.NodeId);
    if i64::from(req.Score) > state.player_components.progress.info.get_key_int(&key) {
        state
            .player_components
            .progress
            .info
            .set_key_int(&key, i64::from(req.Score));
    }
    if req.Win {
        state
            .player_components
            .progress
            .info
            .mark_claim(&format!("avoid_block:win:{}", req.NodeId));
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(AvoidBlockSubmitOverNodeResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_beam_cast_hit_player_request(
    state: &mut State,
    _req: BeamCastHitPlayerRequest,
) -> Result<BeamCastHitPlayerResponse, String> {
    let _ = state
        .player_components
        .progress
        .info
        .add_key_int("combat:beam_hit", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(BeamCastHitPlayerResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_deadeye_shoot_hit_request(
    state: &mut State,
    req: DeadeyeShootHitRequest,
) -> Result<DeadeyeShootHitResponse, String> {
    let _ = state.player_components.progress.info.add_key_int(
        "deadeye:hits",
        i64::try_from(req.EntityIds.len()).unwrap_or(i64::MAX),
    );
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(DeadeyeShootHitResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_target_gear_hit_request(
    state: &mut State,
    events: &mut EventQueue,
    req: TargetGearHitRequest,
) -> Result<TargetGearHitResponse, String> {
    let mut cfg_id = req.EntityId;
    let mut net_id_opt = None;
    if let Some(scene) = state.scene.as_ref() {
        if let Some(cid) = scene.find_config_id_by_net_id(req.EntityId) {
            cfg_id = i64::from(cid);
            net_id_opt = Some(req.EntityId);
        } else if let Ok(cid32) = i32::try_from(req.EntityId)
            && let Some(nid) = scene.find_entity_net_id_by_config_id(cid32)
        {
            net_id_opt = Some(nid);
        }
    }
    let _ = state
        .player_components
        .progress
        .info
        .add_key_int(&format!("gear_hit:{cfg_id}"), 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    if let (Some(scene), Some(net_id)) = (state.scene.as_mut(), net_id_opt) {
        if let Some(upd) = scene.update_entity_state_tag(
            &state.assets.tables.gameplay_tag_remap,
            net_id,
            "关卡.Common.状态.激活",
        ) {
            quest_actions::emit_entity_state_tag_update_checked(
                scene,
                Some(&state.assets),
                &mut state.conn,
                net_id,
                upd,
            );
        }
        scene.mark_fsm_dirty(net_id, wake_reason::TAG | wake_reason::STATE);
    }
    if cfg_id > 0 {
        state
            .player_components
            .quest
            .info
            .set_entity_state(cfg_id, "关卡.Common.状态.激活");
        let _ = state.player_components.quest.save(&state.fs);
        if let Ok(cid32) = i32::try_from(cfg_id) {
            let mut qctx = state.quest_context(events);
            let _ = quest_engine::on_entity_state_changed(
                &mut qctx,
                cid32,
                "关卡.Common.状态.激活",
            );
        }
    }
    Ok(TargetGearHitResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn current_role_weapon_kind(
    scene: &sonoro_gamesv_logic::scene::Scene,
    assets: &sonoro_gamesv_data::Assets,
) -> destructible::WeaponKind {
    let Ok(formation_idx) = usize::try_from(scene.formation_info.cur_formation) else {
        return destructible::WeaponKind::Unknown;
    };
    let Some(formation) = scene.formation_info.formations.get(formation_idx) else {
        return destructible::WeaponKind::Unknown;
    };
    let Some(role) = assets.tables.role_info.get_data_by_id(&formation.cur_role) else {
        return destructible::WeaponKind::Unknown;
    };
    destructible::weapon_kind_from_role_weapon_type(role.WeaponType)
}

pub fn handle_throw_damage_request(
    state: &mut State,
    events: &mut EventQueue,
    req: ThrowDamageRequest,
) -> Result<ThrowDamageResponse, String> {
    let eid = req.EntityId;
    let mut net_id = eid;
    let mut cfg_id = eid;
    let mut max_dur = 1i32;
    let mut hit_damage = 1i32;
    let mut found_in_scene = false;
    let mut destruction_reward = None;
    let mut reward_refreshes = false;

    if let Some(scene) = state.scene.as_ref() {
        if let Some(cid) = scene.find_config_id_by_net_id(eid) {
            found_in_scene = true;
            net_id = eid;
            cfg_id = i64::from(cid);
        } else if let Ok(cid32) = i32::try_from(eid)
            && let Some(nid) = scene.find_entity_net_id_by_config_id(cid32)
        {
            found_in_scene = true;
            net_id = nid;
            cfg_id = eid;
        }
        if cfg_id > 0
            && let Some(res) = level_play::resolve_entity_config(&state.assets, scene.instance_id, cfg_id)
            && let Some(di) = res.components.DestructibleItem
        {
            max_dur = destructible::initial_durability(di.Durability, 0);
            hit_damage = destructible::hit_damage(
                di.WeaponDamage.as_ref(),
                current_role_weapon_kind(scene, &state.assets),
            );
            destruction_reward =
                destructible::destruction_reward_id(res.components.RewardComponent.as_ref());
            reward_refreshes = res
                .components
                .RefreshComponent
                .as_ref()
                .is_some_and(|rc| !rc.Disabled.unwrap_or(false));
        }
    }

    let dur_key = format!("durability:{cfg_id}");
    let saved = state.player_components.progress.info.get_key_int(&dur_key);
    let cur_dur = if saved > 0 && saved <= i64::from(i32::MAX) {
        saved as i32
    } else {
        max_dur
    };
    let next_dur = destructible::after_hit(cur_dur, hit_damage);
    state
        .player_components
        .progress
        .info
        .set_key_int(&dur_key, i64::from(next_dur));
    if let Some(scene) = state.scene.as_mut()
        && let Some(storage) = scene.entity_mut(net_id)
    {
        storage.world_item.durability = next_dur;
    }
    let _ = state
        .player_components
        .progress
        .info
        .add_key_int("combat:throw_dmg", 1);

    let _ = state.conn.push_msg(
        ThrowDamageNotify::MSG_ID,
        &ThrowDamageNotify {
            EntityId: net_id,
            Durability: next_dur,
        },
    );

    if next_dur <= 0 {
        state
            .player_components
            .progress
            .info
            .mark_claim(&format!("explore:durability_empty:{cfg_id}"));
        state
            .player_components
            .progress
            .info
            .set_key_int("explore:durability_empty", 1);
        let state_upd = if found_in_scene
            && let Some(scene) = state.scene.as_mut()
            && scene.net_id_map.contains_key(&net_id)
        {
            if let Some(upd) = scene.update_entity_state_tag(
                &state.assets.tables.gameplay_tag_remap,
                net_id,
                "关卡.Common.状态.销毁",
            ) {
                quest_actions::emit_entity_state_tag_update_checked(
                    scene,
                    Some(&state.assets),
                    &mut state.conn,
                    net_id,
                    upd,
                );
            }
            let _ = scene.remove(&state.fs, net_id);
            let _ = state.conn.push_msg(
                EntityRemoveNotify::MSG_ID,
                &EntityRemoveNotify {
                    IsRemove: true,
                    RemoveInfos: vec![EntityRemoveInfo {
                        EntityId: net_id,
                        ..Default::default()
                    }],
                },
            );
            i32::try_from(cfg_id).ok()
        } else {
            None
        };
        if let Some(cid32) = state_upd
            && let Some(reward_id) = destruction_reward
        {
            grant_destruction_reward(state, cid32, reward_id, reward_refreshes)?;
        }
        if let Some(cid32) = state_upd {
            let mut qctx = state.quest_context(events);
            let _ = quest_engine::events::on_entity_state_changed(
                &mut qctx,
                cid32,
                "关卡.Common.状态.销毁",
            );
        }
    }

    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let mut qctx = state.quest_context(events);
    if let Err(err) = quest_engine::events::on_condition_state_changed(&mut qctx) {
        tracing::warn!(target: "combat_sync", "on_throw_damage failed: {err}");
    }
    Ok(ThrowDamageResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

/// Same window the Collect interaction gives a refreshing collectible
/// (`apply_interact_collect_or_reward`).
const DESTRUCTIBLE_RESPAWN_MS: i64 = 300_000;

/// Pays a destroyed destructible's `RewardComponent` once: a refreshing one goes on the
/// respawn cooldown, a one-off one is marked taken, as the Collect interaction does.
fn grant_destruction_reward(
    state: &mut State,
    cfg_id: i32,
    reward_id: i32,
    refreshes: bool,
) -> Result<(), String> {
    let now_ms = sonoro_gamesv_logic::scene::now_millis();
    let taken = state.player_components.quest.info.has_opened_chest(cfg_id)
        || state
            .scene
            .as_ref()
            .is_some_and(|s| s.instance.is_on_respawn_cooldown(cfg_id, now_ms));
    if taken {
        return Ok(());
    }
    // Only the items the client data lists are paid: crate packages (2202, 1315, ...) have
    // an empty DropPreview, and the chest override table is not client data.
    let listed = state
        .assets
        .tables
        .drop_package
        .get_data_by_id(&reward_id)
        .is_some_and(|pkg| !pkg.DropPreview.is_empty());
    if !listed {
        return Ok(());
    }
    {
        let mut rctx = state.reward_context();
        reward::grant_world_pickup_drop_package(&mut rctx, reward_id, 1)
            .map_err(|e| e.to_string())?;
    }
    if refreshes {
        if let Some(scene) = state.scene.as_mut() {
            scene
                .instance
                .set_respawn_cooldown(cfg_id, now_ms, now_ms + DESTRUCTIBLE_RESPAWN_MS);
            scene.save_persistent(&state.fs).map_err(|e| e.to_string())?;
        }
    } else {
        state.player_components.quest.info.mark_chest_opened(cfg_id);
        state
            .player_components
            .quest
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn handle_explore_skill_action_request(
    state: &mut State,
    req: ExploreSkillActionRequest,
) -> Result<ExploreSkillActionResponse, String> {
    let _ = state
        .player_components
        .progress
        .info
        .add_key_int(&format!("explore:skill_act:{}", req.EntityId), 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(ExploreSkillActionResponse {
        EntityId: req.EntityId,
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_buff_producer_request(
    state: &mut State,
    req: EntityBuffProducerRequest,
) -> Result<EntityBuffProducerResponse, String> {
    let _ = state
        .player_components
        .progress
        .info
        .add_key_int(&format!("buff_prod:req:{}", req.BuffConsumerId), 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(EntityBuffProducerResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_buff_producer_operate_request(
    state: &mut State,
    events: &mut EventQueue,
    req: EntityBuffProducerOperateRequest,
) -> Result<EntityBuffProducerOperateResponse, String> {
    let op_val = i64::from(req.OpType.map_or(0, |ot| ot.to_i32()));
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("buff_prod:op:{}", req.OpEntityId), op_val);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;

    if let Some(scene) = state.scene.as_ref() {
        let resolved_cfg = scene
            .find_config_id_by_net_id(req.OpEntityId)
            .map_or(req.OpEntityId, i64::from);
        if let Some(lcfg) = state
            .assets
            .find_level_entity(scene.instance_id, resolved_cfg)
        {
            let mut comps = lcfg.ComponentsData.clone();
            if let Some(tpl) = state.assets.find_template_config(&lcfg.BlueprintType) {
                tpl.ComponentsData.merge_into(&mut comps);
            }
            let mut buff_id = 0i64;
            for entry in comps.extra.iter().chain(comps.template_extra.iter()) {
                if let ExtraEntry::BuffProducerComponent(bp) = entry
                    && !bp.Disabled.unwrap_or(false)
                    && let Some(bid) = bp.BuffId
                    && bid > 0
                {
                    buff_id = bid;
                }
            }
            if buff_id > 0 {
                let use_player_target = state
                    .assets
                    .tables
                    .buff
                    .get_data_by_id(&buff_id)
                    .is_some_and(|bd| {
                        matches!(
                            bd.FormationPolicy,
                            FormationPolicy::FormationBuff | FormationPolicy::SharedByFormation
                        )
                    });
                let mut target_ent = if use_player_target {
                    scene
                        .entities
                        .iter()
                        .find(|e| e.config.entity_type == EntityType::Player)
                        .and_then(|e| scene.entity_handle(e.net_id))
                } else {
                    None
                };
                if target_ent.is_none() {
                    target_ent = scene.entity_handle(scene.current_role_entity_id());
                }
                if let Some(ent) = target_ent {
                    let op_type = req.OpType.unwrap_or(BuffOperateType::AddBuff);
                    if op_type == BuffOperateType::AddBuff {
                        events.enqueue(Event::BuffAddition {
                            target: ent,
                            instigator: ent,
                            buffs: vec![BuffAdditionEntry {
                                id: buff_id,
                                stack_count: 1,
                                is_active: true,
                                ..Default::default()
                            }],
                        });
                    } else {
                        events.enqueue(Event::BuffRemovalById {
                            entity: ent,
                            buff_id,
                        });
                    }
                }
            }
        }
    }

    Ok(EntityBuffProducerOperateResponse {
        ErrorCode: Some(ErrorCode::Success),
        OpEntityId: req.OpEntityId,
        OpType: req.OpType,
    })
}

pub fn handle_quick_hack_skill_request(
    state: &mut State,
    events: &mut EventQueue,
    req: QuickHackSkillRequest,
) -> Result<QuickHackSkillResponse, String> {
    let mut cfg_id = req.EntityId;
    let mut net_id_opt = None;
    if let Some(scene) = state.scene.as_ref() {
        if let Some(cid) = scene.find_config_id_by_net_id(req.EntityId) {
            cfg_id = i64::from(cid);
            net_id_opt = Some(req.EntityId);
        } else if let Ok(cid32) = i32::try_from(req.EntityId)
            && let Some(nid) = scene.find_entity_net_id_by_config_id(cid32)
        {
            net_id_opt = Some(nid);
        }
    }
    state
        .player_components
        .progress
        .info
        .set_key_int(&format!("quickhack:{cfg_id}"), i64::from(req.SkillId));
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    if let (Some(scene), Some(net_id)) = (state.scene.as_mut(), net_id_opt) {
        if let Some(upd) = scene.update_entity_state_tag(
            &state.assets.tables.gameplay_tag_remap,
            net_id,
            "关卡.Common.状态.激活",
        ) {
            quest_actions::emit_entity_state_tag_update_checked(
                scene,
                Some(&state.assets),
                &mut state.conn,
                net_id,
                upd,
            );
        }
        scene.mark_fsm_dirty(net_id, wake_reason::TAG | wake_reason::STATE);
    }
    if cfg_id > 0 {
        state
            .player_components
            .quest
            .info
            .set_entity_state(cfg_id, "关卡.Common.状态.激活");
        let _ = state.player_components.quest.save(&state.fs);
        if let Ok(cid32) = i32::try_from(cfg_id) {
            let mut qctx = state.quest_context(events);
            let _ = quest_engine::on_entity_state_changed(
                &mut qctx,
                cid32,
                "关卡.Common.状态.激活",
            );
        }
    }
    Ok(QuickHackSkillResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn find_nearest_detectable_entity(
    scene: &sonoro_gamesv_logic::scene::Scene,
) -> Option<DetectionEntityPosInfo> {
    let role_eid = scene.current_role_entity_id();
    let role_pos = (role_eid != 0)
        .then(|| scene.entity(role_eid).and_then(|e| e.position).map(|p| p.location))
        .flatten();
    let mut best_dist_sq = f64::INFINITY;
    let mut best = None;
    for storage in &scene.entities {
        if storage.config.entity_type == EntityType::Player || !storage.visible {
            continue;
        }
        let Some(pos) = storage.position else {
            continue;
        };
        let Some(rp) = role_pos else {
            return Some(DetectionEntityPosInfo {
                Pos: Some(Vector {
                    X: pos.location[0],
                    Y: pos.location[1],
                    Z: pos.location[2],
                }),
                ConfigId: storage.config.config_id,
            });
        };
        let dx = f64::from(pos.location[0]) - f64::from(rp[0]);
        let dy = f64::from(pos.location[1]) - f64::from(rp[1]);
        let dz = f64::from(pos.location[2]) - f64::from(rp[2]);
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq < best_dist_sq {
            best_dist_sq = dist_sq;
            best = Some(DetectionEntityPosInfo {
                Pos: Some(Vector {
                    X: pos.location[0],
                    Y: pos.location[1],
                    Z: pos.location[2],
                }),
                ConfigId: storage.config.config_id,
            });
        }
    }
    best
}

pub fn handle_check_use_sound_box_skill_request(
    state: &mut State,
    _req: CheckUseSoundBoxSkillRequest,
) -> Result<CheckUseSoundBoxSkillResponse, String> {
    let has_target = state
        .scene
        .as_ref()
        .is_none_or(|scene| find_nearest_detectable_entity(scene).is_some());
    Ok(CheckUseSoundBoxSkillResponse {
        Code: Some(if has_target {
            ErrorCode::Success
        } else {
            ErrorCode::ErrNoSoundBox
        }),
    })
}

pub fn handle_use_sound_box_skill_request(
    state: &mut State,
    _req: UseSoundBoxSkillRequest,
) -> Result<UseSoundBoxSkillResponse, String> {
    let detected = state
        .scene
        .as_ref()
        .and_then(find_nearest_detectable_entity);
    Ok(UseSoundBoxSkillResponse {
        Code: Some(ErrorCode::Success),
        DetectionEntityPosInfo: detected,
    })
}

pub fn handle_gm_entity_fsm_group_info_request(
    state: &mut State,
    req: GmEntityFsmGroupInfoRequest,
) -> Result<GmEntityFsmGroupInfoResponse, String> {
    let mut machines = Vec::new();
    if let Some(scene) = state.scene.as_ref()
        && let Some(fsm) = scene.entity(req.EntityId).and_then(|e| e.fsm.as_ref())
    {
        for (i, node) in fsm.runtime_nodes.iter().enumerate() {
            machines.push(GmEntityFsmMachineInfo {
                StateIndex: i as i32,
                GmEntityFsmStateInfos: vec![GmEntityFsmStateInfo {
                    StateIndex: node.leaf().unwrap_or(0),
                    GmEntityFsmTransitionInfos: Vec::new(),
                }],
            });
        }
    }
    Ok(GmEntityFsmGroupInfoResponse {
        EntityId: req.EntityId,
        GmEntityFsmMachineInfos: machines,
    })
}

pub fn handle_simple_combat_entity_die_request(
    state: &mut State,
    events: &mut EventQueue,
    req: SimpleCombatEntityDieRequest,
) -> Result<SimpleCombatEntityDieResponse, String> {
    let has_scene = state.scene.is_some();
    for (&eid, ctx) in &req.EntityDieCtxMap {
        let _ = state
            .player_components
            .progress
            .info
            .add_key_int("simple_combat:kills", 1);
        state
            .player_components
            .progress
            .info
            .set_key_int(&format!("simple_kill:{eid}"), 1);
        if ctx.TrapDefenseGoldenCoinAbosorbedCtx.is_some()
            || ctx.SurvivorsGoldenCoinAbosorbedCtx.is_some()
        {
            let _ = state
                .player_components
                .progress
                .info
                .add_key_int("simple_combat:coins", 1);
        }
        if has_scene {
            let cfg_opt = i32::try_from(eid).ok();
            let mut qctx = state.quest_context(events);
            let _ = quest_engine::events::on_entity_killed(&mut qctx, eid, cfg_opt);
            if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
                let _ = level_play::on_entity_killed_with_conditions(&mut lp_ctx, eid, cfg_opt);
            }
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(SimpleCombatEntityDieResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_simple_combat_entity_sync_attribute_push(
    state: &mut State,
    _events: &mut EventQueue,
    req: SimpleCombatEntitySyncAttributePush,
) -> Result<(), String> {
    if let Some(scene) = state.scene.as_mut() {
        for (&eid, pb_info) in &req.EntityAttributeMap {
            if let Some(attr) = scene
                .entity_mut(eid)
                .and_then(|e| e.attribute.as_mut())
            {
                for (&k, &v) in &pb_info.AttributeMap {
                    if let Ok(idx) = usize::try_from(k)
                        && let Some(slot) = attr.attributes.get_mut(idx)
                    {
                        slot.current = v;
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn handle_set_client_global_var_request(
    state: &mut State,
    req: SetClientGlobalVarRequest,
) -> Result<SetClientGlobalVarResponse, String> {
    if !req.VarName.is_empty() {
        let vname = String::from_utf8_lossy(&req.VarName);
        let key = format!("var:{vname}");
        if let Some(v) = req.VarDefinePb
            && let Some(val) = v.Value
        {
            match val {
                VarDefinePbValueOneof::Boolean(b) => state
                    .player_components
                    .progress
                    .info
                    .set_key_int(&key, i64::from(b)),
                VarDefinePbValueOneof::Int(n) => {
                    state.player_components.progress.info.set_key_int(&key, n)
                }
                VarDefinePbValueOneof::Float(f) => state
                    .player_components
                    .progress
                    .info
                    .set_key_int(&key, (f * 1000.0) as i64),
                VarDefinePbValueOneof::Entity(e) => state
                    .player_components
                    .progress
                    .info
                    .set_key_int(&key, i64::from(e)),
                VarDefinePbValueOneof::Quest(q) => state
                    .player_components
                    .progress
                    .info
                    .set_key_int(&key, i64::from(q)),
                VarDefinePbValueOneof::String(s) => state
                    .player_components
                    .progress
                    .info
                    .set_key_str(&key, &String::from_utf8_lossy(&s)),
                VarDefinePbValueOneof::QuestState(qs) => state
                    .player_components
                    .progress
                    .info
                    .set_key_int(&key, i64::from(qs.to_i32())),
                _ => state.player_components.progress.info.set_key_int(&key, 1),
            }
        }
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(SetClientGlobalVarResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_client_basic_info_request(
    state: &mut State,
    req: ClientBasicInfoRequest,
) -> Result<ClientBasicInfoResponse, String> {
    if let Some(info) = req.ClientBasicInfo {
        if !info.Platform.is_empty() {
            state.player_components.progress.info.set_key_str(
                "client:platform",
                &String::from_utf8_lossy(&info.Platform),
            );
        }
        if !info.OS.is_empty() {
            state
                .player_components
                .progress
                .info
                .set_key_str("client:os", &String::from_utf8_lossy(&info.OS));
        }
        if !info.Model.is_empty() {
            state
                .player_components
                .progress
                .info
                .set_key_str("client:model", &String::from_utf8_lossy(&info.Model));
        }
        state
            .player_components
            .progress
            .info
            .set_key_int("client:lang", i64::from(info.Language));
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(ClientBasicInfoResponse {})
}

pub fn handle_simple_track_report_request(
    state: &mut State,
    _req: SimpleTrackReportRequest,
) -> Result<SimpleTrackReportResponse, String> {
    let _ = state
        .player_components
        .progress
        .info
        .add_key_int("track:report_cnt", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;

    let last_inst = state.player_components.scene.last_scene_info.instance_id;
    let mut msgs = Vec::new();
    for &lp_id in &state.player_components.quest.info.completed_level_plays {
        let lp_opt = state.assets.tables.level_play_data.get_data_by_id(&lp_id);
        let inst_id = lp_opt
            .as_ref()
            .map_or(last_inst, |lp| if lp.Data.InstanceId != 0 { lp.Data.InstanceId } else { last_inst });
        let chest_cnt = lp_opt
            .as_ref()
            .and_then(|lp| lp.Data.LevelPlayRewardConfig.as_ref())
            .map_or(0, |rc| {
                if rc.RewardEntityId > 0
                    && state
                        .player_components
                        .quest
                        .info
                        .has_opened_chest(rc.RewardEntityId)
                {
                    1
                } else {
                    0
                }
            });
        msgs.push(SimpleTrackReportMsg {
            InstId: inst_id,
            LevelPlayId: lp_id,
            GainTreasureCount: chest_cnt,
        });
    }
    if msgs.is_empty() && !state.player_components.quest.info.opened_chests.is_empty() {
        msgs.push(SimpleTrackReportMsg {
            InstId: last_inst,
            LevelPlayId: 0,
            GainTreasureCount: state.player_components.quest.info.opened_chests.len() as i32,
        });
    }
    Ok(SimpleTrackReportResponse {
        ErrorCode: Some(ErrorCode::Success),
        SimpleTrackReportMsgs: msgs,
    })
}

pub fn handle_sec_fb_round_start_flow_request(
    _state: &mut State,
    req: SecFBRoundStartFlowRequest,
) -> Result<SecFBRoundStartFlowResponse, String> {
    Ok(SecFBRoundStartFlowResponse {
        ErrorCode: Some(ErrorCode::Success),
        LogId: req.LogId,
    })
}

macro_rules! sec_log_ok {
    ($fn_name:ident, $req:ty, $resp:ident, $ok_err:expr) => {
        pub fn $fn_name(_state: &mut State, req: $req) -> Result<$resp, String> {
            Ok($resp {
                Error: Some(DErrorResult {
                    ErrorCode: $ok_err,
                    ..Default::default()
                }),
                LogId: req.LogId,
            })
        }
    };
}

sec_log_ok!(
    handle_sec_fb_round_end_flow_report_request,
    SecFBRoundEndFlowReportRequest,
    SecFBRoundEndFlowReportResponse,
    Some(ErrorCode::Success)
);
sec_log_ok!(
    handle_sec_role_fight_flow_request,
    SecRoleFightFlowRequest,
    SecRoleFightFlowResponse,
    Some(ErrorCode::Success)
);
sec_log_ok!(
    handle_sec_world_start_flow_request,
    SecWorldStartFlowRequest,
    SecWorldStartFlowResponse,
    Some(ErrorCode::Success)
);
sec_log_ok!(
    handle_sec_world_info_flow_request,
    SecWorldInfoFlowRequest,
    SecWorldInfoFlowResponse,
    Some(ErrorCode::Success)
);
sec_log_ok!(
    handle_sec_world_s_end_flow_request,
    SecWorldSEndFlowRequest,
    SecWorldSEndFlowResponse,
    Some(ErrorCode::Success)
);
