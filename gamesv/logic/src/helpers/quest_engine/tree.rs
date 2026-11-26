use std::cell::Cell;
use std::io;

use serde_json::{Map, Value};
use sonoro_gamesv_data::asset_indexes::entity_world_pos;
use sonoro_gamesv_data::lazy_node_store::NodeEntry;
use sonoro_gamesv_data::tables::quest_data::DataEntry as QuestDataEntry;
use sonoro_gamesv_fs::quest_info::{ActiveQuest, append_unique_int, remove_int_value};
use sonoro_proto::enums::{
    BtType, ChildQuestNodeStatus, FailReason, GameCtxType, NodeStatus, QuestState, SuspendType,
};
use sonoro_proto::pb::{
    BehaviorTreeInfoNotify, BtRollbackInfo, BtRollbackInfoNotify, BtRollbackNotify,
    BtRollbackStartNotify, BtSuspendNotify, FailRollbackData, LowMemoryChangeSceneStartNotify,
    OccupationPbInfo, OperationRestrictionNotify, QuestFinishListNotify, QuestListNotify,
    QuestRollBackEndNotify, QuestShowListNotify, QuestInfo as PbQuestInfo, TraceQuestNotify,
    UpdateNodeStatusNotify,
};
use sonoro_proto::wire::PbEnum;

use crate::helpers::condition;
use crate::helpers::level_play::{self, LevelPlayContext};
use crate::helpers::quest_actions;
use crate::helpers::quest_types::{
    QuestContext, json_bool, json_float, json_int, json_str, repeater_cooldown_key,
    subtree_wait_delay_ms,
};
use crate::helpers::reward;
use crate::helpers::role;
use crate::scene::now_millis;
use crate::schedulers::quest_timer_queue::{QuestTimerEntry, QuestTimerQueue};
use crate::sink::PushSink;

use super::triggers::{
    action_node_progress_key, build_pre_show_quest_list, build_tree_info, entity_hp_percent,
    eval_compare_var_list, execute_actions, extract_node_action_items, find_node_by_id,
    find_root_node, get_child_condition, init_tree_info, is_sync_play_flow_action,
    push_ancestor_activations, push_bt_level_events, push_daily_and_branch_notifies,
    push_do_interact_options_for_node, push_node_status, push_ready_quest_list,
    remove_do_interact_options_for_node, should_auto_complete_child_quest,
    snapshot_active_quest_ids, start_flow_from_obj, sync_scene_current_node,
    ensure_tracked_focus_valid,
};
use super::{PROLOGUE_QUEST_ID, interrupt_active_flow_for_quest, spawn_node_target_entities};

const MAX_REFRESH_PASSES: usize = 8;
const MAX_TRANSITION_DEPTH: u8 = 16;
const MAX_STEPS_PER_ADVANCE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    Running,
    Completed,
    Failed,
    QuestEnded,
}

thread_local! {
    pub(super) static REFRESH_LOOP_DEPTH: Cell<u8> = const { Cell::new(0) };
    pub(super) static PENDING_REFRESH_QUESTS: Cell<bool> = const { Cell::new(false) };
    pub(super) static PENDING_REFRESH_PUSH: Cell<bool> = const { Cell::new(false) };
    pub(super) static PENDING_RECHECK_CONDITIONS: Cell<bool> = const { Cell::new(false) };
    static QUEST_TRANSITION_DEPTH: Cell<u8> = const { Cell::new(0) };
    static FINALIZING_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct RefreshLoopGuard;
impl RefreshLoopGuard {
    fn enter() -> Self {
        REFRESH_LOOP_DEPTH.with(|d| d.set(d.get().saturating_add(1)));
        Self
    }
}
impl Drop for RefreshLoopGuard {
    fn drop(&mut self) {
        REFRESH_LOOP_DEPTH.with(|d| {
            let next = d.get().saturating_sub(1);
            d.set(next);
            if next == 0 {
                PENDING_REFRESH_QUESTS.with(|p| p.set(false));
                PENDING_REFRESH_PUSH.with(|p| p.set(false));
                PENDING_RECHECK_CONDITIONS.with(|p| p.set(false));
            }
        });
    }
}

struct TransitionDepthGuard;
impl TransitionDepthGuard {
    fn try_enter() -> Option<Self> {
        QUEST_TRANSITION_DEPTH.with(|d| {
            let cur = d.get();
            if cur >= MAX_TRANSITION_DEPTH {
                None
            } else {
                d.set(cur + 1);
                Some(Self)
            }
        })
    }
}
impl Drop for TransitionDepthGuard {
    fn drop(&mut self) {
        QUEST_TRANSITION_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

struct FinalizingGuard;
impl FinalizingGuard {
    fn enter() -> Self {
        FINALIZING_DEPTH.with(|d| d.set(d.get().saturating_add(1)));
        Self
    }
}
impl Drop for FinalizingGuard {
    fn drop(&mut self) {
        FINALIZING_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

pub fn now_ms_pub() -> i64 {
    now_millis()
}

pub fn parse_timer_delay_ms_pub(cond: &Map<String, Value>) -> Option<i64> {
    parse_timer_delay_ms(cond)
}

pub fn parse_timer_delay_ms(cond: &Map<String, Value>) -> Option<i64> {
    let ctype = json_str(cond.get("Type"))?;
    if ctype == "Timer" {
        let seconds = json_float(cond.get("Time")).unwrap_or(0.0);
        if !seconds.is_finite() || seconds <= 0.0 {
            return Some(0);
        }
        let ms_f = (f64::from(seconds) * 1000.0).round();
        if ms_f >= i64::MAX as f64 {
            return Some(i64::MAX);
        }
        return Some(ms_f as i64);
    }
    if ctype == "WaitTime" {
        if let Some(seconds) = json_float(cond.get("Time")) {
            if !seconds.is_finite() || seconds <= 0.0 {
                return Some(0);
            }
            let ms_f = (f64::from(seconds) * 1000.0).round();
            if ms_f >= i64::MAX as f64 {
                return Some(i64::MAX);
            }
            return Some(ms_f as i64);
        }
        let days = json_int(cond.get("Days")).unwrap_or(0);
        let hours = json_int(cond.get("Hours")).unwrap_or(0);
        let mins = json_int(cond.get("Minutes")).unwrap_or(0);
        let secs = json_int(cond.get("Seconds")).unwrap_or(0);
        let total_game_sec =
            i64::from(days) * 86400 + i64::from(hours) * 3600 + i64::from(mins) * 60 + i64::from(secs);
        if total_game_sec <= 0 {
            return Some(0);
        }
        let scaled_ms = (total_game_sec * 1000) / 60;
        return Some(scaled_ms.clamp(1500, 8000));
    }
    None
}

pub fn entity_has_state_tag<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    eid: i32,
    want_state: &str,
) -> bool {
    if eid <= 0 || want_state.is_empty() {
        return false;
    }
    if ctx
        .quest_comp
        .info
        .get_entity_state(i64::from(eid))
        .is_some_and(|st| st == want_state)
    {
        return true;
    }
    let Some(scene) = ctx.scene.as_deref() else {
        return false;
    };
    let Some(tag_id) = crate::helpers::gameplay_tags::id_from_name(
        &ctx.assets.tables.gameplay_tag_remap,
        want_state,
    ) else {
        return false;
    };
    for s in &scene.entities {
        if s.config.config_id != eid {
            continue;
        }
        if let Some(tag_comp) = s.tag.as_ref()
            && tag_comp.has_tag(&ctx.assets.tables.gameplay_tag_parent, tag_id)
        {
            return true;
        }
    }
    false
}

pub fn advance_matching_child_nodes<S, E, F>(
    ctx: &mut QuestContext<'_, S>,
    ev: &E,
    mut check: F,
) -> io::Result<bool>
where
    S: PushSink + ?Sized,
    F: FnMut(&mut QuestContext<'_, S>, &E, i32, &ActiveQuest, &[NodeEntry], &NodeEntry, &Map<String, Value>, &str) -> io::Result<bool>,
{
    let active_snapshot = snapshot_active_quest_ids(ctx);
    let mut progressed = false;
    for qid in active_snapshot {
        if super::is_login_reentry_pending(ctx, qid) {
            continue;
        }
        let Some(aq) = ctx.quest_comp.info.find_active_quest(qid).cloned() else {
            continue;
        };
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(qid) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            if let Some(node) = find_node_by_id(&nodes, nid)
                && let Some(cond) = get_child_condition(&node.data)
                && let Some(ctype) = json_str(cond.get("Type"))
                && check(ctx, ev, qid, &aq, &nodes, node, cond, ctype)?
            {
                progressed = true;
            }
        }
    }
    Ok(progressed)
}

pub fn complete_matching_child_quests<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    cond_type: &str,
    id_field: &str,
    target_id: i32,
) -> io::Result<bool> {
    advance_matching_child_nodes(
        ctx,
        &(cond_type, id_field, target_id),
        |c, &(ct, field, tid), qid, _aq, nodes, node, cond, ctype| {
            if ctype != ct {
                return Ok(false);
            }
            let wanted = json_int(cond.get(field)).unwrap_or(0);
            if wanted != 0 && wanted != tid {
                return Ok(false);
            }
            complete_child_quest(c, qid, nodes, node)?;
            Ok(true)
        },
    )
}

pub fn recheck_pending_conditions<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<bool> {
    if REFRESH_LOOP_DEPTH.with(|d| d.get()) > 0 {
        PENDING_RECHECK_CONDITIONS.with(|p| p.set(true));
        return Ok(false);
    }
    run_refresh_and_recheck_loop(ctx, false, false, true)
}

fn recheck_pending_conditions_pass<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<bool> {
    advance_matching_child_nodes(ctx, &(), |c, _, qid, _aq, nodes, node, _cond, ctype| {
        if super::is_login_reentry_pending(c, qid) {
            return Ok(false);
        }
        if matches!(
            ctype,
            "CheckConditionGroup"
                | "CheckClientConditionGroup"
                | "CompareVar"
                | "CheckEntityState"
                | "CheckLevelPlay"
                | "CheckActivityState"
                | "GetItem"
                | "GetSkill"
                | "VisionSystem"
                | "ScheduleTime"
                | "EnterDungeon"
                | "ReadMail"
                | "CheckTargetBattleAttribute"
                | "WaitBattleCondition"
                | "AwakeAndLoadEntity"
                | "AtomicProcess"
                | "CheckSpringFestivalGameplayCompleted"
                | "CheckUiGame"
                | "CheckFinishLoading"
                | "CheckElementalBoxState"
                | "CheckEntityBuffStack"
                | "CheckEntitySelfDirection"
                | "CheckIsInDragActorPlay"
                | "CompareTeammateDie"
                | "CompareCost"
                | "IsCapableToEvolve"
                | "IsCore"
                | "ActiveSkillTag"
                | "HasOptimalFactor"
                | "CheckBVBEvent"
                | "CheckChessWinner"
                | "CheckHasFollowShooter"
                | "CheckIsGramophonePlayingMusic"
                | "CheckPhotographCameraParam"
                | "CheckPlayerUseSkill"
                | "CheckRoverRogueAbilitySelect"
        ) {
            if should_auto_complete_child_quest(c, qid, node) {
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
        } else if ctype == "LeaveDungeon" && c.scene.as_deref().is_some_and(|s| s.instance_id == 8) {
            complete_child_quest(c, qid, nodes, node)?;
            return Ok(true);
        }
        Ok(false)
    })
}

pub fn step_pending_control_nodes<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<bool> {
    let active_snapshot = snapshot_active_quest_ids(ctx);
    let mut progressed = false;
    for qid in active_snapshot {
        if super::is_login_reentry_pending(ctx, qid) {
            continue;
        }
        let Some(aq) = ctx.quest_comp.info.find_active_quest(qid).cloned() else {
            continue;
        };
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(qid) else {
            continue;
        };
        let has_control = nodes.iter().any(|n| {
            !aq.is_node_finished(n.id)
                && matches!(
                    n.node_type.as_str(),
                    "Condition" | "ConditionSelector" | "Repeater" | "Parallel" | "ParallelSelect"
                )
        });
        if has_control || aq.active_nodes.is_empty() {
            advance_quest_tree(ctx, qid, true)?;
            progressed = true;
        }
    }
    Ok(progressed)
}

pub fn refresh_daily_quests<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    push_updates: bool,
) -> io::Result<bool> {
    if !ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID) {
        return Ok(false);
    }

    let active_or_unclaimed = ctx
        .quest_comp
        .info
        .daily
        .daily_quests
        .iter()
        .any(|&dq| !ctx.quest_comp.info.daily.claimed_daily_rewards.contains(&dq));
    if !ctx.quest_comp.info.daily.daily_quests.is_empty() && active_or_unclaimed {
        let mut added_missing = false;
        let daily_snap = ctx.quest_comp.info.daily.daily_quests.clone();
        for dq in daily_snap {
            if !ctx.quest_comp.info.has_finished_quest(dq)
                && !ctx.quest_comp.info.has_active_quest(dq)
                && activate_quest(ctx, dq, push_updates)?
            {
                added_missing = true;
            }
        }
        return Ok(added_missing);
    }

    let mut seen_types = [0i32; 4];
    let mut picked_count = 0usize;
    let mut any_added = false;
    let pref_country = if ctx.quest_comp.info.daily.preferred_daily_area > 0 {
        ctx.quest_comp.info.daily.preferred_daily_area
    } else {
        1
    };

    let groups: Vec<_> = ctx.assets.tables.daily_task_group.items.clone();
    for grp in groups {
        if picked_count >= 4 {
            break;
        }
        if grp.CountryId != pref_country || grp.TaskIds.is_empty() {
            continue;
        }
        if seen_types[..picked_count].contains(&grp.TypeId) {
            continue;
        }
        for tid in grp.TaskIds {
            if ctx.quest_comp.info.has_finished_quest(tid)
                || ctx.quest_comp.info.has_active_quest(tid)
                || !ctx.assets.quest_nodes.has_owner(tid)
            {
                continue;
            }
            let Some(q) = ctx.assets.tables.quest_data.get_data_by_id(&tid) else {
                continue;
            };
            if !condition::eval_quest_provide_conditions(
                &ctx.make_eval_context(),
                q.Data.ProvideType.as_ref(),
            ) {
                continue;
            }
            append_unique_int(&mut ctx.quest_comp.info.daily.daily_quests, tid);
            seen_types[picked_count] = grp.TypeId;
            picked_count += 1;
            if activate_quest(ctx, tid, push_updates)? {
                any_added = true;
            }
            break;
        }
    }
    Ok(any_added)
}

pub fn refresh_available_quests<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    push_updates: bool,
) -> io::Result<bool> {
    if REFRESH_LOOP_DEPTH.with(|d| d.get()) > 0 {
        PENDING_REFRESH_QUESTS.with(|p| p.set(true));
        if push_updates {
            PENDING_REFRESH_PUSH.with(|p| p.set(true));
        }
        return Ok(false);
    }
    run_refresh_and_recheck_loop(ctx, true, push_updates, false)
}

fn run_refresh_and_recheck_loop<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    initial_refresh: bool,
    initial_push: bool,
    initial_recheck: bool,
) -> io::Result<bool> {
    let _guard = RefreshLoopGuard::enter();
    PENDING_REFRESH_QUESTS.with(|p| p.set(initial_refresh));
    PENDING_REFRESH_PUSH.with(|p| p.set(initial_push));
    PENDING_RECHECK_CONDITIONS.with(|p| p.set(initial_recheck));

    let mut any_progressed = false;
    let mut pass = 0usize;
    while pass < MAX_REFRESH_PASSES {
        let do_refresh = PENDING_REFRESH_QUESTS.with(|p| p.get());
        let do_recheck = PENDING_RECHECK_CONDITIONS.with(|p| p.get());
        if !do_refresh && !do_recheck {
            break;
        }
        pass += 1;
        if do_refresh {
            let push = PENDING_REFRESH_PUSH.with(|p| p.get());
            PENDING_REFRESH_QUESTS.with(|p| p.set(false));
            PENDING_REFRESH_PUSH.with(|p| p.set(false));
            if refresh_available_quests_pass(ctx, push)? {
                any_progressed = true;
            }
        }
        if PENDING_RECHECK_CONDITIONS.with(|p| p.get()) {
            PENDING_RECHECK_CONDITIONS.with(|p| p.set(false));
            if recheck_pending_conditions_pass(ctx)? {
                any_progressed = true;
            }
        }
    }
    Ok(any_progressed)
}

fn refresh_available_quests_pass<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    push_updates: bool,
) -> io::Result<bool> {
    let prologue_done = ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID);
    let mut eligible_ids = Vec::new();
    let eval_ctx = ctx.make_eval_context();
    for q in &ctx.assets.tables.quest_data.items {
        let d = &q.Data;
        if !prologue_done && d.Id != PROLOGUE_QUEST_ID {
            continue;
        }
        if !is_eligible_quest(ctx, d, true) {
            continue;
        }
        if !condition::eval_quest_provide_conditions(&eval_ctx, d.ProvideType.as_ref()) {
            continue;
        }
        eligible_ids.push(d.Id);
    }

    let mut any_added = false;
    for qid in eligible_ids {
        if activate_quest(ctx, qid, push_updates)? {
            any_added = true;
        }
    }
    Ok(any_added)
}

pub fn is_eligible_quest<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    d: &QuestDataEntry,
    auto_only: bool,
) -> bool {
    if d.Id <= 0 || d.Type == 100 {
        return false;
    }
    if d.Type == 15
        && (d.ActivityId <= 0
            || !ctx.quest_comp.info.progress.opened_activities.contains(&d.ActivityId))
    {
        return false;
    }
    if ctx.quest_comp.info.has_finished_quest(d.Id)
        || ctx.quest_comp.info.has_active_quest(d.Id)
        || !ctx.assets.quest_nodes.has_owner(d.Id)
    {
        return false;
    }

    if auto_only {
        let is_sys = d.DistributeType == "System";
        let is_passive = d.DistributeType == "Passive";
        let is_activity = d.DistributeType == "Activity"
            && (d.ActivityId == 0
                || condition::is_activity_open(&ctx.make_eval_context(), d.ActivityId));
        if !is_sys && !is_passive && !is_activity {
            return false;
        }

        let explicitly_opened = is_activity
            && d.ActivityId > 0
            && ctx.quest_comp.info.progress.opened_activities.contains(&d.ActivityId);
        let is_standard_quest = (1..=4).contains(&d.Type) || d.Type == 7 || d.Type == 9;
        if !is_standard_quest && !explicitly_opened {
            let has_meaningful_cond = (|| -> bool {
                let Some(Value::Object(pt)) = &d.ProvideType else {
                    return false;
                };
                let Some(Value::Array(cv)) = pt.get("Conditions") else {
                    return false;
                };
                if cv.is_empty() {
                    return false;
                }
                for item in cv {
                    let Value::Object(obj) = item else {
                        return true;
                    };
                    let Some(ctype) = json_str(obj.get("Type")) else {
                        return true;
                    };
                    if ctype == "OpenDate" || ctype == "Gender" {
                        continue;
                    }
                    if ctype == "ExploreLevel" {
                        let req_lv = json_int(obj.get("ExploreLevel")).unwrap_or(1);
                        if d.Type == 11 && req_lv <= 5 {
                            continue;
                        }
                    }
                    return true;
                }
                false
            })();
            if !has_meaningful_cond {
                return false;
            }
        }
    }
    true
}

pub fn activate_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    push_updates: bool,
) -> io::Result<bool> {
    let Some(_guard) = TransitionDepthGuard::try_enter() else {
        return Ok(false);
    };
    if ctx.quest_comp.info.has_active_quest(quest_id)
        || ctx.quest_comp.info.has_finished_quest(quest_id)
    {
        return Ok(false);
    }

    ctx.quest_comp.info.get_or_add_active_quest(quest_id);

    if push_updates {
        ctx.sink.push(
            QuestListNotify::MSG_ID,
            &QuestListNotify {
                Quests: vec![PbQuestInfo {
                    QuestId: quest_id,
                    Status: Some(QuestState::InProgress),
                }],
            },
        );
        let show_list = build_pre_show_quest_list(ctx);
        ctx.sink.push(
            QuestShowListNotify::MSG_ID,
            &QuestShowListNotify { QuestId: show_list },
        );
        push_ready_quest_list(ctx);
        ctx.sink.push(
            BehaviorTreeInfoNotify::MSG_ID,
            &BehaviorTreeInfoNotify {
                TreeInfos: vec![init_tree_info(quest_id, Default::default(), Vec::new())],
            },
        );
    }

    if let Some(q) = ctx.assets.tables.quest_data.get_data_by_id(&quest_id) {
        execute_actions(ctx, quest_id, 0, &q.Data.ActiveActions)?;
        execute_actions(ctx, quest_id, 0, &q.Data.AcceptActions)?;
    }

    advance_quest_tree(ctx, quest_id, push_updates)?;
    Ok(true)
}

pub fn advance_quest_tree<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    push_updates: bool,
) -> io::Result<()> {
    if super::is_login_reentry_pending(ctx, quest_id) {
        return Ok(());
    }
    let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
        return Ok(());
    };
    let Some(root) = find_root_node(&nodes).or_else(|| nodes.first()) else {
        return Ok(());
    };
    let mut steps = 0usize;
    let res = step_node(ctx, quest_id, &nodes, root, push_updates, &mut steps)?;
    if res == StepResult::Completed && ctx.quest_comp.info.has_active_quest(quest_id) {
        succeed_quest(ctx, quest_id)?;
    }
    Ok(())
}

pub fn step_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    nodes: &[NodeEntry],
    node: &NodeEntry,
    push_updates: bool,
    steps: &mut usize,
) -> io::Result<StepResult> {
    if *steps >= MAX_STEPS_PER_ADVANCE {
        return Ok(StepResult::Running);
    }
    *steps += 1;

    let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id) else {
        return Ok(StepResult::QuestEnded);
    };
    if aq.is_node_finished(node.id) {
        return Ok(StepResult::Completed);
    }

    if let Some(suspend_res) =
        check_node_occupation_and_save(ctx, BtType::BtTypeQuest, quest_id, node, push_updates)?
    {
        return Ok(suspend_res);
    }

    match node.node_type.as_str() {
        "Sequence" => {
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                let child_res = step_node(ctx, quest_id, nodes, child, push_updates, steps)?;
                match child_res {
                    StepResult::Running | StepResult::Failed | StepResult::QuestEnded => {
                        return Ok(child_res);
                    }
                    StepResult::Completed => continue,
                }
            }
            finish_non_child_node(ctx, quest_id, node, push_updates)?;
            Ok(StepResult::Completed)
        }
        "Select" => {
            let mut any_child = false;
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                any_child = true;
                let child_res = step_node(ctx, quest_id, nodes, child, push_updates, steps)?;
                match child_res {
                    StepResult::QuestEnded | StepResult::Running => return Ok(child_res),
                    StepResult::Completed => {
                        finish_non_child_node(ctx, quest_id, node, push_updates)?;
                        return Ok(StepResult::Completed);
                    }
                    StepResult::Failed => continue,
                }
            }
            if any_child && node.parent_id != 0 {
                return Ok(StepResult::Failed);
            }
            finish_non_child_node(ctx, quest_id, node, push_updates)?;
            Ok(StepResult::Completed)
        }
        "ParallelSelect" | "Parallel" => {
            let is_parallel_all = node.node_type == "Parallel";
            let eligible_children = nodes
                .iter()
                .filter(|c| c.parent_id == node.id && c.node_type != "QuestFailed")
                .count() as i32;
            let needed_sel = if node.count > 0 {
                node.count
            } else {
                eligible_children.max(1)
            };
            let mut finished_count = 0i32;
            let mut failed_count = 0i32;
            let mut total_children = 0i32;
            let mut any_ended = false;

            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                total_children += 1;
                let Some(cur_aq) = ctx.quest_comp.info.find_active_quest(quest_id) else {
                    return Ok(StepResult::QuestEnded);
                };
                if cur_aq.is_node_finished(child.id) {
                    finished_count += 1;
                    continue;
                }
                let child_res = step_node(ctx, quest_id, nodes, child, push_updates, steps)?;
                match child_res {
                    StepResult::QuestEnded => {
                        any_ended = true;
                        break;
                    }
                    StepResult::Completed => {
                        finished_count += 1;
                        if !is_parallel_all && finished_count >= needed_sel {
                            break;
                        }
                    }
                    StepResult::Failed => failed_count += 1,
                    StepResult::Running => {}
                }
            }
            if any_ended || !ctx.quest_comp.info.has_active_quest(quest_id) {
                return Ok(StepResult::QuestEnded);
            }
            let needed = if is_parallel_all {
                total_children
            } else {
                needed_sel
            };
            if is_parallel_all && failed_count > 0 {
                return Ok(StepResult::Failed);
            }
            if finished_count >= needed {
                if !is_parallel_all {
                    deactivate_subtree(ctx, quest_id, nodes, node.id, push_updates)?;
                }
                finish_non_child_node(ctx, quest_id, node, push_updates)?;
                return Ok(StepResult::Completed);
            }
            if !is_parallel_all
                && node.parent_id != 0
                && total_children > 0
                && (total_children - failed_count) < needed
            {
                return Ok(StepResult::Failed);
            }
            Ok(StepResult::Running)
        }
        "AlwaysPending" | "AlwaysFalse" => {
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                if step_node(ctx, quest_id, nodes, child, push_updates, steps)?
                    == StepResult::QuestEnded
                {
                    return Ok(StepResult::QuestEnded);
                }
            }
            Ok(StepResult::Running)
        }
        "AlwaysTrue" => {
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                let res = step_node(ctx, quest_id, nodes, child, push_updates, steps)?;
                if res == StepResult::QuestEnded || res == StepResult::Running {
                    return Ok(res);
                }
            }
            finish_non_child_node(ctx, quest_id, node, push_updates)?;
            Ok(StepResult::Completed)
        }
        "QuestSucceed" => {
            if push_updates {
                ctx.sink.push(
                    UpdateNodeStatusNotify::MSG_ID,
                    &UpdateNodeStatusNotify {
                        TreeOwnerId: quest_id,
                        TreeIncId: i64::from(quest_id),
                        NodeId: node.id,
                        Status: Some(NodeStatus::CompletedSuccess),
                    },
                );
                if let Some(Value::Array(acts)) = node.data.get("FinishActions")
                    && !acts.is_empty()
                {
                    let _ = push_bt_level_events(
                        ctx,
                        quest_id,
                        node.id,
                        GameCtxType::SuccessNodeAction,
                        acts,
                    );
                }
            }
            run_node_finish_effects(ctx, quest_id, node)?;
            succeed_quest(ctx, quest_id)?;
            Ok(StepResult::QuestEnded)
        }
        "QuestFailed" => step_quest_failed_node(ctx, quest_id, node, push_updates),
        "Action" | "ActionWithResult" => {
            let mut single_buf = [Value::Null];
            let items: Vec<Value> =
                extract_node_action_items(&node.data, &mut single_buf).to_vec();
            if !items.is_empty() {
                let akey = action_node_progress_key(quest_id, node.id);
                let start_idx = ctx
                    .scene
                    .as_deref()
                    .and_then(|sc| sc.action_node_next_idx.get(&akey).copied())
                    .unwrap_or(0);
                if start_idx == 0 && push_updates {
                    ctx.sink.push(
                        UpdateNodeStatusNotify::MSG_ID,
                        &UpdateNodeStatusNotify {
                            TreeOwnerId: quest_id,
                            TreeIncId: i64::from(quest_id),
                            NodeId: node.id,
                            Status: Some(NodeStatus::Activate),
                        },
                    );
                    push_bt_level_events(
                        ctx,
                        quest_id,
                        node.id,
                        GameCtxType::ActionGroupNodeAction,
                        &items,
                    )?;
                }
                if start_idx < items.len() {
                    for (idx, act_item) in items.iter().enumerate().skip(start_idx) {
                        execute_actions(ctx, quest_id, node.id, std::slice::from_ref(act_item))?;
                        if let Some(scene) = ctx.scene.as_deref_mut()
                            && is_sync_play_flow_action(act_item)
                            && ((scene.active_flow_inc_id != 0
                                && scene.active_flow_quest_id == quest_id
                                && scene.active_flow_node_id == node.id)
                                || scene.has_pending_flow_for_node(quest_id, node.id))
                        {
                            scene.action_node_next_idx.insert(akey, idx + 1);
                            return Ok(StepResult::Running);
                        }
                    }
                }
                if let Some(scene) = ctx.scene.as_deref_mut() {
                    if (scene.active_flow_inc_id != 0
                        && scene.active_flow_quest_id == quest_id
                        && scene.active_flow_node_id == node.id)
                        || scene.has_pending_flow_for_node(quest_id, node.id)
                    {
                        scene.action_node_next_idx.insert(akey, items.len());
                        return Ok(StepResult::Running);
                    }
                    scene.action_node_next_idx.remove(&akey);
                }
            }
            finish_non_child_node(ctx, quest_id, node, push_updates)?;
            Ok(StepResult::Completed)
        }
        "Condition" => {
            let wait_true = node
                .data
                .get("WaitUntilTrue")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let ok = if let Some(cb) = node.data.get("Condition") {
                condition::eval_inline_condition_block(
                    &ctx.make_eval_context_for_quest(quest_id),
                    cb,
                )
            } else {
                true
            };
            if ok {
                finish_non_child_node(ctx, quest_id, node, push_updates)?;
                return Ok(StepResult::Completed);
            }
            Ok(if wait_true {
                StepResult::Running
            } else {
                StepResult::Failed
            })
        }
        "ConditionSelector" => {
            if let Some(Value::Array(slots)) = node.data.get("Slots") {
                for slot in slots {
                    let Value::Object(sobj) = slot else {
                        continue;
                    };
                    let pass = sobj.get("Condition").is_none_or(|sc| {
                        condition::eval_inline_condition_block(
                            &ctx.make_eval_context_for_quest(quest_id),
                            sc,
                        )
                    });
                    if !pass {
                        continue;
                    }
                    let Some(Value::Object(sn)) = sobj.get("Node") else {
                        continue;
                    };
                    let Some(cid) = json_int(sn.get("Id")) else {
                        continue;
                    };
                    let Some(cnode) = find_node_by_id(nodes, cid) else {
                        continue;
                    };
                    let res = step_node(ctx, quest_id, nodes, cnode, push_updates, steps)?;
                    match res {
                        StepResult::Completed => {
                            finish_non_child_node(ctx, quest_id, node, push_updates)?;
                            return Ok(StepResult::Completed);
                        }
                        StepResult::Running | StepResult::QuestEnded => return Ok(res),
                        StepResult::Failed => continue,
                    }
                }
            }
            Ok(StepResult::Running)
        }
        "Repeater" => {
            let cd_key = repeater_cooldown_key(-quest_id, node.id);
            while *steps < MAX_STEPS_PER_ADVANCE {
                if let Some(ec) = node.data.get("ExitCondition")
                    && condition::eval_inline_condition_block(
                        &ctx.make_eval_context_for_quest(quest_id),
                        ec,
                    )
                {
                    deactivate_subtree(ctx, quest_id, nodes, node.id, push_updates)?;
                    finish_non_child_node(ctx, quest_id, node, push_updates)?;
                    return Ok(StepResult::Completed);
                }
                if let Some(scene) = ctx.scene.as_deref()
                    && let Some(&ready_ms) = scene.lp_repeater_cooldowns.get(&cd_key)
                    && now_millis() < ready_ms
                {
                    return Ok(StepResult::Running);
                }
                let mut any_child = false;
                let mut child_completed = false;
                for child in nodes {
                    if child.parent_id != node.id {
                        continue;
                    }
                    any_child = true;
                    let res = step_node(ctx, quest_id, nodes, child, push_updates, steps)?;
                    if res == StepResult::QuestEnded {
                        return Ok(StepResult::QuestEnded);
                    }
                    if res == StepResult::Completed {
                        child_completed = true;
                        break;
                    }
                    return Ok(StepResult::Running);
                }
                if !any_child {
                    finish_non_child_node(ctx, quest_id, node, push_updates)?;
                    return Ok(StepResult::Completed);
                }
                if !child_completed {
                    return Ok(StepResult::Running);
                }
                reset_quest_subtree_finished_nodes(ctx, quest_id, nodes, node.id);
                let wait_ms = subtree_wait_delay_ms(nodes, node.id);
                if wait_ms > 0
                    && let Some(scene) = ctx.scene.as_deref_mut()
                {
                    scene
                        .lp_repeater_cooldowns
                        .insert(cd_key, now_millis() + wait_ms);
                    return Ok(StepResult::Running);
                }
            }
            Ok(StepResult::Running)
        }
        "ChildQuest" => step_child_quest(ctx, quest_id, nodes, node, push_updates),
        _ => {
            finish_non_child_node(ctx, quest_id, node, push_updates)?;
            Ok(StepResult::Completed)
        }
    }
}

fn step_child_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    nodes: &[NodeEntry],
    node: &NodeEntry,
    push_updates: bool,
) -> io::Result<StepResult> {
    let Some(aq_ptr) = ctx.quest_comp.info.find_active_quest_mut(quest_id) else {
        return Ok(StepResult::QuestEnded);
    };
    let newly_entered = aq_ptr.mark_node_active(node.id);

    if newly_entered {
        if push_updates {
            push_ancestor_activations(ctx, quest_id, nodes, node.parent_id);
            push_node_status(
                ctx,
                quest_id,
                node.id,
                NodeStatus::Activate,
                ChildQuestNodeStatus::Progress,
            )?;
        }
        if let Some(Value::Array(acts)) = node.data.get("EnterActions") {
            execute_actions(ctx, quest_id, node.id, acts)?;
        }
        on_enter_child_quest_special(ctx, quest_id, node)?;
    }

    if should_auto_complete_child_quest(ctx, quest_id, node) {
        complete_child_quest_without_advance(ctx, quest_id, node, push_updates)?;
        return Ok(StepResult::Completed);
    }
    Ok(StepResult::Running)
}

pub fn ensure_active_quest_timers_scheduled<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let now_ms = now_millis();
    let active_snap = ctx.quest_comp.info.active_quests.clone();
    if let Some(scene) = ctx.scene.as_deref_mut() {
        for aq in &active_snap {
            let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
                continue;
            };
            for &nid in &aq.active_nodes {
                if scene.quest_timers.contains(aq.quest_id, nid) {
                    continue;
                }
                let Some(node) = find_node_by_id(&nodes, nid) else {
                    continue;
                };
                if node.node_type != "ChildQuest" {
                    continue;
                }
                let Some(cond) = get_child_condition(&node.data) else {
                    continue;
                };
                if let Some(delay_ms) = parse_timer_delay_ms(cond)
                    && delay_ms > 0
                {
                    scene.quest_timers.upsert(QuestTimerEntry {
                        quest_id: aq.quest_id,
                        node_id: nid,
                        due_ms: now_ms + delay_ms,
                    });
                }
            }
        }
    }
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        level_play::ensure_active_timers_scheduled(&mut lp_ctx)?;
    }
    Ok(())
}

pub fn on_timers_due<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    now_ms: i64,
) -> io::Result<()> {
    if ctx.scene.is_none() {
        return Ok(());
    }
    let mut progressed = false;

    loop {
        let due = ctx
            .scene
            .as_deref_mut()
            .and_then(|s| s.quest_timers.pop_due(now_ms));
        let Some(due) = due else {
            break;
        };
        let Some(aq) = ctx.quest_comp.info.find_active_quest(due.quest_id) else {
            continue;
        };
        if !aq.is_node_active(due.node_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(due.quest_id) else {
            continue;
        };
        let Some(node) = find_node_by_id(&nodes, due.node_id).cloned() else {
            continue;
        };
        complete_child_quest(ctx, due.quest_id, &nodes, &node)?;
        progressed = true;
    }

    loop {
        let due = ctx
            .scene
            .as_deref_mut()
            .and_then(|s| s.lp_timers.pop_due(now_ms));
        let Some(due) = due else {
            break;
        };
        if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
            && level_play::on_timer_due(&mut lp_ctx, due.quest_id, due.node_id)?
        {
            progressed = true;
        }
    }

    loop {
        let due = ctx
            .scene
            .as_deref_mut()
            .and_then(|s| s.fail_timers.pop_due(now_ms));
        let Some(due) = due else {
            break;
        };
        if ctx.quest_comp.info.has_active_quest(due.quest_id) {
            rollback_quest_to_node(ctx, due.quest_id, due.node_id, FailReason::TimeoutFail, false)?;
            progressed = true;
        }
    }

    loop {
        let due = ctx
            .scene
            .as_deref_mut()
            .and_then(|s| s.lp_fail_timers.pop_due(now_ms));
        let Some(due) = due else {
            break;
        };
        if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
            && level_play::on_fail_timer_due(&mut lp_ctx, due.quest_id, due.node_id)?
        {
            progressed = true;
        }
    }

    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

fn on_enter_child_quest_special<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
) -> io::Result<()> {
    if !should_auto_complete_child_quest(ctx, quest_id, node) {
        spawn_node_target_entities(ctx, quest_id, node);
        if let Some(scene) = ctx.scene.as_deref_mut() {
            push_do_interact_options_for_node(scene, ctx.sink, quest_id, node, None)?;
        }
    }

    let Some(cond) = get_child_condition(&node.data) else {
        return Ok(());
    };
    let Some(ctype) = json_str(cond.get("Type")) else {
        return Ok(());
    };

    if let Some(delay_ms) = parse_timer_delay_ms(cond) {
        if delay_ms > 0
            && let Some(scene) = ctx.scene.as_deref_mut()
            && !scene.quest_timers.contains(quest_id, node.id)
        {
            scene.quest_timers.upsert(QuestTimerEntry {
                quest_id,
                node_id: node.id,
                due_ms: now_millis() + delay_ms,
            });
        }
        return Ok(());
    }

    if ctype == "PlayFlow"
        && !(quest_id == PROLOGUE_QUEST_ID && node.id == 19)
        && let Some(Value::Object(flow_val)) = cond.get("Flow")
    {
        start_flow_from_obj(ctx, quest_id, node.id, flow_val)?;
    } else if ctype == "ProgramSpecialProcess"
        && let Some(Value::Object(sp)) = cond.get("SpecialProcess")
        && json_str(sp.get("Type")) == Some("LowMemorySceneSwitch")
    {
        let mut img_path = String::new();
        if let Some(Value::Array(ops)) = sp.get("Operations") {
            for op in ops {
                if let Some(obj) = op.as_object()
                    && json_str(obj.get("Type")) == Some("AssignLoadingBackground")
                    && let Some(p) = json_str(obj.get("ImagePath"))
                {
                    img_path = p.to_string();
                    break;
                }
            }
        }
        ctx.sink.push(
            LowMemoryChangeSceneStartNotify::MSG_ID,
            &LowMemoryChangeSceneStartNotify {
                IsSwitchHLOD: false,
                AssignLoadingBackgroundImagePath: img_path.into_bytes(),
            },
        );
    }
    Ok(())
}

pub fn complete_child_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    _nodes: &[NodeEntry],
    node: &NodeEntry,
) -> io::Result<()> {
    complete_child_quest_without_advance(ctx, quest_id, node, true)?;
    advance_quest_tree(ctx, quest_id, true)
}

pub fn complete_child_quest_without_advance<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
    push_updates: bool,
) -> io::Result<()> {
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.quest_timers.cancel(quest_id, node.id);
    }
    remove_do_interact_options_for_node(ctx, quest_id, node.id);
    if !ctx.quest_comp.info.mark_node_finished(quest_id, node.id) {
        return Ok(());
    }
    if push_updates {
        push_node_status(
            ctx,
            quest_id,
            node.id,
            NodeStatus::CompletedSuccess,
            ChildQuestNodeStatus::Finished,
        )?;
    }
    run_node_finish_effects(ctx, quest_id, node)
}

pub fn finish_non_child_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
    push_updates: bool,
) -> io::Result<()> {
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.release_occupations_for_node(quest_id, node.id);
    }
    if node.reward_id > 0 {
        let _ = reward::grant_drop_package(&mut ctx.make_reward_context(), node.reward_id, 6003)?;
    }
    if !ctx.quest_comp.info.has_active_quest(quest_id) {
        return Ok(());
    }
    ctx.quest_comp.info.mark_node_finished(quest_id, node.id);

    if push_updates {
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node.id,
                Status: Some(NodeStatus::CompletedSuccess),
            },
        );
    }
    if let Some(Value::Object(occ)) = node.data.get("OccupationConfig")
        && let Some(Value::Array(exit_acts)) = occ.get("ExitActions")
        && !exit_acts.is_empty()
    {
        execute_actions(ctx, quest_id, node.id, exit_acts)?;
    }
    run_node_finish_effects(ctx, quest_id, node)
}

pub fn run_node_finish_effects<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
) -> io::Result<()> {
    if let Some(Value::Array(acts)) = node.data.get("FinishActions") {
        execute_actions(ctx, quest_id, node.id, acts)?;
    }
    Ok(())
}

pub fn check_node_occupation_and_save_pub<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    bt_type: BtType,
    owner_id: i32,
    node: &NodeEntry,
    push_updates: bool,
) -> io::Result<bool> {
    Ok(check_node_occupation_and_save(ctx, bt_type, owner_id, node, push_updates)?.is_some())
}

pub fn eval_quest_failed_condition_pub<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    node: &NodeEntry,
    player_pos_opt: Option<[f32; 3]>,
    fail_queue: &mut QuestTimerQueue,
) -> io::Result<Option<FailReason>> {
    eval_quest_failed_condition(ctx, owner_id, node, player_pos_opt, fail_queue)
}

fn check_node_occupation_and_save<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    bt_type: BtType,
    quest_id: i32,
    node: &NodeEntry,
    push_updates: bool,
) -> io::Result<Option<StepResult>> {
    if ctx.scene.is_none() || !node.data.is_object() {
        return Ok(None);
    }

    if let Some(Value::Object(occ)) = node.data.get("OccupationConfig") {
        let reclaim = json_bool(occ.get("ReclaimOccupationWhenEnter")).unwrap_or(false);
        if let Some(Value::Array(occ_list)) = occ.get("Occupations")
            && !occ_list.is_empty()
            && let Some(scene) = ctx.scene.as_deref_mut()
        {
            let mut conflicts = Vec::new();
            for item in occ_list {
                let Some(res_name) = json_str(Some(item)) else {
                    continue;
                };
                if !reclaim
                    && let Some(conf) =
                        scene.find_occupation_conflict(res_name, quest_id, i64::from(quest_id))
                {
                    conflicts.push(OccupationPbInfo {
                        ResourceName: conf.resource_name.as_bytes().to_vec(),
                        NodeId: conf.node_id,
                        IncId: conf.inc_id,
                    });
                }
            }
            if !conflicts.is_empty() {
                if push_updates {
                    ctx.sink.push(
                        BtSuspendNotify::MSG_ID,
                        &BtSuspendNotify {
                            NodeId: node.id,
                            SuspendType: SuspendType::Occupation.to_i32(),
                            OccupationInfo: conflicts,
                            TreeOwnerId: quest_id,
                            TreeIncId: i64::from(quest_id),
                            IsScreenOccupy: false,
                        },
                    );
                }
                return Ok(Some(StepResult::Running));
            }
            for item in occ_list {
                let Some(res_name) = json_str(Some(item)) else {
                    continue;
                };
                scene.upsert_occupation(
                    bt_type,
                    quest_id,
                    i64::from(quest_id),
                    node.id,
                    res_name,
                );
            }
        }
    }

    if let Some(Value::Object(save_val)) = node.data.get("SaveConfig") {
        let newly_entered = ctx
            .scene
            .as_deref_mut()
            .is_some_and(|scene| scene.mark_save_node_entered(quest_id, node.id));
        if newly_entered {
            if let Some(Value::Array(init_acts)) = save_val.get("InitConditionActions")
                && !init_acts.is_empty()
            {
                let mut cbs = super::action_callbacks::<S>();
                cbs.bt_type = bt_type;
                quest_actions::execute_action_list(ctx, &mut cbs, quest_id, node.id, init_acts)?;
            }
            if let Some(Value::Array(enter_acts)) = save_val.get("EnterActions")
                && !enter_acts.is_empty()
            {
                let mut cbs = super::action_callbacks::<S>();
                cbs.bt_type = bt_type;
                quest_actions::execute_action_list(ctx, &mut cbs, quest_id, node.id, enter_acts)?;
            }
        }
    }

    Ok(None)
}

fn eval_quest_failed_condition<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
    player_pos_opt: Option<[f32; 3]>,
    fail_queue: &mut QuestTimerQueue,
) -> io::Result<Option<FailReason>> {
    let Some(Value::Object(fc)) = node.data.get("FailedCondition") else {
        return Ok(None);
    };

    if let Some(Value::Object(tval)) = fc.get("Timer") {
        let secs = json_float(tval.get("Time")).unwrap_or(0.0);
        if secs.is_finite() && secs > 0.0 && !fail_queue.contains(quest_id, node.id) {
            let delay_ms = (f64::from(secs) * 1000.0).round() as i64;
            fail_queue.upsert(QuestTimerEntry {
                quest_id,
                node_id: node.id,
                due_ms: now_millis() + delay_ms,
            });
        }
    }

    if let Some(Value::Object(rl_val)) = fc.get("RangeLimiting") {
        let pt_obj = rl_val.get("Point").and_then(|p| p.as_object());
        let range = json_float(rl_val.get("Range")).unwrap_or(0.0);
        if let Some(pt) = pt_obj
            && range > 0.0
        {
            let px = json_float(pt.get("X")).unwrap_or(0.0);
            let py = json_float(pt.get("Y")).unwrap_or(0.0);
            let pz = json_float(pt.get("Z")).unwrap_or(0.0);
            if px != 0.0 || py != 0.0 || pz != 0.0 {
                let ppos_opt = player_pos_opt.or_else(|| {
                    condition::current_player_pos_3d(&ctx.make_eval_context_for_quest(quest_id))
                });
                if let Some(ppos) = ppos_opt
                    && let Some(scene) = ctx.scene.as_deref_mut()
                {
                    let dx = ppos[0] - px;
                    let dy = ppos[1] - py;
                    let dz = ppos[2] - pz;
                    let inside = (dx * dx + dy * dy + dz * dz) <= (range * range);
                    let entered_marker_id = node.id + 1_000_000_000;
                    if inside {
                        scene.mark_save_node_entered(quest_id, entered_marker_id);
                        if rl_val.get("OverRangeCountdown").is_some() {
                            fail_queue.cancel(quest_id, node.id);
                        }
                    } else if scene.has_save_node_entered(quest_id, entered_marker_id) {
                        let countdown = json_float(rl_val.get("OverRangeCountdown")).unwrap_or(0.0);
                        if countdown > 0.0 {
                            if !fail_queue.contains(quest_id, node.id) {
                                let delay_ms = (f64::from(countdown) * 1000.0).round() as i64;
                                fail_queue.upsert(QuestTimerEntry {
                                    quest_id,
                                    node_id: node.id,
                                    due_ms: now_millis() + delay_ms,
                                });
                            }
                        } else {
                            return Ok(Some(FailReason::OutRangeFail));
                        }
                    }
                }
            }
        }
    }

    if let Some(Value::Object(cv_val)) = fc.get("CompareVar")
        && eval_compare_var_list(ctx, cv_val)
    {
        return Ok(Some(FailReason::CompareVarFail));
    }

    if let Some(Value::Object(edl_val)) = fc.get("EntityDistanceLimiting") {
        let target_eid = json_int(edl_val.get("EntityId")).unwrap_or(0);
        let max_dist = json_float(edl_val.get("MaxDistance")).unwrap_or(0.0);
        if target_eid > 0 && max_dist > 0.0 {
            let ppos_opt = player_pos_opt.or_else(|| {
                condition::current_player_pos_3d(&ctx.make_eval_context_for_quest(quest_id))
            });
            let epos_opt = resolve_scene_or_asset_entity_pos_3d(ctx, target_eid);
            if let (Some(ppos), Some(epos)) = (ppos_opt, epos_opt)
                && let Some(scene) = ctx.scene.as_deref_mut()
            {
                let dx = ppos[0] - epos[0];
                let dy = ppos[1] - epos[1];
                let dz = ppos[2] - epos[2];
                let inside = (dx * dx + dy * dy + dz * dz) <= (max_dist * max_dist);
                let entered_marker_id = node.id + 1_500_000_000;
                if inside {
                    scene.mark_save_node_entered(quest_id, entered_marker_id);
                    if edl_val.get("OverMaxCountdown").is_some() {
                        fail_queue.cancel(quest_id, node.id);
                    }
                } else if scene.has_save_node_entered(quest_id, entered_marker_id) {
                    let countdown = json_float(edl_val.get("OverMaxCountdown")).unwrap_or(0.0);
                    if countdown > 0.0 {
                        if !fail_queue.contains(quest_id, node.id) {
                            let delay_ms = (f64::from(countdown) * 1000.0).round() as i64;
                            fail_queue.upsert(QuestTimerEntry {
                                quest_id,
                                node_id: node.id,
                                due_ms: now_millis() + delay_ms,
                            });
                        }
                    } else {
                        return Ok(Some(FailReason::OutRangeFail));
                    }
                }
            }
        }
    }

    if let Some(Value::Object(edc_val)) = fc.get("EntityDeathCondition")
        && let Some(Value::Array(eids_val)) = edc_val.get("EntityIds")
    {
        let aq_opt = ctx.quest_comp.info.find_active_quest(quest_id);
        for v in eids_val {
            let Some(eid) = json_int(Some(v)) else {
                continue;
            };
            if eid <= 0 {
                continue;
            }
            if aq_opt.is_some_and(|aq| aq.has_killed_entity(eid)) {
                return Ok(Some(FailReason::EntityDieFail));
            }
            if let Some(hp_pct) = entity_hp_percent(ctx, eid)
                && hp_pct <= 0.0
            {
                return Ok(Some(FailReason::EntityDieFail));
            }
        }
    }

    if let Some(Value::Object(esc_val)) = fc.get("EntityStateCondition")
        && let Some(Value::Array(conds_val)) = esc_val.get("Conditions")
    {
        for item in conds_val {
            let Value::Object(obj) = item else {
                continue;
            };
            let Some(eid) = json_int(obj.get("EntityId")) else {
                continue;
            };
            let Some(want_state) = json_str(obj.get("State")) else {
                continue;
            };
            if eid > 0 && entity_has_state_tag(ctx, eid, want_state) {
                return Ok(Some(FailReason::EntityStateFail));
            }
        }
    }

    if let Some(Value::Object(dl_val)) = fc.get("CheckDataLayer") {
        let dl_id = json_int(dl_val.get("DataLayerId")).unwrap_or(0);
        if dl_id > 0 {
            let want_load = json_bool(dl_val.get("IsLoad")).unwrap_or(true);
            let is_loaded = ctx.quest_comp.info.world.loaded_datalayers.contains(&dl_id)
                && !ctx.quest_comp.info.world.unloaded_datalayers.contains(&dl_id);
            if is_loaded == want_load {
                return Ok(Some(FailReason::DataLayerFail));
            }
        }
    }

    if let Some(Value::Object(tr_val)) = fc.get("TimeRange") {
        let st_obj = tr_val.get("StartTime").and_then(|v| v.as_object());
        let et_obj = tr_val.get("EndTime").and_then(|v| v.as_object());
        if let (Some(st), Some(et)) = (st_obj, et_obj) {
            let start_m =
                json_int(st.get("Hours")).unwrap_or(0) * 60 + json_int(st.get("Minutes")).unwrap_or(0);
            let end_m =
                json_int(et.get("Hours")).unwrap_or(0) * 60 + json_int(et.get("Minutes")).unwrap_or(0);
            if start_m != end_m {
                let ch = if ctx.quest_comp.info.env.time_locked {
                    ctx.quest_comp.info.env.time_lock_hour
                } else if let Some(s) = ctx.scene.as_deref() {
                    s.instance.map_time.hour
                } else {
                    ctx.quest_comp.info.env.time_lock_hour
                };
                let cm = if ctx.quest_comp.info.env.time_locked {
                    ctx.quest_comp.info.env.time_lock_minute
                } else if let Some(s) = ctx.scene.as_deref() {
                    s.instance.map_time.minute
                } else {
                    ctx.quest_comp.info.env.time_lock_minute
                };
                let cur_m = ch * 60 + cm;
                let in_window = if start_m < end_m {
                    cur_m >= start_m && cur_m <= end_m
                } else {
                    cur_m >= start_m || cur_m <= end_m
                };
                if !in_window {
                    return Ok(Some(FailReason::TimeRange));
                }
            }
        }
    }

    if let Some(Value::Object(pm_val)) = fc.get("PlayerMotionStateCondition")
        && ctx.scene.as_deref().is_some_and(|s| s.has_player_motion)
    {
        let motion = json_str(pm_val.get("MotionState")).unwrap_or("");
        let cmp = json_str(pm_val.get("Compare")).unwrap_or("Eq");
        if let Some(matched) = condition::eval_player_motion_state(
            &ctx.make_eval_context_for_quest(quest_id),
            motion,
        ) {
            let fail = if cmp == "Ne" || cmp == "NotEq" {
                !matched
            } else {
                matched
            };
            if fail {
                return Ok(Some(FailReason::PlayerMotionState));
            }
        }
    }

    if let Some(Value::Object(pmm_val)) = fc.get("PlayerMovementModeCondition")
        && let Some(scene) = ctx.scene.as_deref()
        && scene.has_player_movement_mode
    {
        let mode = json_str(pmm_val.get("Mode")).unwrap_or("");
        let cmp = json_str(pmm_val.get("Compare")).unwrap_or("Eq");
        let in_mode = match mode {
            "WingSuitFly" => scene.player_movement_mode == 5 || scene.player_movement_mode >= 6,
            "Rail" | "Ski" => scene.player_movement_mode >= 6,
            _ => false,
        };
        let fail = if cmp == "Ne" || cmp == "NotEq" {
            !in_mode
        } else {
            in_mode
        };
        if fail {
            return Ok(Some(FailReason::PlayerLevelPlayMoveStateFail));
        }
    }

    if (json_bool(fc.get("IsTeamDeathFailure")).unwrap_or(false)
        || json_bool(fc.get("IsNonAiTeamDeathFailure")).unwrap_or(false))
        && let Some(scene) = ctx.scene.as_deref()
        && let Ok(fi) = usize::try_from(scene.formation_info.cur_formation)
        && let Some(form) = scene.formation_info.formations.get(fi)
        && scene.current_role_entity_id() > 0
        && role::formation_living_status(scene, form)
            == sonoro_proto::enums::LivingStatus::Dead
    {
        return Ok(Some(FailReason::CharacterDieFail));
    }

    if let Some(Value::Array(cdf_val)) = fc.get("CheckDungeonFailure")
        && let Some(scene) = ctx.scene.as_deref()
    {
        for v in cdf_val {
            if let Some(did) = json_int(Some(v))
                && did > 0
                && scene.has_failed_dungeon(did)
            {
                return Ok(Some(FailReason::InstFail));
            }
        }
    }

    if let Some(Value::Object(ea_val)) = fc.get("EntityAlert")
        && let Some(Value::Array(eids_val)) = ea_val.get("EntityIds")
        && let Some(scene) = ctx.scene.as_deref()
    {
        for v in eids_val {
            let Some(target_cfg) = json_int(Some(v)) else {
                continue;
            };
            if target_cfg <= 0 {
                continue;
            }
            for s in &scene.entities {
                if s.config.config_id == target_cfg
                    && scene.fsm_hate_states.get(&s.net_id).copied().unwrap_or(false)
                {
                    return Ok(Some(FailReason::DefaultFail));
                }
            }
        }
    }

    if let Some(Value::Object(sp_val)) = fc.get("SneakPlayCondition")
        && let Some(scene) = ctx.scene.as_deref()
        && scene.active_sneak_id > 0
    {
        let secs = json_float(sp_val.get("Time")).unwrap_or(0.0);
        if secs.is_finite() && secs > 0.0 && !fail_queue.contains(quest_id, node.id) {
            let delay_ms = (f64::from(secs) * 1000.0).round() as i64;
            fail_queue.upsert(QuestTimerEntry {
                quest_id,
                node_id: node.id,
                due_ms: now_millis() + delay_ms,
            });
        }
    }

    Ok(None)
}

fn resolve_scene_or_asset_entity_pos_3d<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cfg_id: i32,
) -> Option<[f32; 3]> {
    if cfg_id <= 0 {
        return None;
    }
    let scene = ctx.scene.as_deref()?;
    for s in &scene.entities {
        if s.config.config_id == cfg_id
            && let Some(p) = s.position
        {
            return Some(p.location);
        }
    }
    ctx.assets
        .find_level_entity(scene.instance_id, i64::from(cfg_id))
        .and_then(|lcfg| entity_world_pos(&lcfg))
}

fn extend_fail_timers_on_kill_in_queue(
    timers: &mut QuestTimerQueue,
    store: &sonoro_gamesv_data::lazy_node_store::LazyNodeStore,
) -> bool {
    let mut extended = false;
    let items: Vec<QuestTimerEntry> = timers.entries().to_vec();
    for entry in items {
        let Ok(nodes) = store.get_owner_nodes(entry.quest_id) else {
            continue;
        };
        let Some(node) = find_node_by_id(&nodes, entry.node_id) else {
            continue;
        };
        let Some(add_secs) = node
            .data
            .get("FailedCondition")
            .and_then(|v| v.get("AddTimeConfig"))
            .and_then(|v| v.get("MonsterConfig"))
            .and_then(|mc| json_float(mc.get("NormalAddTime")))
        else {
            continue;
        };
        if add_secs <= 0.0 {
            continue;
        }
        let add_ms = (f64::from(add_secs) * 1000.0).round() as i64;
        timers.upsert(QuestTimerEntry {
            quest_id: entry.quest_id,
            node_id: entry.node_id,
            due_ms: entry.due_ms + add_ms,
        });
        extended = true;
    }
    extended
}

pub fn extend_fail_timers_on_kill<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> bool {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return false;
    };
    let a = extend_fail_timers_on_kill_in_queue(&mut scene.fail_timers, &ctx.assets.quest_nodes);
    let b = extend_fail_timers_on_kill_in_queue(
        &mut scene.lp_fail_timers,
        &ctx.assets.level_play_nodes,
    );
    a || b
}

fn extend_fail_timers_on_entity_state_in_queue(
    timers: &mut QuestTimerQueue,
    store: &sonoro_gamesv_data::lazy_node_store::LazyNodeStore,
    entity_config_id: i32,
    state_name: &str,
) -> bool {
    let mut extended = false;
    let items: Vec<QuestTimerEntry> = timers.entries().to_vec();
    for entry in items {
        let Ok(nodes) = store.get_owner_nodes(entry.quest_id) else {
            continue;
        };
        let Some(node) = find_node_by_id(&nodes, entry.node_id) else {
            continue;
        };
        let Some(Value::Array(sc_list)) = node
            .data
            .get("FailedCondition")
            .and_then(|v| v.get("AddTimeConfig"))
            .and_then(|v| v.get("StateConfig"))
        else {
            continue;
        };
        let mut add_secs = 0.0f32;
        for sc_item in sc_list {
            let Some(Value::Object(cond)) = sc_item.get("Condition") else {
                continue;
            };
            let eid = json_int(cond.get("EntityId")).unwrap_or(0);
            let want_st = json_str(cond.get("State")).unwrap_or("");
            if (eid == 0 || eid == entity_config_id)
                && (want_st.is_empty() || want_st == state_name)
            {
                add_secs += json_float(sc_item.get("AddTime")).unwrap_or(0.0);
            }
        }
        if add_secs <= 0.0 {
            continue;
        }
        let add_ms = (f64::from(add_secs) * 1000.0).round() as i64;
        timers.upsert(QuestTimerEntry {
            quest_id: entry.quest_id,
            node_id: entry.node_id,
            due_ms: entry.due_ms + add_ms,
        });
        extended = true;
    }
    extended
}

pub fn extend_fail_timers_on_entity_state<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_config_id: i32,
    state_name: &str,
) -> bool {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return false;
    };
    let a = extend_fail_timers_on_entity_state_in_queue(
        &mut scene.fail_timers,
        &ctx.assets.quest_nodes,
        entity_config_id,
        state_name,
    );
    let b = extend_fail_timers_on_entity_state_in_queue(
        &mut scene.lp_fail_timers,
        &ctx.assets.level_play_nodes,
        entity_config_id,
        state_name,
    );
    a || b
}

fn step_quest_failed_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
    push_updates: bool,
) -> io::Result<StepResult> {
    if ctx.scene.is_none() {
        return Ok(StepResult::Running);
    }
    let fail_reason = {
        let mut fail_timers = std::mem::take(&mut ctx.scene.as_deref_mut().expect("scene").fail_timers);
        let res = eval_quest_failed_condition(ctx, quest_id, node, None, &mut fail_timers);
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.fail_timers = fail_timers;
        }
        res?
    };
    if let Some(reason) = fail_reason {
        rollback_quest_to_node(ctx, quest_id, node.id, reason, !push_updates)?;
        return Ok(if ctx.quest_comp.info.has_active_quest(quest_id) {
            StepResult::Running
        } else {
            StepResult::QuestEnded
        });
    }
    Ok(StepResult::Running)
}

pub fn is_subtree_active_for_monitor(
    nodes: &[NodeEntry],
    active_nodes: &[i32],
    parent_id: i32,
) -> bool {
    if parent_id <= 0 {
        return true;
    }
    for &nid in active_nodes {
        let mut cur = nid;
        for _ in 0..32 {
            if cur <= 0 {
                break;
            }
            if cur == parent_id {
                return true;
            }
            let Some(n) = find_node_by_id(nodes, cur) else {
                break;
            };
            cur = n.parent_id;
        }
    }
    false
}

pub fn check_active_quest_failed_monitors<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    pos_opt: Option<[f32; 3]>,
) -> io::Result<bool> {
    if ctx.scene.is_none() {
        return Ok(false);
    }
    let aq_snapshot = ctx.quest_comp.info.active_quests.clone();
    let mut changed = false;
    for aq in aq_snapshot {
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
            continue;
        };
        for node in nodes.iter() {
            if node.node_type != "QuestFailed" {
                continue;
            }
            if aq.is_node_finished(node.id) {
                continue;
            }
            if !is_subtree_active_for_monitor(&nodes, &aq.active_nodes, node.parent_id) {
                continue;
            }
            let fail_reason = {
                let mut fail_timers =
                    std::mem::take(&mut ctx.scene.as_deref_mut().expect("scene").fail_timers);
                let res =
                    eval_quest_failed_condition(ctx, aq.quest_id, node, pos_opt, &mut fail_timers);
                if let Some(scene) = ctx.scene.as_deref_mut() {
                    scene.fail_timers = fail_timers;
                }
                res?
            };
            if let Some(reason) = fail_reason {
                rollback_quest_to_node(ctx, aq.quest_id, node.id, reason, false)?;
                changed = true;
                break;
            }
        }
    }
    Ok(changed)
}

pub fn rollback_quest_to_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    fail_node_id: i32,
    reason: FailReason,
    client_initiated: bool,
) -> io::Result<()> {
    if !ctx.quest_comp.info.has_active_quest(quest_id) {
        return Ok(());
    }
    let nodes = ctx
        .assets
        .quest_nodes
        .get_owner_nodes(quest_id)
        .unwrap_or_default();

    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.quest_timers.cancel_quest(quest_id);
        scene.fail_timers.cancel_quest(quest_id);
        scene.release_occupations_for_tree(quest_id, i64::from(quest_id));
        scene.clear_save_nodes_for_owner(quest_id);
        scene.clear_action_node_progress_for_owner(quest_id);
        if !client_initiated && reason != FailReason::GiveUpFail {
            scene.last_server_rollback_quest_id = quest_id;
            scene.last_server_rollback_ms = now_millis();
        }
        interrupt_active_flow_for_quest(ctx, quest_id);
        ctx.sink.push(
            OperationRestrictionNotify::MSG_ID,
            &OperationRestrictionNotify {
                IsNull: true,
                Params: Vec::new(),
            },
        );
    }

    let fail_node_opt = if fail_node_id > 0 {
        find_node_by_id(&nodes, fail_node_id).cloned()
    } else {
        None
    };
    let has_pos_rollback = fail_node_opt
        .as_ref()
        .and_then(|n| n.data.get("FailedCondition"))
        .and_then(|fc| fc.get("FailedTeleport"))
        .is_some();

    if !client_initiated {
        ctx.sink.push(
            BtRollbackStartNotify::MSG_ID,
            &BtRollbackStartNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                IsRollbackSubLevel: false,
                IsRollbackPos: has_pos_rollback,
            },
        );
        ctx.sink.push(
            BtRollbackNotify::MSG_ID,
            &BtRollbackNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                FailNodeId: fail_node_id,
                FailReason: Some(reason),
                BtRollbackInfo: Some(BtRollbackInfo {
                    FailRollbackData: Some(FailRollbackData {
                        FailNodeId: fail_node_id,
                        FailReason: Some(reason),
                    }),
                    ForceClosedRollbackData: None,
                }),
            },
        );
    }

    if let Some(ref fn_entry) = fail_node_opt {
        run_node_finish_effects(ctx, quest_id, fn_entry)?;
    }

    let (latest_ckpt_id, active_copy) = {
        let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id) else {
            return Ok(());
        };
        let mut latest_ckpt = 0;
        for &fid in aq.finished_nodes.iter().chain(aq.active_nodes.iter()) {
            if (fail_node_id <= 0 || fid <= fail_node_id)
                && let Some(fnode) = find_node_by_id(&nodes, fid)
                && fnode.data.get("SaveConfig").is_some()
                && fid > latest_ckpt
            {
                latest_ckpt = fid;
            }
        }
        (latest_ckpt, aq.active_nodes.clone())
    };

    for aid in active_copy {
        remove_do_interact_options_for_node(ctx, quest_id, aid);
        if let Some(aq_ptr) = ctx.quest_comp.info.find_active_quest_mut(quest_id) {
            aq_ptr.mark_node_inactive(aid);
        }
    }

    if let Some(aq_ptr) = ctx.quest_comp.info.find_active_quest_mut(quest_id) {
        aq_ptr.killed_entities.clear();
        if latest_ckpt_id > 0 {
            aq_ptr.finished_nodes.retain(|&fid| fid < latest_ckpt_id);
        } else {
            aq_ptr.finished_nodes.clear();
        }
    }

    if let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id).cloned() {
        let tree_info = build_tree_info(ctx, &aq);
        ctx.sink.push(
            BtRollbackInfoNotify::MSG_ID,
            &BtRollbackInfoNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                TreeInfo: Some(tree_info),
            },
        );
    }
    ctx.sink.push(
        QuestRollBackEndNotify::MSG_ID,
        &QuestRollBackEndNotify {
            TreeIncId: i64::from(quest_id),
            QuestId: quest_id,
        },
    );

    advance_quest_tree(ctx, quest_id, true)
}

pub fn succeed_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> io::Result<()> {
    let Some(_guard) = TransitionDepthGuard::try_enter() else {
        return Ok(());
    };
    if !ctx.quest_comp.info.has_active_quest(quest_id) {
        return Ok(());
    }
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.quest_timers.cancel_quest(quest_id);
        scene.fail_timers.cancel_quest(quest_id);
        scene.release_occupations_for_tree(quest_id, i64::from(quest_id));
        scene.clear_save_nodes_for_owner(quest_id);
        scene.clear_action_node_progress_for_owner(quest_id);
    }

    let quest_opt = ctx.assets.tables.quest_data.get_data_by_id(&quest_id);
    if let Some(ref q) = quest_opt
        && q.Data.RewardId > 0
    {
        let _ = reward::grant_drop_package(&mut ctx.make_reward_context(), q.Data.RewardId, 6003)?;
    }

    ctx.quest_comp.info.finish_quest(quest_id);

    if let Some(ref q) = quest_opt {
        execute_actions(ctx, quest_id, 0, &q.Data.FinishActions)?;
    }

    ctx.sink.push(
        QuestFinishListNotify::MSG_ID,
        &QuestFinishListNotify {
            QuestId: vec![quest_id],
        },
    );
    ctx.sink.push(
        QuestListNotify::MSG_ID,
        &QuestListNotify {
            Quests: vec![PbQuestInfo {
                QuestId: quest_id,
                Status: Some(QuestState::Finish),
            }],
        },
    );

    if ctx.quest_comp.info.daily.daily_quests.contains(&quest_id) {
        push_daily_and_branch_notifies(ctx);
    }

    let _ = refresh_available_quests(ctx, true)?;
    let _ = recheck_pending_conditions(ctx)?;
    if QUEST_TRANSITION_DEPTH.with(|d| d.get()) <= 1
        && REFRESH_LOOP_DEPTH.with(|d| d.get()) == 0
    {
        quest_actions::push_func_open(ctx);
    }
    Ok(())
}

fn reset_quest_subtree_finished_nodes<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    nodes: &[NodeEntry],
    parent_id: i32,
) {
    let Some(aq_ptr) = ctx.quest_comp.info.find_active_quest_mut(quest_id) else {
        return;
    };
    reset_subtree_recursive(aq_ptr, nodes, parent_id);
}

fn reset_subtree_recursive(aq_ptr: &mut ActiveQuest, nodes: &[NodeEntry], parent_id: i32) {
    for n in nodes {
        if n.parent_id != parent_id {
            continue;
        }
        remove_int_value(&mut aq_ptr.finished_nodes, n.id);
        aq_ptr.mark_node_inactive(n.id);
        reset_subtree_recursive(aq_ptr, nodes, n.id);
    }
}

pub fn deactivate_subtree<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    nodes: &[NodeEntry],
    parent_id: i32,
    push_updates: bool,
) -> io::Result<()> {
    if !ctx.quest_comp.info.has_active_quest(quest_id) {
        return Ok(());
    }
    for n in nodes {
        if n.parent_id != parent_id {
            continue;
        }
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.quest_timers.cancel(quest_id, n.id);
            scene.fail_timers.cancel(quest_id, n.id);
            scene.release_occupations_for_node(quest_id, n.id);
        }
        remove_do_interact_options_for_node(ctx, quest_id, n.id);
        let was_active = ctx
            .quest_comp
            .info
            .find_active_quest_mut(quest_id)
            .is_some_and(|aq| aq.mark_node_inactive(n.id));
        let is_child_quest = n.node_type == "ChildQuest";
        if push_updates {
            if was_active && is_child_quest {
                push_node_status(
                    ctx,
                    quest_id,
                    n.id,
                    NodeStatus::Destroy,
                    ChildQuestNodeStatus::NotActive,
                )?;
            } else if was_active || !is_child_quest {
                ctx.sink.push(
                    UpdateNodeStatusNotify::MSG_ID,
                    &UpdateNodeStatusNotify {
                        TreeOwnerId: quest_id,
                        TreeIncId: i64::from(quest_id),
                        NodeId: n.id,
                        Status: Some(NodeStatus::Destroy),
                    },
                );
            }
        }
        deactivate_subtree(ctx, quest_id, nodes, n.id, push_updates)?;
    }
    Ok(())
}

pub fn finalize_progression<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) -> io::Result<()> {
    if FINALIZING_DEPTH.with(|d| d.get()) == 0 {
        let _guard = FinalizingGuard::enter();
        let _ = step_pending_control_nodes(ctx);
    }
    ensure_tracked_focus_valid(ctx);
    let tracked = ctx.quest_comp.info.tracked_quest_id;
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.current_quest_id = tracked;
    }
    sync_scene_current_node(ctx);

    ctx.sink
        .push(TraceQuestNotify::MSG_ID, &TraceQuestNotify { QuestId: tracked });
    ctx.quest_comp.save(ctx.fs)
}
