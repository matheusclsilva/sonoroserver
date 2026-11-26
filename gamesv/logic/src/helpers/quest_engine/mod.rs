pub mod events;
pub mod tree;
pub mod triggers;

use std::collections::HashSet;
use std::io;

use serde_json::Value;
use sonoro_gamesv_data::asset_indexes::entity_world_pos;
use sonoro_gamesv_data::lazy_node_store::NodeEntry;
use sonoro_gamesv_fs::quest_info::{QuestInfo, append_unique_int, remove_int_value};
use sonoro_proto::enums::{BtType, ErrorCode, QuestFocusReason, QuestLockType, QuestState};
use sonoro_proto::pb::{
    BehaviorTreeCtxPb, ConditionGroupFinishInfo, DailyQuestReward, DestroyQuestsNotify,
    FlowEndNotify, FlowStartNotify, FocusQuestSetNotify, GameCtxPb, GameCtxPbCtxInfoOneof,
    OneQuestBranchPageInfo, QuestBranchInfo, QuestBranchUpdateNotify, QuestListNotify,
    QuestLockInfo, QuestLockInfoNotify, RemoveGuestNotify, TraceQuestNotify,
    WaitQuestConfirmResourceNotify,
};

use crate::helpers::condition;
use crate::helpers::level_play::LevelPlayContext;
use crate::helpers::prologue_quest;
use crate::helpers::quest_actions;
use crate::helpers::quest_types::{ActionCallbacks, QuestContext, json_bool, json_int, json_str};
use crate::helpers::reward;
use crate::helpers::role::{self, RoleRepairSummary};
use crate::helpers::save_block;
use crate::sink::PushSink;

pub use events::*;
pub use tree::{
    StepResult, check_node_occupation_and_save_pub, eval_quest_failed_condition_pub, now_ms_pub,
    parse_timer_delay_ms_pub, step_pending_control_nodes,
};
pub use triggers::{
    eval_child_condition_auto, eval_inline_condition, get_child_condition_pub,
    has_async_play_flow_action, is_client_action_name_registered, is_safe_bt_leisure_interact,
    matches_reach_area_pub, node_action_items_pub, push_bt_child_node_status,
    push_bt_composition_enter_events, push_bt_level_events_for_type,
    push_do_interact_options_for_bt_node, push_do_interact_options_for_bt_node_mode,
    register_dynamic_interact_options_for_entity,
    remove_do_interact_options_for_bt_node, sync_dynamic_interact_options_for_entity,
};

pub const PROLOGUE_QUEST_ID: i32 = prologue_quest::PROLOGUE_QUEST_ID;

pub fn action_callbacks<S: PushSink + ?Sized>() -> ActionCallbacks<'static, S> {
    ActionCallbacks {
        bt_type: BtType::BtTypeQuest,
        is_flow_action: false,
        defer_flow_transition: false,
        deferred_bt_vars: None,
        start_flow: Some(triggers::start_flow_with_mode),
        start_flow_state: Some(|ctx, qid, nid, flow_name, flow_id, state_id, enqueue| {
            triggers::start_flow_params(ctx, qid, nid, flow_name, flow_id, state_id, enqueue)
        }),
        complete_guide: Some(triggers::complete_guide_by_id),
        on_entity_state_changed: Some(events::on_entity_state_changed),
        on_phone_message: Some(events::on_phone_message_event),
        on_npc_mail: Some(events::on_npc_mail_sent),
        on_time_changed: Some(events::on_time_changed),
        on_weather_changed: Some(events::on_weather_changed),
        on_item_gained: Some(events::on_item_gained),
        on_level_play_enabled: Some(|ctx, lp_id, enabled| {
            if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
                crate::helpers::level_play::on_level_play_enabled(&mut lp_ctx, lp_id, enabled)?;
            }
            if enabled && tree::recheck_pending_conditions(ctx)? {
                tree::finalize_progression(ctx)?;
            }
            Ok(())
        }),
        on_accept_focus_wait: Some(accept_focus_wait_quest_action),
        on_finish_flow_state: Some(finish_flow_state_from_action),
        on_destroy_quest: Some(destroy_quest),
        on_finish_condition: Some(|ctx, owner_id, node_id| {
            if owner_id > 0 && node_id > 0 {
                let _ = events::on_submit_node(ctx, owner_id, i64::from(owner_id), node_id)?;
            }
            Ok(())
        }),
        on_trace_current_quest: Some(|ctx, qid| on_scene_trace(ctx, i64::from(qid))),
        on_var_changed: Some(events::on_var_changed),
    }
}

fn discard_save_block_progress_on_login<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    ctx.quest_comp.login_restore.reenter_quests.clear();
    ctx.quest_comp.login_restore.pos_rollback = None;

    let quest_ids = triggers::snapshot_active_quest_ids(ctx);
    for quest_id in quest_ids {
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
            continue;
        };
        let Some((active_nodes, finished_nodes)) = ctx
            .quest_comp
            .info
            .find_active_quest(quest_id)
            .map(|aq| (aq.active_nodes.clone(), aq.finished_nodes.clone()))
        else {
            continue;
        };
        let discard = save_block::plan_login_discard(&nodes, &active_nodes, &finished_nodes);
        if discard.is_empty() {
            continue;
        }

        if let Some(aq) = ctx.quest_comp.info.find_active_quest_mut(quest_id) {
            for &node_id in &discard.reenter {
                aq.mark_node_inactive(node_id);
            }
            for &node_id in &discard.unfinish {
                remove_int_value(&mut aq.finished_nodes, node_id);
            }
        }
        for &node_id in &discard.unfinish {
            ctx.quest_comp
                .info
                .forget_child_quest_finished(quest_id, node_id);
        }
        forget_save_block_node_progress(ctx, quest_id, &nodes, &discard.blocks)?;

        ctx.quest_comp.login_restore.reenter_quests.push(quest_id);
        if ctx.quest_comp.login_restore.pos_rollback.is_none() {
            for &block_id in &discard.blocks {
                if let Some(rollback) = save_block::pos_rollback_of(&nodes, block_id) {
                    ctx.quest_comp.login_restore.pos_rollback = Some(rollback);
                    break;
                }
            }
        }
        tracing::info!(
            target: "quest_engine",
            "login restore: quest {quest_id} resumes at save block(s) {:?}; re-entering {} node(s), forgetting {} finished",
            discard.blocks,
            discard.reenter.len(),
            discard.unfinish.len()
        );
    }
    Ok(())
}

fn forget_save_block_node_progress<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    nodes: &[NodeEntry],
    blocks: &[i32],
) -> io::Result<()> {
    let mut target_ids = Vec::new();
    for node in nodes {
        let in_block = blocks
            .iter()
            .any(|&block_id| save_block::is_in_block(nodes, node.id, block_id));
        if !in_block {
            continue;
        }
        for target_id in extract_node_target_entity_ids(node) {
            target_ids.push((node.id, target_id));
        }
    }

    let mut claims_changed = false;
    for (node_id, target_id) in target_ids {
        if let Some(aq) = ctx.quest_comp.info.find_active_quest_mut(quest_id) {
            remove_int_value(&mut aq.killed_entities, target_id);
        }
        if let Some(progress) = ctx.progress_comp.as_deref_mut() {
            let key = format!("interact_done:{quest_id}:{node_id}:{target_id}");
            if progress.info.remove_claim(&key) {
                claims_changed = true;
            }
        }
    }
    if claims_changed && let Some(progress) = ctx.progress_comp.as_deref_mut() {
        progress.save(ctx.fs)?;
    }
    Ok(())
}

pub(crate) fn is_login_reentry_pending<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    quest_id: i32,
) -> bool {
    ctx.quest_comp
        .login_restore
        .reenter_quests
        .contains(&quest_id)
}

fn reenter_save_blocks_after_login<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    ctx.quest_comp.login_restore.pos_rollback = None;
    if ctx.quest_comp.login_restore.reenter_quests.is_empty() {
        return Ok(());
    }
    let pending = std::mem::take(&mut ctx.quest_comp.login_restore.reenter_quests);
    for quest_id in pending {
        if !ctx.quest_comp.info.has_active_quest(quest_id) {
            continue;
        }
        tracing::info!(
            target: "quest_engine",
            "login restore: entering save block of quest {quest_id} again"
        );
        tree::advance_quest_tree(ctx, quest_id, true)?;
    }
    tree::finalize_progression(ctx)?;
    Ok(())
}

pub fn sync_on_login<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) -> io::Result<()> {
    discard_save_block_progress_on_login(ctx)?;
    ctx.quest_comp.info.unlock_system(10035);
    if !ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID)
        && !ctx.quest_comp.info.has_active_quest(PROLOGUE_QUEST_ID)
    {
        tree::activate_quest(ctx, PROLOGUE_QUEST_ID, false)?;
    }

    let _ = tree::refresh_daily_quests(ctx, false)?;
    let _ = tree::refresh_available_quests(ctx, false)?;
    let _ = tree::recheck_pending_conditions(ctx)?;
    let _ = tree::step_pending_control_nodes(ctx)?;
    let _ = reconcile_account_state(ctx, false)?;
    let _ = cleanup_orphaned_quest_environment(ctx, false)?;

    triggers::ensure_tracked_focus_valid(ctx);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.current_quest_id = ctx.quest_comp.info.tracked_quest_id;
    }
    triggers::sync_scene_current_node(ctx);

    triggers::push_quest_state_notifies_with_enter(ctx, false, true)?;
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        crate::helpers::level_play::sync_on_login(&mut lp_ctx)?;
    }
    tree::ensure_active_quest_timers_scheduled(ctx)?;
    let _ = quest_actions::sync_world_state_on_join(ctx);
    ensure_active_quest_targets_spawned(ctx);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.login_world_synced = true;
    }
    ctx.quest_comp.save(ctx.fs)?;
    Ok(())
}

pub fn resync_on_reconnect<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) -> io::Result<()> {
    triggers::ensure_tracked_focus_valid(ctx);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.current_quest_id = ctx.quest_comp.info.tracked_quest_id;
    }
    triggers::sync_scene_current_node(ctx);

    let client_loaded = ctx
        .scene
        .as_deref()
        .is_some_and(|s| s.scene_loading_finished);
    if client_loaded {
        ensure_active_quest_targets_spawned(ctx);
    }
    let aq_snap: Vec<(i32, Vec<i32>)> = ctx
        .quest_comp
        .info
        .active_quests
        .iter()
        .map(|aq| (aq.quest_id, aq.active_nodes.clone()))
        .collect();
    if let Some(scene) = ctx.scene.as_deref_mut() {
        for (quest_id, active_nodes) in aq_snap {
            let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
                continue;
            };
            for nid in active_nodes {
                let Some(node) = triggers::find_node_by_id(&nodes, nid) else {
                    continue;
                };
                triggers::push_do_interact_options_for_node(
                    scene,
                    ctx.sink,
                    quest_id,
                    node,
                    None,
                )?;
            }
        }
    }
    if client_loaded && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        crate::helpers::level_play::sync_on_login(&mut lp_ctx)?;
    }
    ctx.sink.push(
        TraceQuestNotify::MSG_ID,
        &TraceQuestNotify {
            QuestId: ctx.quest_comp.info.tracked_quest_id,
        },
    );
    if ctx.quest_comp.info.focus_quest_id > 0 {
        ctx.sink.push(
            FocusQuestSetNotify::MSG_ID,
            &FocusQuestSetNotify {
                FocusQuestId: ctx.quest_comp.info.focus_quest_id,
                Reason: Some(QuestFocusReason::Inherit),
            },
        );
    }
    Ok(())
}

pub fn on_scene_join_ready<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) -> io::Result<()> {
    triggers::push_ready_quest_list(ctx);
    ensure_active_quest_targets_spawned(ctx);
    let prologue_active = ctx
        .quest_comp
        .info
        .find_active_quest(PROLOGUE_QUEST_ID)
        .is_some_and(|aq| aq.is_node_active(19));
    if let Some(scene) = ctx.scene.as_deref_mut() {
        if prologue_active && !scene.prologue_flow_started {
            scene.set_deferred_join_flow(1, prologue_quest::PROLOGUE_FLOW_NAME, 1, 1);
        } else {
            scene.clear_deferred_join_flow();
        }
    }
    Ok(())
}

fn sync_morale_on_scene_load<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let active_play = ctx
        .progress_comp
        .as_deref()
        .map_or(0, |pc| pc.info.get_key_int("morale:active_play"));
    let play_1_on = ctx
        .progress_comp
        .as_deref()
        .map_or(0, |pc| pc.info.get_key_int("morale:play:1"));
    let resolved_id = if active_play > 0 && active_play <= i64::from(i32::MAX) {
        active_play as i32
    } else {
        1
    };
    let cfg = crate::helpers::morale::resolve_play_config(Some(ctx.assets), resolved_id);
    let keep_exp = ctx.inventory_comp.info.normal_item_count(cfg.keep_item_id);
    let temp_exp = ctx.inventory_comp.info.normal_item_count(cfg.temp_item_id);
    if active_play <= 0 && play_1_on <= 0 && keep_exp <= 0 && temp_exp <= 0 {
        return Ok(());
    }
    let _ = crate::helpers::morale::sync_from_inventory(
        Some(ctx.sink),
        ctx.fs,
        ctx.assets,
        ctx.basic_comp,
        ctx.inventory_comp,
        ctx.progress_comp.as_deref_mut(),
        Some(cfg.play_id),
        None,
        true,
    )?;
    Ok(())
}

pub fn on_scene_trace<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    trace_id: i64,
) -> io::Result<()> {
    if trace_id > 0
        && let Ok(id32) = i32::try_from(trace_id)
    {
        if ctx.quest_comp.info.has_active_quest(id32) {
            ctx.quest_comp.info.tracked_quest_id = id32;
            ctx.quest_comp.save(ctx.fs)?;
        } else {
            quest_actions::ensure_quest_entity_spawned(ctx, id32);
        }
    }
    resync_on_reconnect(ctx)
}

pub fn on_scene_loading_finish<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    if ctx.scene.is_none() {
        return Ok(());
    }
    reenter_save_blocks_after_login(ctx)?;
    ensure_active_quest_targets_spawned(ctx);
    triggers::sync_active_dynamic_interact_options(ctx)?;
    tree::ensure_active_quest_timers_scheduled(ctx)?;
    triggers::ensure_tracked_focus_valid(ctx);
    triggers::sync_scene_current_node(ctx);
    ctx.sink.push(
        TraceQuestNotify::MSG_ID,
        &TraceQuestNotify {
            QuestId: ctx.quest_comp.info.tracked_quest_id,
        },
    );
    if ctx.quest_comp.info.focus_quest_id > 0 {
        ctx.sink.push(
            FocusQuestSetNotify::MSG_ID,
            &FocusQuestSetNotify {
                FocusQuestId: ctx.quest_comp.info.focus_quest_id,
                Reason: Some(QuestFocusReason::Inherit),
            },
        );
    }
    sync_morale_on_scene_load(ctx)?;

    let prologue_active = ctx
        .quest_comp
        .info
        .find_active_quest(PROLOGUE_QUEST_ID)
        .is_some_and(|aq| aq.is_node_active(19));
    let run_prologue_flow = ctx.scene.as_deref_mut().is_some_and(|scene| {
        let should = scene.deferred_join_flow.is_some()
            || (prologue_active && !scene.prologue_flow_started);
        scene.clear_deferred_join_flow();
        should
    });
    if run_prologue_flow
        && let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(PROLOGUE_QUEST_ID)
        && let Some(node) = triggers::find_node_by_id(&nodes, 19).cloned()
        && let Some(cond) = triggers::get_child_condition(&node.data)
        && let Some(Value::Object(flow_val)) = cond.get("Flow")
    {
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.prologue_flow_started = true;
        }
        triggers::start_flow_from_obj(ctx, PROLOGUE_QUEST_ID, 19, flow_val)?;
    }

    if ctx
        .scene
        .as_deref()
        .is_some_and(|scene| scene.active_flow_inc_id != 0)
    {
        return Ok(());
    }

    let tracked = ctx.quest_comp.info.tracked_quest_id;
    if events::try_start_pending_play_flow_for_quest(ctx, tracked)? {
        return Ok(());
    }
    let active_snapshot = triggers::snapshot_active_quest_ids(ctx);
    for qid in active_snapshot {
        if qid == tracked {
            continue;
        }
        if events::try_start_pending_play_flow_for_quest(ctx, qid)? {
            return Ok(());
        }
    }
    Ok(())
}

pub fn on_flow_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    flow_inc_id: i64,
    action_id: i32,
) -> io::Result<()> {
    if action_id <= 0 {
        return Ok(());
    }
    let Some((flow_list_name, flow_id, state_id, quest_id, node_id, bt_type, is_active, prev_state_id, saved_owner)) =
        ctx.scene.as_deref_mut().and_then(|scene| {
            if scene.active_flow_inc_id == 0
                || (flow_inc_id != 0 && scene.active_flow_inc_id != flow_inc_id)
            {
                let af = scene.find_async_flow(flow_inc_id)?;
                Some((
                    af.flow_list_name.clone(),
                    af.flow_id,
                    af.state_id,
                    af.quest_id,
                    af.node_id,
                    af.bt_type,
                    false,
                    scene.active_flow_state_id,
                    scene.active_flow_owner_entity_id,
                ))
            } else if scene.active_flow_list_name.is_empty() {
                None
            } else {
                Some((
                    scene.active_flow_list_name.clone(),
                    scene.active_flow_id,
                    scene.active_flow_state_id,
                    scene.active_flow_quest_id,
                    scene.active_flow_node_id,
                    scene.active_flow_bt_type,
                    true,
                    scene.active_flow_state_id,
                    scene.active_flow_owner_entity_id,
                ))
            }
        })
    else {
        return Ok(());
    };

    let Ok(Some(state_val)) = ctx
        .assets
        .flow_states
        .get_flow_state(&flow_list_name, flow_id, state_id)
    else {
        return Ok(());
    };
    let Some(action_val) =
        sonoro_gamesv_data::lazy_flow_store::find_action_by_id(&state_val, action_id).cloned()
    else {
        return Ok(());
    };
    if is_active && let Some(scene) = ctx.scene.as_deref_mut() {
        scene.mark_flow_action_executed(action_id);
    }
    let mut cbs = action_callbacks();
    cbs.bt_type = bt_type;
    cbs.is_flow_action = true;
    quest_actions::execute_action_list(ctx, &mut cbs, quest_id, node_id, &[action_val])?;
    if is_active
        && saved_owner != 0
        && let Some(scene) = ctx.scene.as_deref_mut()
        && scene.active_flow_state_id != prev_state_id
        && scene.active_flow_owner_entity_id == 0
    {
        scene.active_flow_owner_entity_id = saved_owner;
    }
    Ok(())
}

pub fn interrupt_active_flow_for_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> Option<i64> {
    let ended_inc = {
        let scene = ctx.scene.as_deref_mut()?;
        if scene.active_flow_inc_id == 0 || scene.active_flow_quest_id != quest_id {
            return None;
        }
        let ended_inc = scene.active_flow_inc_id;
        scene.clear_active_flow();
        scene.prologue_flow_started = false;
        ended_inc
    };
    triggers::restore_plot_main_role(ctx);
    ctx.sink.push(
        FlowEndNotify::MSG_ID,
        &FlowEndNotify {
            FlowIncId: ended_inc,
            Async: false,
            IsFinished: false,
        },
    );
    Some(ended_inc)
}

pub fn collect_active_quest_target_ids<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
) -> HashSet<i32> {
    let mut targets = HashSet::new();
    for aq in &ctx.quest_comp.info.active_quests {
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            let Some(node) = triggers::find_node_by_id(&nodes, nid) else {
                continue;
            };
            for eid in extract_node_target_entity_ids(node) {
                targets.insert(eid);
            }
        }
    }
    targets
}

pub fn should_hide_world_entity(
    info: &QuestInfo,
    active_targets: &HashSet<i32>,
    config_id: i32,
) -> bool {
    if config_id <= 0 {
        return false;
    }
    if info.world.shown_entities.contains(&config_id) {
        return false;
    }
    if info.world.hidden_entities.contains(&config_id) {
        return true;
    }
    if active_targets.contains(&config_id) {
        return false;
    }
    info.world
        .hidden_groups
        .iter()
        .any(|grp| grp.entity_ids.contains(&config_id))
}

pub fn ensure_active_quest_targets_spawned<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    let targets: Vec<i32> = collect_active_quest_target_ids(ctx).into_iter().collect();
    for cfg_id in targets {
        quest_actions::ensure_quest_entity_spawned(ctx, cfg_id);
    }
}

pub fn spawn_node_target_entities<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
) {
    let ids = extract_node_target_entity_ids(node);
    for cfg_id in ids {
        if let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id)
            && aq.has_killed_entity(cfg_id)
        {
            continue;
        }
        if ctx.quest_comp.info.world.hidden_entities.contains(&cfg_id) {
            continue;
        }
        quest_actions::ensure_quest_entity_spawned(ctx, cfg_id);
    }
}

fn push_unique_target(out: &mut Vec<i32>, eid: i32) {
    if eid > 0 && !out.contains(&eid) {
        out.push(eid);
    }
}

pub fn extract_node_target_entity_ids(node: &NodeEntry) -> Vec<i32> {
    let mut out = Vec::new();
    if let Some(cond) = triggers::get_child_condition(&node.data) {
        for field in [
            "EntityId",
            "RangeEntityId",
            "RangeEntity",
            "TalkEntityId",
            "TargetEntityId",
        ] {
            if let Some(eid) = json_int(cond.get(field)) {
                push_unique_target(&mut out, eid);
            }
        }

        for arr_field in [
            "EntityIds",
            "TargetsToAwake",
            "ExistTargets",
            "MonsterCreatorEntityIds",
            "RangeEntities",
        ] {
            if let Some(Value::Array(arr)) = cond.get(arr_field) {
                for v in arr {
                    if let Some(eid) = json_int(Some(v)) {
                        push_unique_target(&mut out, eid);
                    }
                }
            }
        }

        for obj_array in ["AddOptions", "PhotoTargets", "Conditions"] {
            if let Some(Value::Array(arr)) = cond.get(obj_array) {
                for item in arr {
                    if let Some(eid) = item.get("EntityId").and_then(|v| json_int(Some(v))) {
                        push_unique_target(&mut out, eid);
                    }
                }
            }
        }

        if let Some(Value::Object(pos_cond)) = cond.get("PosCondition")
            && let Some(eid) = json_int(pos_cond.get("RangeEntity"))
        {
            push_unique_target(&mut out, eid);
        }

        if let Some(Value::Object(ao)) = cond.get("AddOption")
            && let Some(eid) = json_int(ao.get("EntityId"))
        {
            push_unique_target(&mut out, eid);
        }
        if let Some(Value::Object(hi)) = cond.get("HandInItems")
            && let Some(Value::Object(ao)) = hi.get("AddOption")
            && let Some(eid) = json_int(ao.get("EntityId"))
        {
            push_unique_target(&mut out, eid);
        }
        if let Some(Value::Object(et)) = cond.get("EventType")
            && let Some(eid) = json_int(et.get("EntityId"))
        {
            push_unique_target(&mut out, eid);
        }
        if let Some(Value::Array(opts)) = cond.get("Option") {
            for o in opts {
                if let Some(eid) = o.get("EntityId").and_then(|v| json_int(Some(v))) {
                    push_unique_target(&mut out, eid);
                }
                if let Some(Value::Array(eids)) = o.get("EntityIds") {
                    for v in eids {
                        if let Some(eid) = json_int(Some(v)) {
                            push_unique_target(&mut out, eid);
                        }
                    }
                }
            }
        }
    }

    if let Some(track_type) = node
        .data
        .get("TrackTarget")
        .and_then(|tt| tt.get("TrackType"))
        .and_then(Value::as_object)
    {
        if let Some(Value::Array(eids)) = track_type.get("EntityIds") {
            for v in eids {
                if let Some(eid) = json_int(Some(v)) {
                    push_unique_target(&mut out, eid);
                }
            }
        }
        if let Some(Value::Array(ents)) = track_type.get("Entities") {
            for item in ents {
                let eid = match item {
                    Value::Object(obj) => json_int(obj.get("EntityId"))
                        .or_else(|| json_int(obj.get("Id")))
                        .unwrap_or(0),
                    _ => json_int(Some(item)).unwrap_or(0),
                };
                push_unique_target(&mut out, eid);
            }
        }
    }

    out
}

pub fn build_quest_branch_info<S: PushSink + ?Sized>(ctx: &QuestContext<'_, S>) -> QuestBranchInfo {
    let pages = ctx
        .quest_comp
        .info
        .progress
        .quest_branches
        .iter()
        .map(|rec| OneQuestBranchPageInfo {
            id: rec.page_id,
            CurBranch: rec.cur_branch,
            CompleteBranches: rec.complete_branches.clone(),
        })
        .collect();
    QuestBranchInfo {
        QuestBranchInfos: pages,
        UnlockBranchComponentsGroup: Vec::new(),
        UnlockTimePoints: Vec::new(),
    }
}

pub fn select_quest_branch<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    page_id: i32,
    branch_id: i32,
) -> io::Result<OneQuestBranchPageInfo> {
    let page_pb = {
        let rec = ctx
            .quest_comp
            .info
            .select_quest_branch(page_id, branch_id);
        OneQuestBranchPageInfo {
            id: rec.page_id,
            CurBranch: rec.cur_branch,
            CompleteBranches: rec.complete_branches.clone(),
        }
    };
    ctx.quest_comp.save(ctx.fs)?;
    let branch_info = build_quest_branch_info(ctx);
    ctx.sink.push(
        QuestBranchUpdateNotify::MSG_ID,
        &QuestBranchUpdateNotify {
            QuestBranchInfo: Some(branch_info),
            ErrorCode: Some(ErrorCode::Success),
        },
    );
    Ok(page_pb)
}

pub fn set_focus_deter_cond<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    deter: bool,
) -> io::Result<()> {
    ctx.quest_comp.info.progress.focus_deter_cond = deter;
    ctx.quest_comp.save(ctx.fs)
}

pub fn accept_quest_by_id<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> io::Result<()> {
    if quest_id <= 0
        || ctx.quest_comp.info.has_active_quest(quest_id)
        || ctx.quest_comp.info.has_finished_quest(quest_id)
    {
        return Ok(());
    }
    if let Some(q) = ctx.assets.tables.quest_data.get_data_by_id(&quest_id)
        && tree::is_eligible_quest(ctx, &q.Data, false)
    {
        let _ = tree::activate_quest(ctx, quest_id, true)?;
        tree::finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn accept_focus_wait_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> io::Result<()> {
    if quest_id <= 0 {
        return Ok(());
    }
    if let Some(q) = ctx.assets.tables.quest_data.get_data_by_id(&quest_id)
        && tree::is_eligible_quest(ctx, &q.Data, false)
    {
        let _ = tree::activate_quest(ctx, quest_id, true)?;
    }
    if ctx.quest_comp.info.has_active_quest(quest_id) {
        ctx.quest_comp.info.focus_quest_id = quest_id;
        ctx.quest_comp.info.tracked_quest_id = quest_id;
        ctx.sink.push(
            FocusQuestSetNotify::MSG_ID,
            &FocusQuestSetNotify {
                FocusQuestId: quest_id,
                Reason: Some(QuestFocusReason::Request),
            },
        );
        tree::finalize_progression(ctx)?;
    }
    Ok(())
}

fn accept_focus_wait_quest_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let focus_id = ctx.quest_comp.info.focus_quest_id;
    if focus_id > 0 {
        accept_focus_wait_quest(ctx, focus_id)?;
    }
    Ok(())
}

fn finish_flow_state_from_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    if tree::recheck_pending_conditions(ctx)? {
        tree::finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn destroy_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> io::Result<()> {
    if quest_id <= 0 {
        return Ok(());
    }
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.quest_timers.cancel_quest(quest_id);
        scene.fail_timers.cancel_quest(quest_id);
        scene.release_occupations_for_tree(quest_id, i64::from(quest_id));
        scene.clear_save_nodes_for_owner(quest_id);
        scene.clear_action_node_progress_for_owner(quest_id);
        interrupt_active_flow_for_quest(ctx, quest_id);
    }
    if !ctx.quest_comp.info.has_active_quest(quest_id) {
        return Ok(());
    }
    ctx.quest_comp.info.remove_active_quest(quest_id);

    ctx.sink.push(
        DestroyQuestsNotify::MSG_ID,
        &DestroyQuestsNotify {
            QuestIds: vec![quest_id],
        },
    );
    ctx.sink.push(
        QuestListNotify::MSG_ID,
        &QuestListNotify {
            Quests: vec![sonoro_proto::pb::QuestInfo {
                QuestId: quest_id,
                Status: Some(QuestState::Delete),
            }],
        },
    );

    tree::finalize_progression(ctx)
}

fn resolve_gm_target_quest_id<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    quest_id_opt: Option<i32>,
) -> i32 {
    if let Some(qid) = quest_id_opt
        && qid > 0
        && ctx.quest_comp.info.has_active_quest(qid)
    {
        return qid;
    }
    let tracked = ctx.quest_comp.info.tracked_quest_id;
    if tracked > 0 && ctx.quest_comp.info.has_active_quest(tracked) {
        return tracked;
    }
    ctx.quest_comp
        .info
        .active_quests
        .first()
        .map_or(0, |aq| aq.quest_id)
}

pub fn gm_pick_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    force_replay: bool,
) -> io::Result<bool> {
    if quest_id <= 0 {
        return Ok(false);
    }
    let exists = ctx.assets.tables.quest_data.get_data_by_id(&quest_id).is_some()
        || ctx.assets.quest_nodes.has_owner(quest_id);
    if !exists {
        return Ok(false);
    }

    if force_replay {
        remove_int_value(&mut ctx.quest_comp.info.finished_quests, quest_id);
        if ctx.quest_comp.info.has_active_quest(quest_id) {
            destroy_quest(ctx, quest_id)?;
        }
    }

    if !ctx.quest_comp.info.has_active_quest(quest_id)
        && !ctx.quest_comp.info.has_finished_quest(quest_id)
    {
        let _ = tree::activate_quest(ctx, quest_id, true)?;
    }
    if ctx.quest_comp.info.has_active_quest(quest_id) {
        ctx.quest_comp.info.tracked_quest_id = quest_id;
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.current_quest_id = quest_id;
        }
    }

    triggers::push_quest_state_notifies(ctx)?;
    ensure_active_quest_targets_spawned(ctx);
    tree::finalize_progression(ctx)?;
    Ok(true)
}

pub fn gm_skip_quest_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id_opt: Option<i32>,
) -> io::Result<i32> {
    let quest_id = resolve_gm_target_quest_id(ctx, quest_id_opt);
    if quest_id <= 0 {
        return Ok(0);
    }

    let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id).cloned() else {
        return Ok(0);
    };
    let nodes = ctx
        .assets
        .quest_nodes
        .get_owner_nodes(quest_id)
        .unwrap_or_default();
    if nodes.is_empty() || aq.active_nodes.is_empty() {
        tree::succeed_quest(ctx, quest_id)?;
        triggers::push_quest_state_notifies(ctx)?;
        ensure_active_quest_targets_spawned(ctx);
        tree::finalize_progression(ctx)?;
        return Ok(quest_id);
    }

    for nid in aq.active_nodes {
        let Some(node) = triggers::find_node_by_id(&nodes, nid).cloned() else {
            continue;
        };
        if node.node_type == "ChildQuest" {
            tree::complete_child_quest_without_advance(ctx, quest_id, &node, true)?;
        } else {
            tree::finish_non_child_node(ctx, quest_id, &node, true)?;
        }
    }

    if ctx.quest_comp.info.has_active_quest(quest_id) {
        tree::advance_quest_tree(ctx, quest_id, true)?;
    }

    triggers::push_quest_state_notifies(ctx)?;
    ensure_active_quest_targets_spawned(ctx);
    tree::finalize_progression(ctx)?;
    Ok(quest_id)
}

#[derive(Debug, Clone, Default)]
pub struct AccountRepairReport {
    pub repaired_quests: usize,
    pub unlocked_systems: usize,
    pub removed_trial_roles: usize,
    pub unlocked_locks: usize,
    pub left_dungeon: bool,
    pub role_summary: RoleRepairSummary,
}

fn collect_quest_actions_recursive(out: &mut Vec<Value>, val: &Value, depth: usize) {
    if depth > 16 {
        return;
    }
    match val {
        Value::Array(arr) => {
            for item in arr {
                if item.is_object() && item.get("Name").is_some() {
                    out.push(item.clone());
                } else {
                    collect_quest_actions_recursive(out, item, depth + 1);
                }
            }
        }
        Value::Object(obj) => {
            if obj.get("Name").is_some() && obj.get("Params").is_some() {
                out.push(val.clone());
                return;
            }
            for (k, v) in obj {
                if k == "FailedCondition" {
                    continue;
                }
                collect_quest_actions_recursive(out, v, depth + 1);
            }
        }
        _ => {}
    }
}

fn is_skip_safe_action_name(
    name: &str,
    params_opt: Option<&serde_json::Map<String, Value>>,
    reconcile_only: bool,
    allow_world_layers: bool,
) -> bool {
    if matches!(
        name,
        "OpenSystemFunction"
            | "UnlockSystemItem"
            | "UnlockTeleportTrigger"
            | "CompleteGuide"
            | "GuideTrigger"
            | "RunActions"
    ) {
        return true;
    }
    if name == "EnableFunction" {
        if reconcile_only && let Some(p) = params_opt {
            if json_bool(p.get("Enable")) == Some(false) {
                return false;
            }
            if matches!(
                json_str(p.get("Type")),
                Some("InteractiveItems" | "MotorRadioUI" | "TeleportDungeon")
            ) {
                return false;
            }
        }
        return true;
    }
    if name == "EnableSystem" {
        if let Some(p) = params_opt
            && reconcile_only
            && !json_bool(p.get("IsEnable")).unwrap_or(true)
        {
            return false;
        }
        return true;
    }
    if name == "SwitchDataLayers" || name == "SwitchSubLevels" {
        return allow_world_layers;
    }
    if reconcile_only {
        return false;
    }
    if name == "SetTimeLockState" || name == "SetWeatherLockState" {
        if let Some(p) = params_opt {
            let lock_str = json_str(p.get("LockState")).unwrap_or("Unlock");
            return lock_str != "Lock";
        }
        return true;
    }
    matches!(
        name,
        "SetVar"
            | "CalculateVar"
            | "RandomVar"
            | "SetAreaState"
            | "EnableLevelPlay"
            | "ChangeEntityState"
            | "ChangeSelfEntityState"
            | "HideGroup"
            | "ShowHidedGroup"
            | "HideSpecificEntities"
            | "ShowSpecificEntities"
            | "LockEntity"
            | "UnlockEntity"
            | "RemoveTrialCharacter"
            | "RestoreTrialCharacter"
            | "RemoveGuestCharacter"
            | "ExitDungeon"
            | "DestroyQuestItem"
            | "DestroyQuest"
            | "SendNpcMail"
            | "PhoneSystemInteract"
            | "ToggleMapMarkState"
            | "FinishCondition"
            | "GetItem"
    )
}

#[allow(clippy::too_many_arguments)]
fn replay_flow_safe_actions_for_skip<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    flow_name: &str,
    flow_id: i32,
    initial_state_id: i32,
    reconcile_only: bool,
    allow_world_layers: bool,
) -> io::Result<()> {
    if flow_name.is_empty() || flow_id <= 0 {
        return Ok(());
    }
    let mut visited_states = Vec::with_capacity(16);
    let mut state_id = initial_state_id;
    for _ in 0..16 {
        if state_id <= 0 || visited_states.contains(&state_id) {
            break;
        }
        visited_states.push(state_id);

        let Ok(Some(state_val)) = ctx.assets.flow_states.get_flow_state(flow_name, flow_id, state_id)
        else {
            break;
        };
        let mut flow_acts = Vec::new();
        if let Some(acts_val) = state_val.get("Actions") {
            collect_quest_actions_recursive(&mut flow_acts, acts_val, 0);
        }
        let mut next_state = 0;
        for act in flow_acts {
            let Some(aname) = json_str(act.get("Name")) else {
                continue;
            };
            let params_obj = act.get("Params").and_then(|p| p.as_object());
            if aname == "ChangeState" {
                if let Some(po) = params_obj
                    && let Some(ns) = json_int(po.get("StateId"))
                {
                    next_state = ns;
                }
                continue;
            }
            if !is_skip_safe_action_name(aname, params_obj, reconcile_only, allow_world_layers) {
                continue;
            }
            let mut cbs = action_callbacks::<S>();
            cbs.is_flow_action = true;
            cbs.defer_flow_transition = true;
            let _ = quest_actions::execute_action(ctx, &mut cbs, quest_id, node_id, &act);
        }
        state_id = next_state;
    }
    Ok(())
}

fn execute_skip_safe_node_actions<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    actions: &[Value],
    reconcile_only: bool,
    allow_world_layers: bool,
) -> io::Result<()> {
    for act in actions {
        let Some(aname) = json_str(act.get("Name")) else {
            continue;
        };
        let params_obj = act.get("Params").and_then(|p| p.as_object());
        if aname == "PlayFlow" {
            if let Some(po) = params_obj {
                let fln = json_str(po.get("FlowListName")).unwrap_or("");
                let fid = json_int(po.get("FlowId")).unwrap_or(0);
                let sid = json_int(po.get("StateId")).unwrap_or(1);
                replay_flow_safe_actions_for_skip(
                    ctx,
                    quest_id,
                    node_id,
                    fln,
                    fid,
                    sid,
                    reconcile_only,
                    allow_world_layers,
                )?;
            }
            continue;
        }
        if aname == "RunActions" {
            if let Some(po) = params_obj
                && let Some(Value::Array(al)) = po.get("ActionList")
            {
                execute_skip_safe_node_actions(
                    ctx,
                    quest_id,
                    node_id,
                    al,
                    reconcile_only,
                    allow_world_layers,
                )?;
            }
            continue;
        }
        if !is_skip_safe_action_name(aname, params_obj, reconcile_only, allow_world_layers) {
            continue;
        }
        let mut cbs = action_callbacks::<S>();
        let _ = quest_actions::execute_action(ctx, &mut cbs, quest_id, node_id, act);
    }
    Ok(())
}

fn visit_skip_tree_nodes(
    nodes: &[NodeEntry],
    node: &NodeEntry,
    out: &mut Vec<NodeEntry>,
    visited: &mut Vec<i32>,
) {
    if out.len() >= 1024 || node.node_type == "QuestFailed" || visited.contains(&node.id) {
        return;
    }
    visited.push(node.id);

    if node.node_type == "ConditionSelector" {
        if let Some(Value::Array(slots)) = node.data.get("Slots") {
            for slot in slots {
                if let Some(cid) = slot
                    .get("Node")
                    .and_then(|sn| sn.get("Id"))
                    .and_then(|v| json_int(Some(v)))
                    && let Some(cnode) = triggers::find_node_by_id(nodes, cid)
                {
                    visit_skip_tree_nodes(nodes, cnode, out, visited);
                    break;
                }
            }
        }
        out.push(node.clone());
        return;
    }

    let is_select = node.node_type == "Select";
    let mut picked_select_branch = false;
    for child in nodes {
        if child.parent_id != node.id || child.node_type == "QuestFailed" {
            continue;
        }
        if is_select && picked_select_branch {
            continue;
        }
        visit_skip_tree_nodes(nodes, child, out, visited);
        if is_select {
            picked_select_branch = true;
        }
    }
    out.push(node.clone());
}

fn collect_quest_skip_traversal(nodes: &[NodeEntry]) -> Vec<NodeEntry> {
    let mut out = Vec::new();
    let mut visited = Vec::new();
    let Some(root) = triggers::find_root_node(nodes).or_else(|| nodes.first()) else {
        return out;
    };
    visit_skip_tree_nodes(nodes, root, &mut out, &mut visited);
    for n in nodes {
        if n.node_type == "QuestFailed" || visited.contains(&n.id) {
            continue;
        }
        visited.push(n.id);
        out.push(n.clone());
    }
    out
}

fn active_quests_reference_action<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    target_names: &[&str],
) -> bool {
    for aq in &ctx.quest_comp.info.active_quests {
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
            continue;
        };
        for node in nodes.iter() {
            let mut acts = Vec::new();
            collect_quest_actions_recursive(&mut acts, &node.data, 0);
            for act in acts {
                if let Some(aname) = json_str(act.get("Name"))
                    && target_names.contains(&aname)
                {
                    return true;
                }
            }
        }
    }
    false
}

pub fn cleanup_orphaned_quest_environment<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    force_trials: bool,
) -> io::Result<AccountRepairReport> {
    let mut report = AccountRepairReport::default();
    let mut quest_dirty = false;

    if !ctx.quest_comp.info.env.trial_roles.is_empty() {
        let in_non_overworld = ctx
            .scene
            .as_deref()
            .is_some_and(|sc| sc.instance_id != 8);
        let needed_by_active = !in_non_overworld
            && active_quests_reference_action(
                ctx,
                &["AddTrialCharacter", "UpdateTrialCharacter"],
            );
        if force_trials || (!needed_by_active && !in_non_overworld) {
            report.removed_trial_roles = ctx.quest_comp.info.env.trial_roles.len();
            quest_actions::remove_trial_roles(ctx, &[])?;
            quest_dirty = true;
        }
    }

    if ctx
        .scene
        .as_deref()
        .is_some_and(|s| s.plot_added_rover_slot.is_some())
        && (force_trials || !active_quests_reference_action(ctx, &["PlayFlow"]))
    {
        triggers::restore_plot_main_role(ctx);
    }

    if ctx.quest_comp.info.env.time_locked
        && (force_trials
            || !active_quests_reference_action(ctx, &["SetTimeLockState", "SetAreaTimeState"]))
    {
        let h = ctx.quest_comp.info.env.time_lock_hour;
        let m = ctx.quest_comp.info.env.time_lock_minute;
        quest_actions::apply_time_lock_state(ctx, false, Some(h), Some(m))?;
        report.unlocked_locks += 1;
        quest_dirty = true;
    }

    if ctx.quest_comp.info.env.weather_locked
        && (force_trials || !active_quests_reference_action(ctx, &["SetWeatherLockState"]))
    {
        ctx.quest_comp.info.env.weather_locked = false;
        report.unlocked_locks += 1;
        quest_dirty = true;
    }

    if !ctx.quest_comp.info.world.active_guests.is_empty()
        && (force_trials || !active_quests_reference_action(ctx, &["AddGuestCharacter"]))
    {
        let guests: Vec<i32> = std::mem::take(&mut ctx.quest_comp.info.world.active_guests);
        for gid in guests {
            ctx.sink
                .push(RemoveGuestNotify::MSG_ID, &RemoveGuestNotify { GuestId: gid });
        }
        report.unlocked_locks += 1;
        quest_dirty = true;
    }

    if quest_dirty {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(report)
}

fn replay_quest_progression_effects<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    reconcile_only: bool,
    mark_nodes_finished: bool,
) -> io::Result<()> {
    let q_opt = ctx
        .assets
        .tables
        .quest_data
        .get_data_by_id(&quest_id)
        .map(|c| c.into_owned());
    let qtype = q_opt.as_ref().map_or(0, |q| q.Data.Type);
    let allow_world_layers = matches!(qtype, 1 | 3 | 9 | 16);

    if let Some(ref q) = q_opt {
        if !q.Data.ActiveActions.is_empty() {
            execute_skip_safe_node_actions(
                ctx,
                quest_id,
                0,
                &q.Data.ActiveActions,
                reconcile_only,
                allow_world_layers,
            )?;
        }
        if !q.Data.AcceptActions.is_empty() {
            execute_skip_safe_node_actions(
                ctx,
                quest_id,
                0,
                &q.Data.AcceptActions,
                reconcile_only,
                allow_world_layers,
            )?;
        }
    }

    let nodes = ctx
        .assets
        .quest_nodes
        .get_owner_nodes(quest_id)
        .unwrap_or_default();
    let ordered_nodes = collect_quest_skip_traversal(&nodes);
    for node in ordered_nodes {
        let mut node_acts = Vec::new();
        collect_quest_actions_recursive(&mut node_acts, &node.data, 0);
        if !node_acts.is_empty() {
            execute_skip_safe_node_actions(
                ctx,
                quest_id,
                node.id,
                &node_acts,
                reconcile_only,
                allow_world_layers,
            )?;
        }

        if mark_nodes_finished {
            ctx.quest_comp
                .info
                .record_child_quest_finished(quest_id, node.id);
            let newly_done = ctx.quest_comp.info.mark_node_finished(quest_id, node.id);
            if newly_done {
                triggers::push_node_status(
                    ctx,
                    quest_id,
                    node.id,
                    sonoro_proto::enums::NodeStatus::CompletedSuccess,
                    sonoro_proto::enums::ChildQuestNodeStatus::Finished,
                )?;
                let _ = tree::run_node_finish_effects(ctx, quest_id, &node);
            }
            if let Some(scene) = ctx.scene.as_deref_mut() {
                scene.release_occupations_for_node(quest_id, node.id);
            }
        } else if matches!(
            node.node_type.as_str(),
            "ChildQuest" | "Action" | "QuestSucceed"
        ) {
            ctx.quest_comp
                .info
                .record_child_quest_finished(quest_id, node.id);
        }
    }

    if let Some(ref q) = q_opt
        && !q.Data.FinishActions.is_empty()
    {
        execute_skip_safe_node_actions(
            ctx,
            quest_id,
            0,
            &q.Data.FinishActions,
            reconcile_only,
            allow_world_layers,
        )?;
    }
    Ok(())
}

pub fn reconcile_account_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    deep_repair: bool,
) -> io::Result<AccountRepairReport> {
    let mut report = AccountRepairReport::default();
    let prev_sys_len = ctx.quest_comp.info.unlocked_systems.len();

    if let Some(role_comp) = ctx.role_comp.as_deref_mut() {
        report.role_summary = role::repair_account_roles_and_weapons(
            ctx.fs,
            ctx.assets,
            ctx.scene.as_deref_mut(),
            ctx.basic_comp,
            role_comp,
            ctx.weapon_comp,
            ctx.echo_comp,
        )
        .map_err(io::Error::other)?;
    }

    let env_rep = cleanup_orphaned_quest_environment(ctx, deep_repair)?;
    report.removed_trial_roles += env_rep.removed_trial_roles;
    report.unlocked_locks += env_rep.unlocked_locks;

    if deep_repair
        && let Some(scene) = ctx.scene.as_deref()
        && scene.instance_id != 8
    {
        let inst_id = scene.instance_id;
        let owned_by_finished = ctx.quest_comp.info.finished_quests.iter().any(|&fq| {
            !ctx.quest_comp.info.has_active_quest(fq)
                && ctx
                    .assets
                    .tables
                    .quest_data
                    .get_data_by_id(&fq)
                    .is_some_and(|q| q.Data.DungeonId == inst_id)
        });
        if owned_by_finished {
            quest_actions::leave_current_dungeon(ctx, 0)?;
            report.left_dungeon = true;
        }
    }

    let finished_snap = ctx.quest_comp.info.finished_quests.clone();
    let mut progress_dirty = false;
    for qid in finished_snap {
        if qid <= 0 {
            continue;
        }
        let rkey = format!("reconcile:q:{qid}");
        let already = ctx
            .progress_comp
            .as_deref()
            .is_some_and(|pc| pc.info.get_key_int(&rkey) != 0);
        if !deep_repair && already {
            continue;
        }
        replay_quest_progression_effects(ctx, qid, true, false)?;
        report.repaired_quests += 1;
        if let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.info.set_key_int(&rkey, 1);
            progress_dirty = true;
        }
    }

    if ctx.quest_comp.info.unlocked_systems.len() > prev_sys_len {
        report.unlocked_systems = ctx.quest_comp.info.unlocked_systems.len() - prev_sys_len;
    }

    if deep_repair {
        let _ = tree::refresh_available_quests(ctx, false)?;
        triggers::ensure_tracked_focus_valid(ctx);
        quest_actions::push_func_open(ctx);
        triggers::push_quest_state_notifies(ctx)?;
        ensure_active_quest_targets_spawned(ctx);
    }

    if progress_dirty && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    ctx.quest_comp.save(ctx.fs)?;
    Ok(report)
}

pub fn gm_skip_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id_opt: Option<i32>,
) -> io::Result<i32> {
    let quest_id = match quest_id_opt {
        Some(qid) if qid > 0 => qid,
        _ => resolve_gm_target_quest_id(ctx, None),
    };
    if quest_id <= 0 {
        return Ok(0);
    }

    let exists = ctx.assets.tables.quest_data.get_data_by_id(&quest_id).is_some()
        || ctx.assets.quest_nodes.has_owner(quest_id)
        || ctx.quest_comp.info.has_active_quest(quest_id)
        || ctx.quest_comp.info.has_finished_quest(quest_id);
    if !exists {
        return Ok(0);
    }

    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.release_occupations_for_tree(quest_id, i64::from(quest_id));
        if scene.active_flow_inc_id != 0 && scene.active_flow_quest_id == quest_id {
            scene.clear_active_flow();
        }
    }

    if !ctx.quest_comp.info.has_active_quest(quest_id)
        && !ctx.quest_comp.info.has_finished_quest(quest_id)
    {
        ctx.quest_comp.info.get_or_add_active_quest(quest_id);
    }

    replay_quest_progression_effects(ctx, quest_id, false, true)?;

    if ctx.quest_comp.info.has_active_quest(quest_id) {
        tree::succeed_quest(ctx, quest_id)?;
    }

    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.info.set_key_int(&format!("reconcile:q:{quest_id}"), 1);
    }

    if let Some(scene) = ctx.scene.as_deref()
        && scene.instance_id != 8
        && let Some(q) = ctx.assets.tables.quest_data.get_data_by_id(&quest_id)
        && q.Data.DungeonId == scene.instance_id
    {
        quest_actions::leave_current_dungeon(ctx, 0)?;
    }

    let _ = cleanup_orphaned_quest_environment(ctx, false)?;
    if let Some(role_comp) = ctx.role_comp.as_deref_mut() {
        let _ = role::repair_account_roles_and_weapons(
            ctx.fs,
            ctx.assets,
            ctx.scene.as_deref_mut(),
            ctx.basic_comp,
            role_comp,
            ctx.weapon_comp,
            ctx.echo_comp,
        )
        .map_err(io::Error::other)?;
    }

    triggers::push_quest_state_notifies(ctx)?;
    ensure_active_quest_targets_spawned(ctx);
    tree::finalize_progression(ctx)?;
    Ok(quest_id)
}

pub fn claim_daily_rewards<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_ids: &[i32],
) -> io::Result<Vec<DailyQuestReward>> {
    let mut rewards = Vec::new();
    let mut any_claimed = false;

    for &qid in quest_ids {
        if qid <= 0
            || !ctx.quest_comp.info.daily.daily_quests.contains(&qid)
            || ctx
                .quest_comp
                .info
                .daily
                .claimed_daily_rewards
                .contains(&qid)
            || !ctx.quest_comp.info.has_finished_quest(qid)
        {
            continue;
        }
        if let Some(q) = ctx.assets.tables.quest_data.get_data_by_id(&qid)
            && q.Data.RewardId > 0
        {
            let _ = reward::grant_drop_package_popup(
                &mut ctx.make_reward_context(),
                q.Data.RewardId,
                6003,
            )?;
            for item in reward::preview_drop_items(ctx.assets, q.Data.RewardId) {
                rewards.push(DailyQuestReward {
                    ItemId: item.item_id,
                    ItemCount: item.count,
                });
            }
        }
        append_unique_int(&mut ctx.quest_comp.info.daily.claimed_daily_rewards, qid);
        any_claimed = true;
    }

    if any_claimed {
        triggers::push_daily_and_branch_notifies(ctx);
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(rewards)
}

pub fn change_daily_prefer<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    target_area: i32,
    target_influence: i32,
) -> io::Result<()> {
    if target_area > 0 {
        ctx.quest_comp.info.daily.preferred_daily_area = target_area;
    }
    ctx.quest_comp.info.daily.preferred_daily_influence = target_influence;
    triggers::push_daily_and_branch_notifies(ctx);
    ctx.quest_comp.save(ctx.fs)
}

pub fn sync_daily_and_branch_on_login<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    triggers::push_daily_and_branch_notifies(ctx);
}

pub fn set_tracked_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> io::Result<()> {
    if quest_id == 0 || ctx.quest_comp.info.has_active_quest(quest_id) {
        ctx.quest_comp.info.tracked_quest_id = quest_id;
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.current_quest_id = quest_id;
        }
        triggers::sync_scene_current_node(ctx);
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn set_scene_time<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    hour: i32,
    minute: i32,
    show_ui: bool,
) -> io::Result<()> {
    let cbs = action_callbacks::<S>();
    quest_actions::apply_set_time(ctx, &cbs, hour, minute, show_ui)?;
    ctx.quest_comp.save(ctx.fs)
}

pub fn change_weather<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    weather_id: i32,
) -> io::Result<()> {
    let cbs = action_callbacks::<S>();
    quest_actions::apply_weather_change(ctx, &cbs, weather_id)?;
    ctx.quest_comp.save(ctx.fs)
}

pub fn change_data_layer<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    load_layers: &[i32],
    unload_layers: &[i32],
) -> io::Result<()> {
    quest_actions::apply_data_layer_change(ctx, load_layers, unload_layers)?;
    if tree::recheck_pending_conditions(ctx)? {
        tree::finalize_progression(ctx)
    } else {
        ctx.quest_comp.save(ctx.fs)
    }
}

pub fn change_sub_levels<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    load_levels: &[String],
    unload_levels: &[String],
    hide_levels: &[String],
    tp_entity_id: i32,
) -> io::Result<()> {
    quest_actions::apply_sub_level_change(
        ctx,
        load_levels,
        unload_levels,
        hide_levels,
        tp_entity_id,
    )?;
    if tree::recheck_pending_conditions(ctx)? {
        tree::finalize_progression(ctx)
    } else {
        ctx.quest_comp.save(ctx.fs)
    }
}

pub fn enter_inst<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    inst_id: i32,
    pos_entity_id: i32,
) -> io::Result<()> {
    if inst_id <= 0 {
        return Ok(());
    }
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.clear_failed_dungeon(inst_id);
    }
    quest_actions::apply_instance_trial_roles(ctx, inst_id)?;
    let target_pos = if pos_entity_id > 0 {
        ctx.assets
            .find_level_entity(inst_id, i64::from(pos_entity_id))
            .and_then(|cfg| entity_world_pos(&cfg))
    } else {
        None
    };
    quest_actions::transition_to_scene(ctx, inst_id, target_pos, None)?;
    events::on_dungeon_event(ctx, inst_id, false)
}

pub fn leave_inst<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    pos_entity_id: i32,
) -> io::Result<()> {
    let prev_inst = ctx.scene.as_deref().map_or(0, |s| s.instance_id);
    quest_actions::leave_current_dungeon(ctx, pos_entity_id)?;
    if prev_inst > 0
        && prev_inst != 8
        && let Some(scene) = ctx.scene.as_deref_mut()
        && !ctx.quest_comp.info.completed_dungeons.contains(&prev_inst)
    {
        scene.record_failed_dungeon(prev_inst);
    }
    Ok(())
}

pub fn restart_flow<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>, flow_inc_id: i64) {
    let Some((flow_name, cur_inc_id, qid, node_id, flow_id, state_id)) =
        ctx.scene.as_deref().and_then(|scene| {
            if scene.active_flow_list_name.is_empty() {
                return None;
            }
            if flow_inc_id != 0
                && scene.active_flow_inc_id != 0
                && scene.active_flow_inc_id != flow_inc_id
            {
                return None;
            }
            Some((
                scene.active_flow_list_name.clone(),
                scene.active_flow_inc_id,
                scene.active_flow_quest_id,
                scene.active_flow_node_id,
                scene.active_flow_id,
                scene.active_flow_state_id,
            ))
        })
    else {
        return;
    };

    triggers::prepare_flow_state_entities(ctx, &flow_name, flow_id, state_id);
    let inc_id = if cur_inc_id != 0 {
        cur_inc_id
    } else {
        ctx.quest_comp.info.alloc_flow_inc_id()
    };
    let bt_inc_id = if qid != 0 { i64::from(qid) } else { inc_id };
    ctx.sink.push(
        FlowStartNotify::MSG_ID,
        &FlowStartNotify {
            FlowIncId: inc_id,
            FlowListName: flow_name.into_bytes(),
            FlowId: flow_id,
            StateId: state_id,
            GameCtx: Some(GameCtxPb {
                CtxType: Some(sonoro_proto::enums::GameCtxType::BehaviorTree),
                CtxInfo: Some(GameCtxPbCtxInfoOneof::BehaviorTree(BehaviorTreeCtxPb {
                    IncId: bt_inc_id,
                    BtType: Some(BtType::BtTypeQuest),
                    BtId: qid,
                    NodeId: node_id,
                })),
            }),
            ..Default::default()
        },
    );
}

pub fn eval_condition_groups<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    group_ids: &[i32],
) -> Vec<ConditionGroupFinishInfo> {
    let eval_ctx = ctx.make_eval_context();
    let mut result = Vec::with_capacity(group_ids.len());
    for &gid in group_ids {
        let mut fin = Vec::new();
        if let Some(grp) = ctx.assets.tables.condition_group.get_data_by_id(&gid) {
            for &cid in &grp.GroupId {
                if condition::eval_condition_id(&eval_ctx, cid) {
                    fin.push(cid);
                }
            }
        }
        result.push(ConditionGroupFinishInfo {
            ConditionGroupId: gid,
            FinishConditions: fin,
        });
    }
    result
}

pub fn collect_missing_resource_quests<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
) -> Vec<i32> {
    let Some(pc) = ctx.progress_comp.as_deref() else {
        return Vec::new();
    };
    ctx.quest_comp
        .info
        .active_quests
        .iter()
        .filter_map(|aq| {
            if !ctx.assets.quest_requires_resource(aq.quest_id) {
                return None;
            }
            if pc.info.has_claim(&format!("quest:dl:{}", aq.quest_id))
                || pc.info.has_claim(&format!("quest:res_ok:{}", aq.quest_id))
            {
                return None;
            }
            Some(aq.quest_id)
        })
        .collect()
}

pub fn sync_quest_resource_availability<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    downloaded_quest_ids: &[i32],
    confirmed_quest_ids: &[i32],
    is_full_report: bool,
) -> io::Result<()> {
    let (reported, should_return) = {
        let Some(pc) = ctx.progress_comp.as_deref_mut() else {
            return Ok(());
        };
        let mut dirty = false;
        if is_full_report && pc.info.get_key_int("quest:res_reported") == 0 {
            pc.info.set_key_int("quest:res_reported", 1);
            dirty = true;
        }
        for &qid in downloaded_quest_ids {
            if qid > 0 && pc.info.mark_claim(&format!("quest:dl:{qid}")) {
                dirty = true;
            }
        }
        for &qid in confirmed_quest_ids {
            if qid > 0 {
                if pc.info.mark_claim(&format!("quest:res_ok:{qid}")) {
                    dirty = true;
                }
                if pc.info.mark_claim(&format!("quest:dl:{qid}")) {
                    dirty = true;
                }
            }
        }
        let early = pc.info.get_key_int("quest:res_reported") == 0 && confirmed_quest_ids.is_empty();
        if early && dirty {
            pc.save(ctx.fs)?;
        }
        (dirty, early)
    };
    if should_return {
        return Ok(());
    }

    let missing = collect_missing_resource_quests(ctx);
    let csv: String = missing
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        let mut dirty = reported;
        if pc.info.get_key_str("quest:missing_res") != csv {
            pc.info.set_key_str("quest:missing_res", &csv);
            dirty = true;
        }
        if dirty {
            pc.save(ctx.fs)?;
        }
    }

    let lock_infos = missing
        .iter()
        .map(|&qid| QuestLockInfo {
            QuestId: qid,
            MultiModeLock: false,
            LockType: Some(QuestLockType::QuestResource),
        })
        .collect();
    ctx.sink.push(
        WaitQuestConfirmResourceNotify::MSG_ID,
        &WaitQuestConfirmResourceNotify {
            LackResourceQuestIds: missing,
        },
    );
    ctx.sink.push(
        QuestLockInfoNotify::MSG_ID,
        &QuestLockInfoNotify {
            QuestLockInfos: lock_infos,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn init_tree_info_binds_quest_blackboard_id() {
        let info = triggers::init_tree_info(139000025, BTreeMap::new(), Vec::new());
        assert_eq!(info.TreeOwnerId, 139000025);
        assert_eq!(info.TreeIncId, 139000025);
        assert_eq!(info.BtType, Some(BtType::BtTypeQuest));
        assert_eq!(info.BlackboardId, 139000025);
    }

    #[test]
    fn parse_timer_delay_ms_supports_wait_time_days_hours_minutes_seconds_and_timer() {
        let wt: Value = serde_json::from_str(
            r#"{"Type":"WaitTime","Days":0,"Hours":1,"Minutes":0,"Seconds":0}"#,
        )
        .unwrap();
        assert_eq!(parse_timer_delay_ms_pub(wt.as_object().unwrap()), Some(8000));

        let tm: Value = serde_json::from_str(r#"{"Type":"Timer","Time":5}"#).unwrap();
        assert_eq!(parse_timer_delay_ms_pub(tm.as_object().unwrap()), Some(5000));
    }

    #[test]
    fn extract_node_target_entity_ids_captures_take_photo2_and_check_entity_state_targets() {
        let parsed: Value = serde_json::from_str(
            r#"{"Condition":{"Type":"TakePhoto2","PhotoTargets":[{"EntityId":181850024}],"PosCondition":{"RangeEntity":181850026}}}"#,
        )
        .unwrap();
        let node = NodeEntry {
            id: 1,
            parent_id: 0,
            node_type: "ChildQuest".to_string(),
            count: 0,
            reward_id: 0,
            data: parsed,
        };
        assert_eq!(
            extract_node_target_entity_ids(&node),
            vec![181850024, 181850026]
        );
    }

    #[test]
    fn is_schedule_time_window_match_handles_daytime_and_wraparound_windows() {
        let day: Value = serde_json::from_str(
            r#"{"Type":"ScheduleTime","StartTime":{"Hour":15,"Minute":0},"EndTime":{"Hour":17,"Minute":0}}"#,
        )
        .unwrap();
        assert!(triggers::is_schedule_time_window_match(
            day.as_object().unwrap(),
            16,
            0,
            1
        ));
        assert!(!triggers::is_schedule_time_window_match(
            day.as_object().unwrap(),
            12,
            0,
            1
        ));

        let night: Value = serde_json::from_str(
            r#"{"Type":"ScheduleTime","StartTime":{"Hour":22,"Minute":0},"EndTime":{"Hour":4,"Minute":0}}"#,
        )
        .unwrap();
        assert!(triggers::is_schedule_time_window_match(
            night.as_object().unwrap(),
            23,
            30,
            1
        ));
        assert!(triggers::is_schedule_time_window_match(
            night.as_object().unwrap(),
            2,
            0,
            1
        ));
        assert!(!triggers::is_schedule_time_window_match(
            night.as_object().unwrap(),
            10,
            0,
            1
        ));
    }

    #[test]
    fn is_client_action_name_registered_allows_safe_client_level_events_and_filters_unsafe() {
        assert!(is_client_action_name_registered("FadeInScreen"));
        assert!(is_client_action_name_registered("ChangeEntityState"));
        assert!(is_client_action_name_registered("PlayMontage"));
        assert!(is_client_action_name_registered("Prompt"));

        assert!(!is_client_action_name_registered("AwakeEntity"));
        assert!(!is_client_action_name_registered("HideGroup"));
        assert!(!is_client_action_name_registered("RemoveBuffFromPlayer"));
        assert!(!is_client_action_name_registered("SetVar"));
        assert!(!is_client_action_name_registered("UnlockTeleportTrigger"));

        assert!(!is_client_action_name_registered("LeisureInteract"));
        assert!(!is_client_action_name_registered("AddBuffToPlayer"));
        assert!(!is_client_action_name_registered("AddBuffToEntity"));
        assert!(!is_client_action_name_registered("OpenSystemFunction"));
    }

    #[test]
    fn is_safe_bt_leisure_interact_allows_context_independent_options_and_explicit_sit_targets() {
        let custom_skill: Value = serde_json::from_str(
            r#"{"Name":"LeisureInteract","Params":{"Option":{"Type":"CustomSkill","SkillId":800022}}}"#,
        )
        .unwrap();
        assert!(is_safe_bt_leisure_interact(&custom_skill));

        let sit_with_target: Value = serde_json::from_str(
            r#"{"Name":"LeisureInteract","Params":{"Option":{"Type":"SitDown"},"SceneEntity":38750004}}"#,
        )
        .unwrap();
        assert!(is_safe_bt_leisure_interact(&sit_with_target));

        let sit_without_target: Value =
            serde_json::from_str(r#"{"Name":"LeisureInteract","Params":{"Option":{"Type":"SitDown"}}}"#)
                .unwrap();
        assert!(!is_safe_bt_leisure_interact(&sit_without_target));

        let context_cast: Value =
            serde_json::from_str(r#"{"Name":"LeisureInteract","Params":{"Option":{"Type":"Catapult"}}}"#)
                .unwrap();
        assert!(!is_safe_bt_leisure_interact(&context_cast));
    }

    #[test]
    fn reentrant_refresh_flags_and_should_hide_world_entity_prevent_recursion_and_ghost_spawns() {
        tree::REFRESH_LOOP_DEPTH.with(|d| d.set(1));
        tree::PENDING_REFRESH_QUESTS.with(|p| p.set(false));
        tree::PENDING_REFRESH_PUSH.with(|p| p.set(false));
        tree::PENDING_RECHECK_CONDITIONS.with(|p| p.set(false));

        assert_eq!(tree::REFRESH_LOOP_DEPTH.with(|d| d.get()), 1);

        tree::REFRESH_LOOP_DEPTH.with(|d| d.set(0));
        tree::PENDING_REFRESH_QUESTS.with(|p| p.set(false));
        tree::PENDING_REFRESH_PUSH.with(|p| p.set(false));
        tree::PENDING_RECHECK_CONDITIONS.with(|p| p.set(false));

        let mut targets = HashSet::new();
        targets.insert(310000014);

        let mut info = QuestInfo::default();
        info.world.hidden_entities = vec![310000014];
        info.world.shown_entities = vec![880700164];

        assert!(should_hide_world_entity(&info, &targets, 310000014));
        assert!(!should_hide_world_entity(&info, &targets, 880700164));
    }

    #[test]
    fn allows_client_submit_node_blocks_do_interact_and_play_flow_while_allowing_reach_area() {
        let interact_node: Value =
            serde_json::from_str(r#"{"Type":"ChildQuest","Condition":{"Type":"DoInteract"}}"#)
                .unwrap();
        assert!(!allows_client_submit_node(&interact_node));

        let playflow_node: Value =
            serde_json::from_str(r#"{"Type":"ChildQuest","Condition":{"Type":"PlayFlow"}}"#)
                .unwrap();
        assert!(!allows_client_submit_node(&playflow_node));

        let reach_node: Value =
            serde_json::from_str(r#"{"Type":"ChildQuest","Condition":{"Type":"ReachArea"}}"#)
                .unwrap();
        assert!(allows_client_submit_node(&reach_node));
    }
}
