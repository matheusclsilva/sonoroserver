use std::cell::Cell;
use std::collections::BTreeMap;
use std::io;

use serde_json::{Map, Value};
use sonoro_gamesv_data::asset_indexes::MapEntityKey;
use sonoro_gamesv_fs::quest_info::append_unique_int;
use sonoro_proto::enums::{BtType, ChildQuestNodeStatus, ErrorCode, FailReason, TimerSetType};
use sonoro_proto::pb::{
    BtRollbackStartNotify, FlowEndNotify, FlowOptionInfoList, LordGymLevelPlayResultNotify,
    QuestGiveUpNotify, TowerEndNotify, TowerFloorPb, TowerFloorUpdateNotify,
    TowerRolePb, UpdateChildQuestNodeStatusNotify, UpdateTimerInfoNotify,
};

use crate::helpers::condition;
use crate::helpers::level_play::{self, LevelPlayContext};
use crate::helpers::morale::{self, DEFAULT_KEEP_ITEM_ID, DEFAULT_TEMP_ITEM_ID};
use crate::helpers::prologue_quest;
use crate::helpers::quest_actions;
use crate::helpers::quest_types::{QuestContext, json_bool, json_int, json_str, resolve_var_name};
use crate::helpers::reward;
use crate::scene::now_millis;
use crate::sink::PushSink;

use super::triggers::{
    action_node_progress_key, build_pre_show_quest_list, consume_hand_in_items,
    count_node_actions, find_node_by_id, get_child_condition, handle_interact_option,
    is_battle_attr_or_wait_satisfied, is_kill_satisfied, is_level_play_satisfied,
    is_schedule_time_window_match, mark_and_check_do_interact_complete, match_do_interact,
    matches_check_ui_game_key, matches_hand_in_entity, matches_quest_interact_starter,
    matches_reach_area, nested_dungeon_id, push_node_progress_notify, push_ready_quest_list,
    reevaluate_condition_listeners, reevaluate_in_range_triggers, resolve_config_id,
    snapshot_active_quest_ids, start_flow_from_obj, start_flow_params, start_flow_params_bt,
};
use super::{
    PROLOGUE_QUEST_ID, interrupt_active_flow_for_quest,
    tree::{
        activate_quest, advance_matching_child_nodes, advance_quest_tree,
        check_active_quest_failed_monitors, complete_child_quest, complete_matching_child_quests,
        extend_fail_timers_on_entity_state, extend_fail_timers_on_kill, finalize_progression,
        finish_non_child_node, is_eligible_quest, is_subtree_active_for_monitor,
        recheck_pending_conditions, refresh_available_quests, rollback_quest_to_node,
    },
};

thread_local! {
    static TIME_CHANGE_DEPTH: Cell<u32> = const { Cell::new(0) };
    static ENTITY_GROUP_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct EntityGroupDepthGuard;
impl EntityGroupDepthGuard {
    fn try_enter() -> Option<Self> {
        ENTITY_GROUP_DEPTH.with(|d| {
            let cur = d.get();
            if cur >= 4 {
                None
            } else {
                d.set(cur + 1);
                Some(Self)
            }
        })
    }
}
impl Drop for EntityGroupDepthGuard {
    fn drop(&mut self) {
        ENTITY_GROUP_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

struct TimeChangeGuard;
impl TimeChangeGuard {
    fn try_enter() -> Option<Self> {
        TIME_CHANGE_DEPTH.with(|d| {
            let cur = d.get();
            if cur >= 2 {
                None
            } else {
                d.set(cur + 1);
                Some(Self)
            }
        })
    }
}
impl Drop for TimeChangeGuard {
    fn drop(&mut self) {
        TIME_CHANGE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestEventKind {
    Parkour,
    RhythmGame,
    SlidingBlocks,
    Deliver,
    Communicate,
    CheckPlayerInput,
    FinishArcadeGameplay,
    SpringManorGameplayClose,
    CompareActorVar,
    CrossSceneSyncKill,
    FinishFlipper,
    FinishKuroSimpleCombatKill,
    FinishKurotato,
    FinishRoleTower,
    FinishRollBlock,
    FinishSurvivorsRouge,
    FinishTetris,
    FinishTrapDefense,
    NewHandInItems,
    ParallaxAlign,
    PlaybackShowTimeline,
    TakePicturesWithTimeScale,
    WalkingPattern,
}

impl QuestEventKind {
    pub fn condition_type(self) -> &'static str {
        match self {
            Self::Parkour => "Parkour",
            Self::RhythmGame => "RhythmGame",
            Self::SlidingBlocks => "SlidingBlocks",
            Self::Deliver => "Deliver",
            Self::Communicate => "Communicate",
            Self::CheckPlayerInput => "CheckPlayerInput",
            Self::FinishArcadeGameplay => "FinishArcadeGameplay",
            Self::SpringManorGameplayClose => "SpringManorGameplayClose",
            Self::CompareActorVar => "CompareActorVar",
            Self::CrossSceneSyncKill => "CrossSceneSyncKillMonster",
            Self::FinishFlipper => "FinishFlipper",
            Self::FinishKuroSimpleCombatKill => "FinishKuroSimpleCombatKillMonster",
            Self::FinishKurotato => "FinishKurotato",
            Self::FinishRoleTower => "FinishRoleTower",
            Self::FinishRollBlock => "FinishRollBlock",
            Self::FinishSurvivorsRouge => "FinishSurvivorsRouge",
            Self::FinishTetris => "FinishTetris",
            Self::FinishTrapDefense => "FinishTrapDefense",
            Self::NewHandInItems => "NewHandInItems",
            Self::ParallaxAlign => "ParallaxAlign",
            Self::PlaybackShowTimeline => "PlaybackShowTimeline",
            Self::TakePicturesWithTimeScale => "TakePicturesWithTimeScale",
            Self::WalkingPattern => "WalkingPattern",
        }
    }
}

fn merged_interact_component<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cfg_id: i32,
) -> Option<sonoro_gamesv_data::tables::entity_components::interact::InteractComponent> {
    if cfg_id <= 0 {
        return None;
    }
    let inst_id = ctx.scene.as_deref().map_or(8, |s| s.instance_id);
    let lcfg = ctx.assets.find_level_entity(inst_id, i64::from(cfg_id))?;
    let mut components = lcfg.ComponentsData.clone();
    if let Some(tpl) = ctx.assets.find_template_config(&lcfg.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut components);
    }
    components.InteractComponent
}

fn execute_normal_interact_actions<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    cfg_id: i32,
    option_index: i32,
    actions: &[sonoro_gamesv_data::tables::entity_components::interact::Action],
    no_active_flow: bool,
) -> io::Result<()> {
    let mut cbs = super::action_callbacks();
    cbs.is_flow_action = false;
    let mut state_changed = false;
    let mut started_flow = false;

    let mut can_start_sync_flow = no_active_flow;
    for act in actions {
        if act.Name == "PlayFlow" {
            if !started_flow
                && let Some(p) = &act.Params
            {
                let is_async = act
                    .Raw
                    .as_ref()
                    .and_then(|v| v.get("Params"))
                    .and_then(|vp| vp.get("Async"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                if !is_async
                    && !can_start_sync_flow
                    && let Some(scene) = ctx.scene.as_deref_mut()
                {
                    scene.clear_active_flow();
                    can_start_sync_flow = true;
                }
                if is_async || can_start_sync_flow {
                    let flow_name = p
                        .FlowListName
                        .as_deref()
                        .or_else(|| p.Flow.as_ref().and_then(|f| f.FlowListName.as_deref()))
                        .or_else(|| p.Flows.as_ref().and_then(|f| f.FlowListName.as_deref()))
                        .unwrap_or("");
                    let flow_id = p
                        .FlowId
                        .or_else(|| p.Flow.as_ref().and_then(|f| f.FlowId))
                        .or_else(|| p.Flows.as_ref().and_then(|f| f.FlowId))
                        .unwrap_or(0);
                    let state_id = p
                        .StateId
                        .or_else(|| p.Flow.as_ref().and_then(|f| f.StateId))
                        .or_else(|| p.Flows.as_ref().and_then(|f| f.StateId))
                        .unwrap_or(1)
                        .max(1);
                    if !flow_name.is_empty() && flow_id > 0 {
                        let _ = start_entity_interact_flow(
                            ctx,
                            entity_id,
                            cfg_id,
                            option_index,
                            flow_name,
                            flow_id,
                            state_id,
                            is_async,
                        )?;
                        started_flow = true;
                    }
                }
            }
        } else if act.Name == "ChangeSelfEntityState" {
            let Some(p) = &act.Params else { continue };
            let state_str = if !p.EntityState.is_empty() {
                p.EntityState.as_str()
            } else {
                p.State.as_deref().unwrap_or("")
            };
            if !state_str.is_empty() {
                quest_actions::apply_entity_state_change(ctx, &cbs, cfg_id, state_str)?;
                state_changed = true;
            }
        } else if let Some(raw_act) = &act.Raw {
            let prev_owner = if let Some(scene) = ctx.scene.as_deref_mut() {
                let prev = scene.active_flow_owner_entity_id;
                scene.active_flow_owner_entity_id = entity_id;
                prev
            } else {
                0
            };
            let res = quest_actions::execute_action(ctx, &mut cbs, cfg_id, option_index, raw_act);
            if let Some(scene) = ctx.scene.as_deref_mut() {
                scene.active_flow_owner_entity_id = prev_owner;
            }
            res?;
            state_changed = true;
        } else if act.Name == "ChangeEntityState" {
            let Some(p) = &act.Params else { continue };
            let action_type = p.Type.as_deref().unwrap_or("Directly");
            if action_type == "BatchDirectly" {
                let Some(state_str) = p.State.as_deref().filter(|s| !s.is_empty()) else {
                    continue;
                };
                for &eid in &p.EntityIds {
                    if let Ok(eid32) = i32::try_from(eid) {
                        quest_actions::apply_entity_state_change(ctx, &cbs, eid32, state_str)?;
                        state_changed = true;
                    }
                }
            } else if let Some(eid) = p.EntityId.and_then(|v| i32::try_from(v).ok()).filter(|&v| v > 0) {
                if action_type == "Loop" {
                    let cur = ctx
                        .quest_comp
                        .info
                        .get_entity_state(i64::from(eid))
                        .unwrap_or("");
                    let next_state = if cur.contains("激活") {
                        "关卡.Common.状态.常态"
                    } else {
                        "关卡.Common.状态.激活"
                    };
                    quest_actions::apply_entity_state_change(ctx, &cbs, eid, next_state)?;
                    state_changed = true;
                } else if let Some(state_str) = p.State.as_deref().filter(|s| !s.is_empty()) {
                    quest_actions::apply_entity_state_change(ctx, &cbs, eid, state_str)?;
                    state_changed = true;
                }
            }
        } else if act.Name == "TeleportDungeon" {
            if let Some(p) = &act.Params
                && let Some(did) = p.DungeonId.filter(|&d| d > 0)
            {
                quest_actions::resolve_teleport_dungeon_action(
                    ctx,
                    did,
                    p.LocationEntityId.filter(|&id| id > 0),
                )?;
            }
        } else if act.Name == "ExitDungeon" {
            let pos_eid = act
                .Params
                .as_ref()
                .and_then(|p| p.LocationEntityId)
                .and_then(|id| i32::try_from(id).ok())
                .filter(|&id| id > 0)
                .unwrap_or(0);
            quest_actions::leave_current_dungeon(ctx, pos_eid)?;
        }
    }

    if state_changed {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_interact_handled<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    option_index: i32,
) -> io::Result<bool> {
    if ctx
        .quest_comp
        .info
        .find_active_quest(PROLOGUE_QUEST_ID)
        .is_some_and(|aq| aq.is_node_active(184))
    {
        let cfg_id = resolve_config_id(ctx.scene.as_deref(), entity_id);
        if cfg_id == 100001055 && let Some(scene) = ctx.scene.as_deref_mut() {
            prologue_quest::transition_node(ctx.sink, scene, 184, 0);
            prologue_quest::finish_prologue_quest(ctx.sink, scene);
            ctx.quest_comp.info.finish_quest(PROLOGUE_QUEST_ID);
            let _ = refresh_available_quests(ctx, true)?;
            finalize_progression(ctx)?;
            return Ok(true);
        }
    }

    let cfg_id = resolve_config_id(ctx.scene.as_deref(), entity_id);
    let interact_comp = merged_interact_component(ctx, cfg_id);
    let normal_opts_len = interact_comp
        .as_ref()
        .filter(|ic| !ic.Disabled.unwrap_or(false))
        .map_or(0, |ic| ic.Options.len());
    let prefer_normal_opt = option_index >= 0 && (option_index as usize) < normal_opts_len;

    if !prefer_normal_opt
        && let Some(scene) = ctx.scene.as_deref()
        && !scene.dynamic_interact_options.is_empty()
    {
        for opt in scene.dynamic_interact_options.clone() {
            if opt.entity_net_id != entity_id && opt.entity_config_id != cfg_id {
                continue;
            }
            if let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(opt.quest_id)
                && let Some(node) = find_node_by_id(&nodes, opt.node_id).cloned()
                && let Some(cond) = get_child_condition(&node.data).cloned()
                && json_str(cond.get("Type")) == Some("DoInteract")
                && let Some(opt_obj) = match_do_interact(&cond, entity_id, cfg_id, option_index).cloned()
            {
                if handle_interact_option(
                    ctx,
                    opt.quest_id,
                    &nodes,
                    &node,
                    &cond,
                    entity_id,
                    &opt_obj,
                )? {
                    finalize_progression(ctx)?;
                }
                return Ok(true);
            }
        }
    }

    let mut progressed = false;

    let active_snapshot = snapshot_active_quest_ids(ctx);
    'active: for qid in active_snapshot {
        let Some(aq) = ctx.quest_comp.info.find_active_quest(qid).cloned() else {
            continue;
        };
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(qid) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            let Some(node) = find_node_by_id(&nodes, nid).cloned() else {
                continue;
            };
            let Some(cond) = get_child_condition(&node.data).cloned() else {
                continue;
            };
            let ctype = json_str(cond.get("Type")).unwrap_or("");
            if ctype == "DoInteract"
                && !prefer_normal_opt
                && let Some(opt_obj) =
                    match_do_interact(&cond, entity_id, cfg_id, option_index).cloned()
            {
                if handle_interact_option(ctx, qid, &nodes, &node, &cond, entity_id, &opt_obj)? {
                    progressed = true;
                }
                break 'active;
            } else if ctype == "HandInItems"
                && matches_hand_in_entity(&cond, entity_id, cfg_id)
                && consume_hand_in_items(ctx, &cond)?
            {
                complete_child_quest(ctx, qid, &nodes, &node)?;
                progressed = true;
                break 'active;
            }
        }
    }

    if !progressed && ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID) {
        let candidates: Vec<i32> = {
            let eval_ctx = ctx.make_eval_context();
            ctx.assets
                .tables
                .quest_data
                .items
                .iter()
                .filter_map(|q| {
                    let d = &q.Data;
                    if d.DistributeType != "Interact" {
                        return None;
                    }
                    if !super::tree::is_eligible_quest(ctx, d, false) {
                        return None;
                    }
                    if !matches_quest_interact_starter(ctx, d, entity_id, cfg_id) {
                        return None;
                    }
                    condition::eval_quest_provide_conditions(&eval_ctx, d.ProvideType.as_ref())
                        .then_some(d.Id)
                })
                .collect()
        };
        for qid in candidates {
            if super::tree::activate_quest(ctx, qid, true)? {
                progressed = true;
            }
        }
    }

    if progressed {
        finalize_progression(ctx)?;
        return Ok(true);
    }

    if cfg_id > 0
        && option_index >= 0
        && let Some(ic) = interact_comp
        && let Some(opt) = ic.Options.get(option_index as usize)
    {
        if let Some(opt_cond) = &opt.Condition {
            let mut eval_ctx = ctx.make_eval_context();
            eval_ctx.self_entity_config_id = cfg_id;
            if !condition::eval_inline_condition_block(&eval_ctx, opt_cond) {
                return Ok(false);
            }
        }
        let mut no_active_flow = ctx.scene.as_deref().is_none_or(|s| s.active_flow_inc_id == 0);
        if let Some(ot) = &opt.Type {
            if ot.Type == "Flow" {
                if !no_active_flow
                    && let Some(scene) = ctx.scene.as_deref_mut()
                {
                    scene.clear_active_flow();
                    no_active_flow = true;
                }
                if no_active_flow && let Some(fl) = &ot.Flow {
                    let flow_name = fl.FlowListName.as_deref().unwrap_or("");
                    let flow_id = fl.FlowId.unwrap_or(0);
                    let state_id = fl.StateId.unwrap_or(1).max(1);
                    if !flow_name.is_empty() && flow_id > 0 {
                        let _ = start_entity_interact_flow(
                            ctx,
                            entity_id,
                            cfg_id,
                            option_index,
                            flow_name,
                            flow_id,
                            state_id,
                            false,
                        )?;
                        return Ok(true);
                    }
                }
            } else if ot.Type == "Actions" {
                execute_normal_interact_actions(
                    ctx,
                    entity_id,
                    cfg_id,
                    option_index,
                    &ot.Actions,
                    no_active_flow,
                )?;
                return Ok(true);
            }
        }
    }

    Ok(false)
}

pub fn on_player_move<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    x: f32,
    y: f32,
    z: f32,
) -> io::Result<()> {
    let pos = [x, y, z];
    let mut progressed = false;

    let active_snapshot = snapshot_active_quest_ids(ctx);
    for qid in active_snapshot {
        let Some(aq) = ctx.quest_comp.info.find_active_quest(qid).cloned() else {
            continue;
        };
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(qid) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            let Some(node) = find_node_by_id(&nodes, nid).cloned() else {
                continue;
            };
            let Some(cond) = get_child_condition(&node.data) else {
                continue;
            };
            if json_str(cond.get("Type")) == Some("ReachArea")
                && matches_reach_area(ctx, qid, &node.data, cond, pos)
            {
                complete_child_quest(ctx, qid, &nodes, &node)?;
                progressed = true;
            }
        }
    }

    if check_active_quest_failed_monitors(ctx, Some(pos))? {
        progressed = true;
    }

    if progressed {
        finalize_progression(ctx)?;
    }

    super::triggers::check_auto_interact_triggers(ctx, pos)?;

    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        level_play::on_player_move(&mut lp_ctx, x, y, z)?;
    }
    Ok(())
}

fn is_entity_kill_relevant_to_quest(
    aq: &sonoro_gamesv_fs::quest_info::ActiveQuest,
    nodes: &[sonoro_gamesv_data::lazy_node_store::NodeEntry],
    cfg_id: i32,
) -> bool {
    for &nid in &aq.active_nodes {
        let Some(node) = find_node_by_id(nodes, nid) else {
            continue;
        };
        if node.node_type != "ChildQuest" {
            continue;
        }
        let Some(cond) = get_child_condition(&node.data) else {
            continue;
        };
        let ctype = json_str(cond.get("Type")).unwrap_or("");
        if ctype == "MonsterCreator" {
            if let Some(Value::Array(arr)) = cond.get("MonsterCreatorEntityIds")
                && arr.iter().any(|v| json_int(Some(v)) == Some(cfg_id))
            {
                return true;
            }
        } else if ctype == "Kill" {
            if json_int(cond.get("EntityId")) == Some(cfg_id) {
                return true;
            }
            for field in ["TargetsToAwake", "ExistTargets"] {
                if let Some(Value::Array(arr)) = cond.get(field)
                    && arr.iter().any(|v| json_int(Some(v)) == Some(cfg_id))
                {
                    return true;
                }
            }
        } else if ctype == "DetectCombatState" || ctype == "DetectCombatState2" {
            if json_int(cond.get("EntityId")) == Some(cfg_id) {
                return true;
            }
            if let Some(Value::Array(cs)) = cond.get("Conditions")
                && cs.iter().any(|c| {
                    c.as_object()
                        .and_then(|o| json_int(o.get("EntityId")))
                        == Some(cfg_id)
                })
            {
                return true;
            }
        }
    }
    for node in nodes {
        if node.node_type != "QuestFailed" {
            continue;
        }
        if !is_subtree_active_for_monitor(nodes, &aq.active_nodes, node.parent_id) {
            continue;
        }
        if let Some(arr) = node
            .data
            .get("FailedCondition")
            .and_then(|fc| fc.get("EntityDeathCondition"))
            .and_then(|edc| edc.get("EntityIds"))
            .and_then(|v| v.as_array())
            && arr.iter().any(|v| json_int(Some(v)) == Some(cfg_id))
        {
            return true;
        }
    }
    false
}

fn entity_has_state_tag<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    target_cfg: i32,
    state_name: &str,
) -> bool {
    super::tree::entity_has_state_tag(ctx, target_cfg, state_name)
}

fn combat_state_allows_hp_fallback(state_name: &str) -> bool {
    ["阶段", "濒死", "处决", "信号", "瘫痪", "对策", "被控", "破防", "虚弱", "狂暴"]
        .iter()
        .any(|pat| state_name.contains(pat))
}

fn combat_state_threshold(state_name: &str, hp_ratio: f32) -> f32 {
    if hp_ratio <= 0.0 {
        return 1.0;
    }
    if state_name.contains("三阶段") || state_name.contains("处决") {
        0.40
    } else {
        0.75
    }
}

fn is_client_judged_condition(ctype: &str) -> bool {
    ctype == "DetectCombatState"
}

fn combat_state2_holds<F>(cond: &Map<String, Value>, fallback_entity: i32, has_state: F) -> bool
where
    F: Fn(i32, &str) -> bool,
{
    let Some(Value::Array(listed)) = cond.get("Conditions") else {
        return false;
    };
    if listed.is_empty() {
        return false;
    }
    let asked = json_int(cond.get("Count")).unwrap_or(0);
    let needed = if asked <= 0 {
        listed.len()
    } else {
        asked as usize
    };
    let mut held = 0usize;
    for entry in listed {
        let Value::Object(obj) = entry else {
            continue;
        };
        let own_entity = json_int(obj.get("EntityId")).unwrap_or(0);
        let state = json_str(obj.get("State")).unwrap_or("");
        let check_entity = if own_entity != 0 {
            own_entity
        } else {
            fallback_entity
        };
        if has_state(check_entity, state) {
            held += 1;
        }
    }
    held >= needed
}

fn check_combat_state_for_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    cfg_id: i32,
    hp_ratio: f32,
) -> io::Result<bool> {
    let active_nodes = match ctx.quest_comp.info.find_active_quest(quest_id) {
        Some(aq) if !aq.active_nodes.is_empty() => aq.active_nodes.clone(),
        _ => return Ok(false),
    };
    let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
        return Ok(false);
    };

    let mut changed = false;
    for nid in active_nodes {
        let Some(node) = find_node_by_id(&nodes, nid).cloned() else {
            continue;
        };
        if node.node_type != "ChildQuest" {
            continue;
        }
        let Some(cond) = get_child_condition(&node.data) else {
            continue;
        };
        let ctype = json_str(cond.get("Type")).unwrap_or("");
        if is_client_judged_condition(ctype) {
            continue;
        }
        if ctype == "DetectCombatState2" {
            let mut matched = false;
            if let Some(Value::Array(cs)) = cond.get("Conditions")
                && !cs.is_empty()
            {
                let req_cnt = json_int(cond.get("Count")).unwrap_or(0);
                let needed = if req_cnt <= 0 {
                    cs.len()
                } else {
                    req_cnt as usize
                };
                let mut hit_cnt = 0usize;
                for item in cs {
                    let Value::Object(obj) = item else {
                        continue;
                    };
                    let te = json_int(obj.get("EntityId")).unwrap_or(0);
                    let st = json_str(obj.get("State")).unwrap_or("");
                    let check_eid = if te != 0 { te } else { cfg_id };
                    let has_tag = entity_has_state_tag(ctx, check_eid, st);
                    let hp_hit = (te == 0 || te == cfg_id)
                        && (hp_ratio <= 0.0
                            || (combat_state_allows_hp_fallback(st)
                                && hp_ratio <= combat_state_threshold(st, hp_ratio)));
                    if has_tag || hp_hit {
                        hit_cnt += 1;
                    }
                }
                matched = hit_cnt >= needed;
            }
            if matched {
                complete_child_quest(ctx, quest_id, &nodes, &node)?;
                changed = true;
            }
        } else if (ctype == "CheckTargetBattleAttribute" || ctype == "WaitBattleCondition")
            && is_battle_attr_or_wait_satisfied(ctx, cond, ctype, cfg_id, hp_ratio)
        {
            complete_child_quest(ctx, quest_id, &nodes, &node)?;
            changed = true;
        }
    }
    Ok(changed)
}

pub fn on_entity_killed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    killed_net_id: i64,
    killed_cfg_id_opt: Option<i32>,
) -> io::Result<()> {
    let resolved_cfg_id = killed_cfg_id_opt.unwrap_or_else(|| {
        ctx.scene
            .as_deref()
            .and_then(|s| s.find_config_id_by_net_id(killed_net_id))
            .unwrap_or(0)
    });

    let mut progressed = false;

    if resolved_cfg_id > 0 {
        let active_snapshot = snapshot_active_quest_ids(ctx);
        for qid in active_snapshot {
            let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(qid) else {
                continue;
            };
            let relevant = ctx
                .quest_comp
                .info
                .find_active_quest(qid)
                .is_some_and(|aq| is_entity_kill_relevant_to_quest(aq, &nodes, resolved_cfg_id));
            if relevant && let Some(aq_ptr) = ctx.quest_comp.info.find_active_quest_mut(qid) {
                aq_ptr.record_killed_entity(resolved_cfg_id);
            }
            if check_combat_state_for_quest(ctx, qid, resolved_cfg_id, 0.0)? {
                progressed = true;
            }
            let active_nodes = ctx
                .quest_comp
                .info
                .find_active_quest(qid)
                .map(|aq| aq.active_nodes.clone())
                .unwrap_or_default();
            for nid in active_nodes {
                let Some(node) = find_node_by_id(&nodes, nid).cloned() else {
                    continue;
                };
                if node.node_type != "ChildQuest" {
                    continue;
                }
                let Some(cond) = get_child_condition(&node.data) else {
                    continue;
                };
                let Some(ctype) = json_str(cond.get("Type")) else {
                    continue;
                };
                if ctype != "Kill" && ctype != "MonsterCreator" {
                    continue;
                }
                if let Some(aq) = ctx.quest_comp.info.find_active_quest(qid).cloned() {
                    if is_kill_satisfied(&aq, cond, ctype) {
                        complete_child_quest(ctx, qid, &nodes, &node)?;
                        progressed = true;
                    } else if relevant {
                        push_node_progress_notify(ctx, &aq, &node);
                    }
                }
            }
        }
    }

    if check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }
    if extend_fail_timers_on_kill(ctx) {
        progressed = true;
    }

    if progressed {
        finalize_progression(ctx)?;
    }

    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        level_play::on_entity_killed_with_conditions(&mut lp_ctx, killed_net_id, killed_cfg_id_opt)?;
    }
    Ok(())
}

pub fn on_entity_hit<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    target_net_id: i64,
    hp_ratio: f32,
) -> io::Result<()> {
    let cfg_id = ctx
        .scene
        .as_deref()
        .and_then(|s| s.find_config_id_by_net_id(target_net_id))
        .unwrap_or(0);
    if cfg_id <= 0 {
        return Ok(());
    }
    let mut progressed = false;
    let active_snapshot = snapshot_active_quest_ids(ctx);
    for qid in active_snapshot {
        if check_combat_state_for_quest(ctx, qid, cfg_id, hp_ratio)? {
            progressed = true;
        }
    }
    if check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_entity_state_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_config_id: i32,
    state_name: &str,
) -> io::Result<()> {
    let mut progressed = extend_fail_timers_on_entity_state(ctx, entity_config_id, state_name);

    let active_snapshot = snapshot_active_quest_ids(ctx);
    for qid in active_snapshot {
        let Some(aq) = ctx.quest_comp.info.find_active_quest(qid).cloned() else {
            continue;
        };
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(qid) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            let Some(node) = find_node_by_id(&nodes, nid).cloned() else {
                continue;
            };
            let Some(cond) = get_child_condition(&node.data) else {
                continue;
            };
            let ctype = json_str(cond.get("Type")).unwrap_or("");
            if ctype == "CheckEntityState" {
                if super::triggers::is_check_entity_state_satisfied(ctx, cond) {
                    complete_child_quest(ctx, qid, &nodes, &node)?;
                    progressed = true;
                } else if let Some(aq_cur) = ctx.quest_comp.info.find_active_quest(qid).cloned() {
                    push_node_progress_notify(ctx, &aq_cur, &node);
                }
            } else if ctype == "ListenToEvent"
                && let Some(Value::Object(et)) = cond.get("EventType")
                && json_str(et.get("Type")) == Some("EntityStateChange")
            {
                let eid = json_int(et.get("EntityId")).unwrap_or(0);
                let want_state = json_str(et.get("State")).unwrap_or("");
                if (eid == 0 || eid == entity_config_id)
                    && (want_state.is_empty() || want_state == state_name)
                {
                    complete_child_quest(ctx, qid, &nodes, &node)?;
                    progressed = true;
                }
            } else if ctype == "DetectCombatState2" {
                if combat_state2_holds(cond, entity_config_id, |eid, st| {
                    entity_has_state_tag(ctx, eid, st)
                }) {
                    complete_child_quest(ctx, qid, &nodes, &node)?;
                    progressed = true;
                }
            } else if ctype == "WaitBattleCondition"
                && is_battle_attr_or_wait_satisfied(ctx, cond, ctype, entity_config_id, 1.0)
            {
                complete_child_quest(ctx, qid, &nodes, &node)?;
                progressed = true;
            }
        }
    }

    if refresh_available_quests(ctx, true)? {
        progressed = true;
    }
    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }

    if progressed {
        finalize_progression(ctx)?;
    }

    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        level_play::on_entity_state_changed(&mut lp_ctx, entity_config_id, state_name)?;
    }
    if evaluate_entity_group_triggers_for_member(ctx, entity_config_id)? {
        finalize_progression(ctx)?;
    }
    Ok(())
}

fn eval_entity_group_condition<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    gc: &Map<String, Value>,
) -> bool {
    let Some(Value::Array(conds)) = gc.get("Conditions") else {
        return false;
    };
    if conds.is_empty() {
        return false;
    }
    let mut matched = 0i32;
    for entry in conds {
        let Some(eobj) = entry.as_object() else {
            continue;
        };
        let Some(eid) = json_int(eobj.get("EntityId")) else {
            continue;
        };
        let Some(want) = json_str(eobj.get("State")) else {
            continue;
        };
        let cur = ctx
            .quest_comp
            .info
            .get_entity_state(i64::from(eid))
            .unwrap_or("关卡.Common.状态.常态");
        if cur == want {
            matched += 1;
        }
    }
    let need = json_int(gc.get("Count")).unwrap_or(conds.len() as i32);
    matched >= need
}

fn evaluate_entity_group_triggers_for_member<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    member_cfg_id: i32,
) -> io::Result<bool> {
    if member_cfg_id <= 0 {
        return Ok(false);
    }
    let Some(_guard) = EntityGroupDepthGuard::try_enter() else {
        return Ok(false);
    };
    let Some(map_id) = ctx
        .scene
        .as_deref()
        .map(|s| ctx.assets.resolve_map_config_id(s.instance_id))
    else {
        return Ok(false);
    };
    let specs = ctx
        .assets
        .indexes
        .entity_groups_by_member
        .get(&MapEntityKey {
            map_id,
            entity_id: i64::from(member_cfg_id),
        })
        .cloned()
        .unwrap_or_default();
    if specs.is_empty() {
        return Ok(false);
    }

    let mut changed = false;
    let mut cbs = super::action_callbacks::<S>();
    for spec in specs {
        let gid = spec.group_entity_id;
        if let Some(Value::Array(triggers)) = spec.component.StateTriggers.as_ref().map(|v| Value::Array(v.clone())).as_ref() {
            for (idx, trig) in triggers.iter().enumerate() {
                let Some(tobj) = trig.as_object() else {
                    continue;
                };
                let is_match = tobj
                    .get("GroupCondition")
                    .and_then(Value::as_object)
                    .is_some_and(|gc| eval_entity_group_condition(ctx, gc));
                if is_match {
                    let already = ctx
                        .progress_comp
                        .as_deref()
                        .is_some_and(|pc| pc.info.has_claim(&format!("eg_trig:{gid}:{idx}")));
                    if !already {
                        if let Some(pc) = ctx.progress_comp.as_deref_mut() {
                            pc.info.mark_claim(&format!("eg_trig:{gid}:{idx}"));
                            pc.save(ctx.fs)?;
                        }
                        if let Some(Value::Array(acts)) = tobj.get("SuccessActions") {
                            quest_actions::execute_action_list(ctx, &mut cbs, 0, idx as i32, acts)?;
                            changed = true;
                        }
                    }
                } else if let Some(Value::Array(fail_acts)) = tobj.get("FailActions")
                    && !fail_acts.is_empty()
                {
                    let was_triggered = ctx
                        .progress_comp
                        .as_deref()
                        .is_some_and(|pc| pc.info.has_claim(&format!("eg_trig:{gid}:{idx}")));
                    if was_triggered {
                        quest_actions::execute_action_list(ctx, &mut cbs, 0, idx as i32, fail_acts)?;
                        changed = true;
                    }
                }
            }
        }

        if let Some(fs_obj) = spec.component.FinishState.as_ref().and_then(Value::as_object)
            && let Some(gc) = fs_obj.get("GroupCondition").and_then(Value::as_object)
            && eval_entity_group_condition(ctx, gc)
            && ctx.quest_comp.info.get_entity_state(gid) != Some("关卡.Common.状态.完成")
        {
            ctx.quest_comp
                .info
                .set_entity_state(gid, "关卡.Common.状态.完成");
            ctx.quest_comp.save(ctx.fs)?;
            if let Ok(gid32) = i32::try_from(gid) {
                on_entity_state_changed(ctx, gid32, "关卡.Common.状态.完成")?;
            }
            changed = true;
        }

        if let Some(fl_obj) = spec.component.FailureState.as_ref().and_then(Value::as_object)
            && let Some(Value::Array(fconds)) = fl_obj.get("FailureConditions")
        {
            let mut failed = false;
            for fc in fconds {
                let Some(fobj) = fc.as_object() else {
                    continue;
                };
                if json_str(fobj.get("Type")) == Some("SequentialState")
                    && let Some(Value::Array(order)) = fobj.get("Order")
                {
                    let mut seen_mismatch = false;
                    for step in order {
                        let Some(sobj) = step.as_object() else {
                            continue;
                        };
                        let Some(eid) = json_int(sobj.get("EntityId")) else {
                            continue;
                        };
                        let Some(want) = json_str(sobj.get("State")) else {
                            continue;
                        };
                        let cur = ctx
                            .quest_comp
                            .info
                            .get_entity_state(i64::from(eid))
                            .unwrap_or("关卡.Common.状态.常态");
                        if cur == want {
                            if seen_mismatch {
                                failed = true;
                                break;
                            }
                        } else if cur != "关卡.Common.状态.常态" {
                            failed = true;
                            break;
                        } else {
                            seen_mismatch = true;
                        }
                    }
                }
            }
            if failed && let Some(ops) = fl_obj.get("FailureOperations").and_then(Value::as_object) {
                if let Some(Value::Array(acts)) = ops.get("Actions") {
                    quest_actions::execute_action_list(ctx, &mut cbs, 0, 0, acts)?;
                    changed = true;
                }
                if json_bool(ops.get("IsResetState")).unwrap_or(false)
                    && let Some(ids) = spec.component.EntityIds.as_ref()
                {
                    for &eid in ids {
                        ctx.quest_comp
                            .info
                            .set_entity_state(eid, "关卡.Common.状态.常态");
                    }
                    ctx.quest_comp.save(ctx.fs)?;
                    changed = true;
                }
            }
        }
    }
    Ok(changed)
}

pub fn on_var_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    var_name: &str,
) -> io::Result<()> {
    let mut progressed = advance_matching_child_nodes(
        ctx,
        &var_name,
        |c, &vname, qid, _aq, nodes, node, cond, ctype| {
            if ctype != "ListenToEvent" {
                return Ok(false);
            }
            let Some(Value::Object(et)) = cond.get("EventType") else {
                return Ok(false);
            };
            if json_str(et.get("Type")) != Some("VarValueChange") {
                return Ok(false);
            }
            let want_name = json_str(et.get("VarName"))
                .or_else(|| json_str(et.get("Name")))
                .unwrap_or("");
            if !want_name.is_empty() && !vname.is_empty() && want_name != vname {
                return Ok(false);
            }
            if let Some(Value::Object(vobj)) = et.get("Var")
                && let Some(vn) = resolve_var_name(vobj)
                && !vn.is_empty()
                && !vname.is_empty()
                && vn != vname
            {
                return Ok(false);
            }
            complete_child_quest(c, qid, nodes, node)?;
            Ok(true)
        },
    )?;

    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        level_play::on_var_changed(&mut lp_ctx, owner_id, var_name)?;
    }
    Ok(())
}

pub fn on_item_gained<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    item_id: i32,
) -> io::Result<()> {
    let mut progressed = advance_matching_child_nodes(ctx, &(), |c, _, qid, aq, nodes, node, cond, ctype| {
        if ctype != "GetItem" {
            return Ok(false);
        }
        if super::triggers::has_all_items(c, cond.get("Items")) {
            complete_child_quest(c, qid, nodes, node)?;
            return Ok(true);
        }
        push_node_progress_notify(c, aq, node);
        Ok(false)
    })?;
    let cfg = morale::resolve_play_config(Some(ctx.assets), 1);
    if item_id == cfg.keep_item_id
        || item_id == cfg.temp_item_id
        || item_id == DEFAULT_KEEP_ITEM_ID
        || item_id == DEFAULT_TEMP_ITEM_ID
    {
        let _ = morale::sync_from_inventory(
            Some(ctx.sink),
            ctx.fs,
            ctx.assets,
            ctx.basic_comp,
            ctx.inventory_comp,
            ctx.progress_comp.as_deref_mut(),
            None,
            None,
            true,
        )?;
        on_condition_state_changed(ctx)?;
    }
    if refresh_available_quests(ctx, true)? {
        progressed = true;
    }
    if check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    reevaluate_in_range_triggers(ctx)?;
    reevaluate_condition_listeners(ctx)?;
    Ok(())
}

pub fn on_use_item<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    item_id: i32,
    count: i32,
) -> io::Result<()> {
    if item_id <= 0 {
        return Ok(());
    }
    let delta = count.max(1);
    let mut progressed = advance_matching_child_nodes(
        ctx,
        &(item_id, delta),
        |c, &(iid, cnt_delta), qid, _aq, nodes, node, cond, ctype| {
            if ctype != "UseItem" {
                return Ok(false);
            }
            let target_iid = json_int(cond.get("ItemId")).unwrap_or(0);
            if target_iid > 0 && target_iid != iid {
                return Ok(false);
            }
            let req = json_int(cond.get("Count")).unwrap_or(1).max(1);
            let new_cnt = if let Some(pc) = c.progress_comp.as_deref_mut() {
                let key = format!("use_item:{qid}:{}:{iid}", node.id);
                let v = pc.info.add_key_int(&key, i64::from(cnt_delta));
                pc.save(c.fs)?;
                v
            } else {
                i64::from(req)
            };
            if new_cnt >= i64::from(req) {
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
            if let Some(aq_cur) = c.quest_comp.info.find_active_quest(qid).cloned() {
                push_node_progress_notify(c, &aq_cur, node);
            }
            Ok(false)
        },
    )?;
    if ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID) {
        let candidates: Vec<i32> = {
            let eval_ctx = ctx.make_eval_context();
            ctx.assets
                .tables
                .quest_data
                .items
                .iter()
                .filter_map(|q| {
                    let d = &q.Data;
                    if d.DistributeType != "UseItem" || !is_eligible_quest(ctx, d, false) {
                        return None;
                    }
                    condition::eval_quest_provide_conditions(&eval_ctx, d.ProvideType.as_ref())
                        .then_some(d.Id)
                })
                .collect()
        };
        for qid in candidates {
            if activate_quest(ctx, qid, true)? {
                progressed = true;
            }
        }
    }
    if check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_dungeon_event<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    inst_id: i32,
    finished: bool,
) -> io::Result<()> {
    if inst_id <= 0 {
        return Ok(());
    }
    let mut dirty = false;
    if finished {
        ctx.quest_comp.info.mark_dungeon_completed(inst_id);
        record_challenge_dungeon_finish(ctx, inst_id)?;
        dirty = true;
    }

    if let Some(d) = ctx.assets.tables.instance_dungeon.get_data_by_id(&inst_id)
        && finished
        && d.FirstRewardId > 0
    {
        let lp_first_key = format!("lp_first:{}", -inst_id);
        let already = ctx
            .progress_comp
            .as_deref()
            .is_some_and(|pc| pc.info.has_claim(&lp_first_key));
        if !already {
            if let Some(pc) = ctx.progress_comp.as_deref_mut() {
                let _ = pc.info.mark_claim(&lp_first_key);
                pc.save(ctx.fs)?;
            }
            let _ = reward::grant_drop_package_popup(
                &mut ctx.make_reward_context(),
                d.FirstRewardId,
                7001,
            );
        }
    }

    if dirty {
        ctx.quest_comp.save(ctx.fs)?;
    }

    let mut progressed = if finished {
        refresh_available_quests(ctx, true)?
    } else {
        false
    };
    if advance_matching_child_nodes(
        ctx,
        &(inst_id, finished),
        |c, &(iid, fin), qid, _aq, nodes, node, cond, ctype| {
            if !fin && ctype == "EnterDungeon" {
                let target = nested_dungeon_id(cond, "EnterType");
                if target == 0 || target == iid {
                    complete_child_quest(c, qid, nodes, node)?;
                    return Ok(true);
                }
            } else if fin && ctype == "FinishDungeon" {
                let target = nested_dungeon_id(cond, "Condition");
                if target == 0 || target == iid {
                    complete_child_quest(c, qid, nodes, node)?;
                    return Ok(true);
                }
            } else if !fin && iid == 8 && ctype == "LeaveDungeon" {
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
            Ok(false)
        },
    )? {
        progressed = true;
    }

    if finished && check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }
    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

fn record_challenge_dungeon_finish<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    dungeon_id: i32,
) -> io::Result<()> {
    let mut lord_gym_passes = Vec::new();
    for lg in &ctx.assets.tables.lord_gym.items {
        if lg.PlayId == dungeon_id {
            lord_gym_passes.push(lg.Id);
        }
    }
    let preferred_tower_id = ctx
        .progress_comp
        .as_deref()
        .map_or(0, |pc| pc.info.get_key_int("tower:last_floor_id") as i32);
    let matched_tc = ctx
        .assets
        .tables
        .tower_config
        .items
        .iter()
        .find(|tc| tc.InstanceId == dungeon_id && (preferred_tower_id <= 0 || tc.Id == preferred_tower_id))
        .or_else(|| {
            ctx.assets
                .tables
                .tower_config
                .items
                .iter()
                .find(|tc| tc.InstanceId == dungeon_id)
        })
        .cloned();

    let mut changed = false;
    for lg_id in lord_gym_passes {
        let mut is_new_record = false;
        if let Some(pc) = ctx.progress_comp.as_deref_mut() {
            is_new_record = pc.info.record_lord_gym_pass(lg_id, 60);
            changed = true;
        }
        ctx.sink.push(
            LordGymLevelPlayResultNotify::MSG_ID,
            &LordGymLevelPlayResultNotify {
                FailLoadGymId: 0,
                IsSuccess: true,
                PassTime: 60,
                LordGymPassRecord: Some(sonoro_proto::pb::LordGymPassRecord {
                    LoadGymId: lg_id,
                    PassTime: 60,
                    RoleIds: Vec::new(),
                }),
                IsNewRecord: is_new_record,
                IsPkgFull: false,
                ItemDatas: Vec::new(),
            },
        );
    }

    if let Some(tc) = matched_tc {
        let limit_s = ctx
            .assets
            .tables
            .instance_dungeon
            .get_data_by_id(&dungeon_id)
            .map_or(300i64, |inst| {
                if inst.AutoLeaveTime > 0 {
                    i64::from(inst.AutoLeaveTime)
                } else {
                    300
                }
            });
        let start_ms = ctx
            .progress_comp
            .as_deref()
            .map_or(0, |pc| pc.info.get_key_int("tower:start_ms"));
        let cur_ms = now_millis();
        let elapsed_s = if start_ms > 0 && cur_ms >= start_ms {
            (cur_ms - start_ms) / 1000
        } else {
            0
        };
        let remaining_s = (limit_s - elapsed_s).max(0);
        let mut star_count = 0i32;
        let mut star_mask = 0i32;
        let mut star_index = Vec::new();
        for &target_s in tc.Target.iter().take(3) {
            let ti = star_index.len();
            if start_ms <= 0 || remaining_s >= i64::from(target_s) {
                star_count += 1;
                star_mask |= 1 << ti;
                star_index.push((ti as i32) + 1);
            }
        }
        if star_count == 0 {
            star_count = 1;
            star_mask = 1;
            star_index.push(1);
        }
        if let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.info
                .record_tower_floor(tc.Id, tc.Difficulty, tc.AreaNum, star_count, star_mask);
            pc.info.set_key_int("tower:last_floor_id", i64::from(tc.Id));
            pc.info.set_key_int("tower:last_diff", i64::from(tc.Difficulty));
            changed = true;
        }
        let formation_roles: Vec<TowerRolePb> = ctx
            .scene
            .as_deref()
            .and_then(|scene| {
                let fi = usize::try_from(scene.formation_info.cur_formation).ok()?;
                scene.formation_info.formations.get(fi).map(|f| {
                    f.roles
                        .iter()
                        .flatten()
                        .map(|r| TowerRolePb {
                            RoleId: r.role_id,
                            LeaveSkillId: 0,
                            SkillBranchId: 0,
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        let floor_pb = TowerFloorPb {
            TowerConfigId: tc.Id,
            Star: star_count,
            Formation: formation_roles,
            StarIndex: star_index,
            IsQuickPass: false,
        };
        ctx.sink.push(
            TowerFloorUpdateNotify::MSG_ID,
            &TowerFloorUpdateNotify {
                TowerFloors: vec![floor_pb.clone()],
            },
        );
        ctx.sink.push(
            TowerEndNotify::MSG_ID,
            &TowerEndNotify {
                Success: true,
                NeedUpdateSeason: false,
                CurrentFloorData: Some(floor_pb),
            },
        );
        on_condition_event(ctx, QuestEventKind::FinishRoleTower, tc.Id, 1)?;
    }

    if changed && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_ui_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    ui_type: &str,
) -> io::Result<()> {
    if ui_type.is_empty() {
        return Ok(());
    }
    let progressed = advance_matching_child_nodes(
        ctx,
        &ui_type,
        |c, &ut_want, qid, _aq, nodes, node, cond, ctype| {
            if ctype != "ShowUi" {
                return Ok(false);
            }
            let Some(utype) = cond
                .get("UiType")
                .and_then(Value::as_object)
                .and_then(|o| json_str(o.get("Type")))
            else {
                return Ok(false);
            };
            if utype != ut_want {
                return Ok(false);
            }
            complete_child_quest(c, qid, nodes, node)?;
            Ok(true)
        },
    )?;
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_vision_system_event<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_type: &str,
) -> io::Result<()> {
    if quest_type.is_empty() {
        return Ok(());
    }
    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        let changed = if quest_type == "VisionSetActivated" {
            pc.info.mark_claim("vision:activated")
        } else if quest_type == "DoVisionIdentify" {
            pc.info.mark_claim("vision:identified")
        } else {
            false
        };
        if changed {
            pc.save(ctx.fs)?;
        }
    }
    let progressed = advance_matching_child_nodes(
        ctx,
        &quest_type,
        |c, &q_want, qid, _aq, nodes, node, cond, ctype| {
            if ctype != "VisionSystem" {
                return Ok(false);
            }
            let Some(qt) = cond
                .get("Quest")
                .and_then(Value::as_object)
                .and_then(|o| json_str(o.get("Type")))
            else {
                return Ok(false);
            };
            if qt != q_want {
                return Ok(false);
            }
            complete_child_quest(c, qid, nodes, node)?;
            Ok(true)
        },
    )?;
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_tower_floor_event<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    tower_id: i32,
    stars: i32,
) -> io::Result<()> {
    let clamped_stars = stars.clamp(1, 3);
    let star_index: Vec<i32> = (1..=clamped_stars).collect();

    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        let (diff, area_num) = ctx
            .assets
            .tables
            .tower_config
            .get_data_by_id(&tower_id)
            .map_or((1, 1), |t| (t.Difficulty, t.AreaNum));
        pc.info.record_tower_floor(
            tower_id,
            diff,
            area_num,
            clamped_stars,
            (1 << clamped_stars) - 1,
        );
        pc.save(ctx.fs)?;
    }

    let formation_roles: Vec<TowerRolePb> = ctx
        .scene
        .as_deref()
        .and_then(|scene| {
            let fi = usize::try_from(scene.formation_info.cur_formation).ok()?;
            scene.formation_info.formations.get(fi).map(|f| {
                f.roles
                    .iter()
                    .flatten()
                    .map(|r| TowerRolePb {
                        RoleId: r.role_id,
                        LeaveSkillId: 0,
                        SkillBranchId: 0,
                    })
                    .collect()
            })
        })
        .unwrap_or_default();

    let floor_pb = TowerFloorPb {
        TowerConfigId: tower_id,
        Star: clamped_stars,
        Formation: formation_roles,
        StarIndex: star_index,
        IsQuickPass: false,
    };
    ctx.sink.push(
        TowerFloorUpdateNotify::MSG_ID,
        &TowerFloorUpdateNotify {
            TowerFloors: vec![floor_pb.clone()],
        },
    );
    ctx.sink.push(
        TowerEndNotify::MSG_ID,
        &TowerEndNotify {
            Success: true,
            NeedUpdateSeason: false,
            CurrentFloorData: Some(floor_pb),
        },
    );
    on_condition_event(ctx, QuestEventKind::FinishRoleTower, tower_id, 1)
}

pub fn on_condition_event<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    kind: QuestEventKind,
    target_id: i32,
    value: i32,
) -> io::Result<()> {
    let cond_type = kind.condition_type();
    if kind == QuestEventKind::CompareActorVar {
        if let Some(pc) = ctx.progress_comp.as_deref_mut() {
            for aq in &ctx.quest_comp.info.active_quests {
                pc.info
                    .set_key_int(&format!("actor_var:{}:{target_id}", aq.quest_id), i64::from(value));
            }
            pc.save(ctx.fs)?;
        }
    } else {
        let progressed = advance_matching_child_nodes(
            ctx,
            &(cond_type, target_id, value),
            |c, &(ct, tid, val), qid, _aq, nodes, node, cond, ctype| {
                if ctype != ct {
                    return Ok(false);
                }
                let cfg_target = json_int(cond.get("GameplayId"))
                    .or_else(|| json_int(cond.get("EntityId")))
                    .or_else(|| json_int(cond.get("Id")))
                    .unwrap_or(0);
                if tid > 0 && cfg_target > 0 && cfg_target != tid {
                    return Ok(false);
                }
                let req = json_int(cond.get("Count"))
                    .or_else(|| json_int(cond.get("NeedNum")))
                    .or_else(|| json_int(cond.get("Step")))
                    .unwrap_or(1)
                    .max(1);
                let delta = val.max(1);
                let new_cnt = if let Some(pc) = c.progress_comp.as_deref_mut() {
                    let k = format!("qevt:{ct}:{qid}:{}", node.id);
                    let nc = pc.info.add_key_int(&k, i64::from(delta));
                    pc.save(c.fs)?;
                    nc
                } else {
                    i64::from(req)
                };
                if new_cnt >= i64::from(req) {
                    complete_child_quest(c, qid, nodes, node)?;
                    return Ok(true);
                }
                Ok(false)
            },
        )?;

        if progressed {
            finalize_progression(ctx)?;
        }
    }

    if let Some(lp_kind) = match kind {
        QuestEventKind::FinishFlipper => Some(level_play::LevelPlayEventKind::FinishFlipper),
        QuestEventKind::FinishKurotato => Some(level_play::LevelPlayEventKind::FinishKurotato),
        QuestEventKind::FinishRoleTower => Some(level_play::LevelPlayEventKind::FinishRoleTower),
        QuestEventKind::FinishRollBlock => Some(level_play::LevelPlayEventKind::FinishRollBlock),
        QuestEventKind::FinishSurvivorsRouge => Some(level_play::LevelPlayEventKind::FinishSurvivorsRouge),
        QuestEventKind::FinishTetris => Some(level_play::LevelPlayEventKind::FinishTetris),
        QuestEventKind::FinishTrapDefense => Some(level_play::LevelPlayEventKind::FinishTrapDefense),
        QuestEventKind::CrossSceneSyncKill => Some(level_play::LevelPlayEventKind::CrossSceneSyncKill),
        QuestEventKind::FinishKuroSimpleCombatKill => {
            Some(level_play::LevelPlayEventKind::FinishKuroSimpleCombatKill)
        }
        QuestEventKind::CompareActorVar => Some(level_play::LevelPlayEventKind::CompareActorVar),
        QuestEventKind::NewHandInItems => Some(level_play::LevelPlayEventKind::NewHandInItems),
        QuestEventKind::ParallaxAlign => Some(level_play::LevelPlayEventKind::ParallaxAlign),
        QuestEventKind::PlaybackShowTimeline => {
            Some(level_play::LevelPlayEventKind::PlaybackShowTimeline)
        }
        QuestEventKind::TakePicturesWithTimeScale => {
            Some(level_play::LevelPlayEventKind::TakePicturesWithTimeScale)
        }
        QuestEventKind::WalkingPattern => Some(level_play::LevelPlayEventKind::WalkingPattern),
        _ => None,
    } && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
    {
        level_play::on_condition_event(&mut lp_ctx, lp_kind, target_id, value)?;
    }

    Ok(())
}

pub fn on_gameplay_state_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    key: &str,
) -> io::Result<()> {
    let mut dirty_pc = false;
    if let Some(pc) = ctx.progress_comp.as_deref_mut()
        && !key.is_empty()
        && pc.info.mark_claim(&format!("gp_done:{key}"))
    {
        dirty_pc = true;
    }
    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }

    let progressed = advance_matching_child_nodes(ctx, &key, |c, &k, qid, _aq, nodes, node, cond, ctype| {
        if ctype == "AtomicProcess" {
            let match_key = json_str(cond.get("Key"))
                .or_else(|| json_str(cond.get("GameplayId")))
                .or_else(|| {
                    cond.get("AtomicProcessType")
                        .and_then(Value::as_object)
                        .and_then(|o| json_str(o.get("Type")))
                })
                .unwrap_or("");
            if k.is_empty()
                || k == "AtomicProcess"
                || match_key.is_empty()
                || match_key == k
            {
                if let Some(pc) = c.progress_comp.as_deref_mut() {
                    pc.info.mark_claim(&format!("quest_sub:{qid}:{}", node.id));
                    pc.save(c.fs)?;
                }
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
        } else if ctype == "CheckSpringFestivalGameplayCompleted" {
            let match_key = json_str(cond.get("Key"))
                .or_else(|| json_str(cond.get("GameplayId")))
                .unwrap_or("");
            let gp_id_match = cond
                .get("GameplayType")
                .and_then(Value::as_object)
                .and_then(|o| json_int(o.get("Id")))
                .is_some_and(|id| k == id.to_string() || k == format!("sf:{id}"));
            if k.is_empty()
                || k == "SpringFestival"
                || match_key.is_empty()
                || match_key == k
                || gp_id_match
            {
                if let Some(pc) = c.progress_comp.as_deref_mut() {
                    pc.info.mark_claim(&format!("quest_sub:{qid}:{}", node.id));
                    pc.save(c.fs)?;
                }
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
        }
        Ok(false)
    })?;
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_ui_gameplay_completed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    gameplay_key: &str,
) -> io::Result<()> {
    if gameplay_key.is_empty() {
        return Ok(());
    }
    if let Some(pc) = ctx.progress_comp.as_deref_mut()
        && pc.info.mark_claim(&format!("ui_gp:{gameplay_key}"))
    {
        pc.save(ctx.fs)?;
    }
    let progressed = advance_matching_child_nodes(
        ctx,
        &gameplay_key,
        |c, &gkey, qid, _aq, nodes, node, cond, ctype| {
            if ctype == "CheckUiGame" && matches_check_ui_game_key(cond, gkey) {
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
            Ok(false)
        },
    )?;
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

fn skill_matches_genre(assets: &sonoro_gamesv_data::Assets, skill_id: i32, genre: i32) -> bool {
    if genre == 9 || genre == 10 {
        if assets.tables.phantom_skill.get_data_by_id(&skill_id).is_some()
            || assets.tables.explore_tools.get_data_by_id(&skill_id).is_some()
        {
            return true;
        }
        return assets
            .tables
            .phantom_skill
            .items
            .iter()
            .any(|ps| ps.Id == skill_id || ps.PhantomSkillId == skill_id);
    }
    let Some(sk) = assets.tables.skill.get_data_by_id(&skill_id) else {
        return false;
    };
    match genre {
        0 => sk.SkillType == 1,
        1 => sk.SkillType == 2,
        2 => sk.SkillType == 3,
        3 => sk.SkillType == 4,
        4 => sk.SkillType == 5,
        5 => sk.SkillType == 7,
        6 | 7 | 11 => sk.SkillType == 6,
        12 => sk.SkillType == 12,
        other => sk.SkillType == other,
    }
}

fn matches_use_skill_condition(
    assets: &sonoro_gamesv_data::Assets,
    cond: &serde_json::Map<String, Value>,
    skill_id: i32,
) -> bool {
    let Some(Value::Object(chk)) = cond.get("Check") else {
        return false;
    };
    match json_str(chk.get("Type")).unwrap_or("") {
        "CheckBySkillId" => {
            json_int(chk.get("SkillId")).is_some_and(|wanted| wanted > 0 && wanted == skill_id)
        }
        "CheckBySkillGenre" => json_int(chk.get("SkillGenre"))
            .is_some_and(|genre| skill_matches_genre(assets, skill_id, genre)),
        "CheckVisionShowId" | "CheckVisionSummonId" => {
            let Some(vid) = json_int(chk.get("VisionId")).filter(|&v| v > 0) else {
                return false;
            };
            skill_id == vid
                || assets
                    .tables
                    .phantom_skill
                    .get_data_by_id(&skill_id)
                    .is_some_and(|ps| ps.PhantomSkillId == vid || ps.Id == vid)
        }
        _ => false,
    }
}

pub fn on_skill_used<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    skill_id: i32,
) -> io::Result<()> {
    if skill_id <= 0 {
        return Ok(());
    }
    let progressed =
        advance_matching_child_nodes(ctx, &skill_id, |c, &sid, qid, _aq, nodes, node, cond, ctype| {
            if ctype != "UseSkill" || !matches_use_skill_condition(c.assets, cond, sid) {
                return Ok(false);
            }
            complete_child_quest(c, qid, nodes, node)?;
            Ok(true)
        })?;
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_level_play_completed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    level_play_id: i32,
) -> io::Result<()> {
    if level_play_id > 0 {
        ctx.quest_comp.info.mark_level_play_completed(level_play_id);
    }
    let mut progressed = refresh_available_quests(ctx, true)?;
    if advance_matching_child_nodes(ctx, &(), |c, _, qid, _aq, nodes, node, cond, ctype| {
        if ctype != "CheckLevelPlay" || !is_level_play_satisfied(c, cond) {
            return Ok(false);
        }
        complete_child_quest(c, qid, nodes, node)?;
        Ok(true)
    })? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    } else if level_play_id > 0 {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_skill_unlock<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    skill_id: i32,
) -> io::Result<()> {
    if skill_id <= 0 {
        return Ok(());
    }
    if let Some(pc) = ctx.progress_comp.as_deref_mut()
        && pc.info.mark_claim(&format!("skill_unlocked:{skill_id}"))
    {
        pc.save(ctx.fs)?;
    }
    let mut progressed = complete_matching_child_quests(ctx, "GetSkill", "SkillId", skill_id)?;
    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_information_view_read<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    display_id: i32,
) -> io::Result<()> {
    if display_id <= 0 {
        return Ok(());
    }
    if let Some(pc) = ctx.progress_comp.as_deref_mut()
        && pc.info.mark_claim(&format!("display_read:{display_id}"))
    {
        pc.save(ctx.fs)?;
    }
    let mut progressed =
        complete_matching_child_quests(ctx, "InformationViewCheck", "InformationViewId", display_id)?;
    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_program_special_process_finish<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let progressed = advance_matching_child_nodes(ctx, &(), |c, _, qid, _aq, nodes, node, _cond, ctype| {
        if ctype != "ProgramSpecialProcess" {
            return Ok(false);
        }
        if let Some(pc) = c.progress_comp.as_deref_mut()
            && pc.info.mark_claim(&format!("quest_psp:{qid}:{}", node.id))
        {
            pc.save(c.fs)?;
        }
        complete_child_quest(c, qid, nodes, node)?;
        Ok(true)
    })?;
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_photo_submitted<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    _tree_inc_id: i64,
    node_id: i32,
) -> io::Result<()> {
    if owner_id > 0 && node_id > 0 {
        ctx.quest_comp
            .info
            .mark_photo_node_completed(owner_id, node_id);
    }
    let progressed = advance_matching_child_nodes(
        ctx,
        &(owner_id, node_id),
        |c, &(want_owner, want_nid), qid, _aq, nodes, node, _cond, ctype| {
            if ctype != "TakePhoto" && ctype != "TakePhoto2" {
                return Ok(false);
            }
            if want_owner != 0 && want_owner != qid {
                return Ok(false);
            }
            if want_nid != 0 && want_nid != node.id {
                return Ok(false);
            }
            complete_child_quest(c, qid, nodes, node)?;
            Ok(true)
        },
    )?;
    if progressed {
        finalize_progression(ctx)?;
    } else if owner_id > 0 && node_id > 0 {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn allows_client_submit_node(node: &Value) -> bool {
    let Some(cond) = get_child_condition(node) else {
        return true;
    };
    let Some(ctype) = json_str(cond.get("Type")) else {
        return true;
    };
    !matches!(
        ctype,
        "DoInteract" | "PlayFlow" | "HandInItems" | "Kill" | "MonsterCreator" | "Timer" | "WaitTime"
    )
}

pub fn on_submit_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    tree_inc_id: i64,
    node_id: i32,
) -> io::Result<ErrorCode> {
    let quest_id = if owner_id > 0 {
        owner_id
    } else {
        let Ok(v) = i32::try_from(tree_inc_id) else {
            return Ok(ErrorCode::Success);
        };
        v
    };
    if quest_id <= 0 || node_id <= 0 {
        return Ok(ErrorCode::Success);
    }
    if !ctx
        .quest_comp
        .info
        .find_active_quest(quest_id)
        .is_some_and(|aq| aq.is_node_active(node_id))
    {
        return Ok(ErrorCode::Success);
    }
    let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
        return Ok(ErrorCode::Success);
    };
    let Some(node) = find_node_by_id(&nodes, node_id).cloned() else {
        return Ok(ErrorCode::Success);
    };
    if node.node_type != "ChildQuest" || !allows_client_submit_node(&node.data) {
        return Ok(ErrorCode::Success);
    }
    complete_child_quest(ctx, quest_id, &nodes, &node)?;
    finalize_progression(ctx)?;
    Ok(ErrorCode::Success)
}

#[derive(Default)]
struct FlowEndReplayOutcome {
    next_state_id: i32,
    has_finish_do_interact: bool,
    finish_do_interact_executed: bool,
}

fn is_flow_replay_server_action(name: &str) -> bool {
    matches!(
        name,
        "SetVar"
            | "CalculateVar"
            | "RandomVar"
            | "AccpetCurrentQuest"
            | "OpenSystemFunction"
            | "EnableSystem"
            | "EnableFunction"
            | "UnlockSystemItem"
            | "GetItem"
            | "DestroyQuestItem"
            | "UnlockTeleportTrigger"
            | "EnableLevelPlay"
            | "ChangeEntityState"
            | "ChangeSelfEntityState"
            | "AwakeEntity"
            | "DestroyEntity"
            | "SetEntityVisible"
            | "HideGroup"
            | "ShowHidedGroup"
            | "HideSpecificEntities"
            | "ShowSpecificEntities"
            | "HideTargetRange"
            | "ShowTargetRange"
            | "HideByRangeInFlow"
            | "ShowAllHidedGroupInFlow"
            | "ShowHidedGroupInFlow"
            | "LockEntity"
            | "UnlockEntity"
            | "SetEntityPos"
            | "ResetEntityPos"
            | "SetPlayerPos"
            | "ServerSetPlayerPos"
            | "ChangeEntityCamp"
            | "MoveSceneItem"
            | "MoveWithSpline"
            | "NewMoveWithSpline"
            | "StopNewMoveWithSpline"
            | "NpcMoveToPoint"
            | "AddFlowInteractOption"
            | "RemoveFlowInteractOption"
            | "ModifySceneItemAttributeTag"
            | "SetElementalBoxToTarget"
            | "ChangeElementalBoxState"
            | "ChangeLiftTarget"
            | "AddPlayBubble"
            | "ClearPlayBubble"
            | "RunActions"
            | "FinishState"
            | "AddTrialCharacter"
            | "RemoveTrialCharacter"
            | "UpdateTrialCharacter"
            | "RestoreTrialCharacter"
            | "SheriffUnlockClue"
            | "SheriffUnlockProgress"
            | "SheriffChangeCriminalState"
            | "SheriffChangeCriminalIdentity"
            | "SheriffUnlockEnding"
            | "SetTime"
            | "SetTimeLockState"
            | "SetWeather"
            | "SetWeatherLockState"
            | "SwitchDataLayers"
            | "SwitchSubLevels"
            | "CompleteGuide"
            | "GuideTrigger"
            | "SendNpcMail"
            | "PhoneSystemInteract"
            | "TraceCurrentQuest"
            | "DestroyQuest"
            | "FinishCondition"
            | "ChangeState"
            | "ChangeOtherState"
            | "SetAudioState"
            | "VehicleEnter"
            | "VehicleExitPlayer"
            | "VehicleExitNpc"
            | "TeleportToAndEnterVehicle"
            | "AddGuestCharacter"
            | "RemoveGuestCharacter"
    )
}

fn scan_flow_actions_for_finish_do_interact(items: &[Value]) -> bool {
    for act in items {
        let Some(obj) = act.as_object() else {
            continue;
        };
        let Some(name) = json_str(obj.get("Name")) else {
            continue;
        };
        if name == "FinishDoInteract" {
            return true;
        }
        if name == "RunActions"
            && let Some(Value::Object(p)) = obj.get("Params")
            && let Some(Value::Array(al)) = p.get("ActionList")
            && scan_flow_actions_for_finish_do_interact(al)
        {
            return true;
        }
        if name == "ShowTalk"
            && let Some(Value::Object(p)) = obj.get("Params")
            && let Some(Value::Array(tis)) = p.get("TalkItems")
        {
            for ti in tis {
                let Some(ti_obj) = ti.as_object() else {
                    continue;
                };
                if let Some(Value::Array(iacts)) = ti_obj.get("IntroActions")
                    && scan_flow_actions_for_finish_do_interact(iacts)
                {
                    return true;
                }
                if let Some(Value::Array(tacts)) = ti_obj.get("Actions")
                    && scan_flow_actions_for_finish_do_interact(tacts)
                {
                    return true;
                }
                if let Some(Value::Array(opts)) = ti_obj.get("Options") {
                    for opt in opts {
                        if let Some(opt_obj) = opt.as_object()
                            && let Some(Value::Array(oacts)) = opt_obj.get("Actions")
                            && scan_flow_actions_for_finish_do_interact(oacts)
                        {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn execute_single_flow_replay_action<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    qid: i32,
    nid: i32,
    act: &Value,
    outcome: &mut FlowEndReplayOutcome,
) -> io::Result<()> {
    let Some(obj) = act.as_object() else {
        return Ok(());
    };
    let Some(name) = json_str(obj.get("Name")) else {
        return Ok(());
    };
    let action_id = json_int(obj.get("ActionId")).unwrap_or(0);
    if action_id > 0
        && ctx
            .scene
            .as_deref()
            .is_some_and(|s| s.has_flow_action_executed(action_id))
    {
        return Ok(());
    }
    if name == "FinishDoInteract" {
        outcome.finish_do_interact_executed = true;
        if action_id > 0
            && let Some(scene) = ctx.scene.as_deref_mut()
        {
            scene.mark_flow_action_executed(action_id);
        }
        return Ok(());
    }
    if name == "RunActions" {
        if let Some(Value::Object(p)) = obj.get("Params")
            && let Some(Value::Array(al)) = p.get("ActionList")
        {
            for sub_act in al {
                execute_single_flow_replay_action(ctx, qid, nid, sub_act, outcome)?;
            }
        }
        if action_id > 0
            && let Some(scene) = ctx.scene.as_deref_mut()
        {
            scene.mark_flow_action_executed(action_id);
        }
        return Ok(());
    }
    if !is_flow_replay_server_action(name) {
        return Ok(());
    }
    let mut cbs = super::action_callbacks();
    cbs.is_flow_action = true;
    cbs.defer_flow_transition = true;
    quest_actions::execute_action(ctx, &mut cbs, qid, nid, act)?;
    if action_id > 0
        && let Some(scene) = ctx.scene.as_deref_mut()
    {
        scene.mark_flow_action_executed(action_id);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn replay_flow_state_on_end<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    flow_name: &str,
    flow_id: i32,
    state_id: i32,
    qid: i32,
    nid: i32,
    option_infos: &BTreeMap<i32, FlowOptionInfoList>,
    is_skip: bool,
) -> io::Result<FlowEndReplayOutcome> {
    let mut outcome = FlowEndReplayOutcome::default();
    let Ok(Some(state_val)) = ctx.assets.flow_states.get_flow_state(flow_name, flow_id, state_id)
    else {
        return Ok(outcome);
    };
    let Some(Value::Array(acts)) = state_val.get("Actions") else {
        return Ok(outcome);
    };

    outcome.has_finish_do_interact = scan_flow_actions_for_finish_do_interact(acts);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.flow_next_state_id = 0;
    }

    for act in acts {
        let Some(obj) = act.as_object() else {
            continue;
        };
        let Some(name) = json_str(obj.get("Name")) else {
            continue;
        };
        if name == "ShowTalk" {
            let st_aid = json_int(obj.get("ActionId")).unwrap_or(0);
            let Some(Value::Object(params)) = obj.get("Params") else {
                continue;
            };
            let Some(Value::Array(tis)) = params.get("TalkItems") else {
                continue;
            };

            for ti in tis {
                let Some(ti_obj) = ti.as_object() else {
                    continue;
                };
                let tid = json_int(ti_obj.get("Id")).unwrap_or(0);
                let opts_opt = ti_obj.get("Options").and_then(Value::as_array);
                let has_options = opts_opt.is_some_and(|v| !v.is_empty());

                let mut selected_indices = Vec::new();
                for (&entry_key, entry_val) in option_infos {
                    if st_aid > 0 && entry_key != 0 && entry_key != st_aid {
                        continue;
                    }
                    for opt_sel in &entry_val.OptionIndexList {
                        if opt_sel.TalkId == tid
                            && let Ok(idx) = usize::try_from(opt_sel.OptionIndex)
                        {
                            selected_indices.push(idx);
                        }
                    }
                }

                if !has_options || !selected_indices.is_empty() || is_skip {
                    if let Some(Value::Array(iacts)) = ti_obj.get("IntroActions") {
                        for iact in iacts {
                            execute_single_flow_replay_action(ctx, qid, nid, iact, &mut outcome)?;
                        }
                    }
                    if let Some(Value::Array(tacts)) = ti_obj.get("Actions") {
                        for tact in tacts {
                            execute_single_flow_replay_action(ctx, qid, nid, tact, &mut outcome)?;
                        }
                    }
                }

                if let Some(opts) = opts_opt {
                    for idx in selected_indices {
                        let Some(opt_obj) = opts.get(idx).and_then(Value::as_object) else {
                            continue;
                        };
                        if opt_obj.get("HiddenOption").and_then(Value::as_bool) == Some(true) {
                            continue;
                        }
                        if let Some(pc_val) = opt_obj.get("PreCondition") {
                            let eval_ctx = ctx.make_eval_context_for_quest(qid);
                            if !condition::eval_inline_condition_block(&eval_ctx, pc_val) {
                                continue;
                            }
                        }
                        if let Some(Value::Array(oacts)) = opt_obj.get("Actions") {
                            for oact in oacts {
                                execute_single_flow_replay_action(
                                    ctx,
                                    qid,
                                    nid,
                                    oact,
                                    &mut outcome,
                                )?;
                            }
                        }
                    }
                }
            }
            continue;
        }
        execute_single_flow_replay_action(ctx, qid, nid, act, &mut outcome)?;
    }

    outcome.next_state_id = ctx.scene.as_deref().map_or(0, |s| s.flow_next_state_id);
    Ok(outcome)
}

pub(super) fn try_start_pending_play_flow_for_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> io::Result<bool> {
    let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id).cloned() else {
        return Ok(false);
    };
    if aq.active_nodes.is_empty() {
        return Ok(false);
    }
    let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
        return Ok(false);
    };
    for &nid in &aq.active_nodes {
        let Some(node) = find_node_by_id(&nodes, nid) else {
            continue;
        };
        if node.node_type != "ChildQuest" {
            continue;
        }
        let Some(cond) = get_child_condition(&node.data) else {
            continue;
        };
        if json_str(cond.get("Type")) != Some("PlayFlow") {
            continue;
        }
        if let Some(Value::Object(flow_obj)) = cond.get("Flow") {
            start_flow_from_obj(ctx, quest_id, nid, flow_obj)?;
            if quest_id == PROLOGUE_QUEST_ID
                && let Some(scene) = ctx.scene.as_deref_mut()
            {
                scene.prologue_flow_started = true;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn dispatch_post_flow_followups<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    if ctx
        .scene
        .as_deref()
        .is_none_or(|s| s.active_flow_inc_id != 0)
    {
        return Ok(());
    }

    let next_pending = ctx
        .scene
        .as_deref_mut()
        .and_then(|s| s.pop_pending_flow());
    if let Some(pf) = next_pending {
        start_flow_params(
            ctx,
            pf.quest_id,
            pf.node_id,
            &pf.flow_list_name,
            pf.flow_id,
            pf.state_id,
            false,
        )?;
        if ctx
            .scene
            .as_deref()
            .is_some_and(|s| s.active_flow_inc_id != 0)
        {
            return Ok(());
        }
    }

    let tracked = ctx.quest_comp.info.tracked_quest_id;
    if try_start_pending_play_flow_for_quest(ctx, tracked)? {
        return Ok(());
    }
    let active_snapshot = snapshot_active_quest_ids(ctx);
    for qid in active_snapshot {
        if qid == tracked {
            continue;
        }
        if try_start_pending_play_flow_for_quest(ctx, qid)? {
            return Ok(());
        }
    }

    if ctx
        .scene
        .as_deref()
        .is_some_and(|s| s.active_flow_inc_id != 0)
    {
        return Ok(());
    }
    reevaluate_in_range_triggers(ctx)?;
    let player_loc = ctx.scene.as_deref().and_then(|s| {
        let pid = s.player_id;
        s.instance.players.iter().find(|p| p.id == pid).map(|p| p.location)
    });
    if let Some(loc) = player_loc {
        on_player_move(ctx, loc[0], loc[1], loc[2])?;
    }
    Ok(())
}

fn complete_first_active_play_flow<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
) -> io::Result<bool> {
    let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id).cloned() else {
        return Ok(false);
    };
    let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
        return Ok(false);
    };
    for &nid in &aq.active_nodes {
        let Some(node) = find_node_by_id(&nodes, nid).cloned() else {
            continue;
        };
        if node.node_type != "ChildQuest" {
            continue;
        }
        let Some(cond) = get_child_condition(&node.data) else {
            continue;
        };
        if json_str(cond.get("Type")) != Some("PlayFlow") {
            continue;
        }
        complete_child_quest(ctx, quest_id, &nodes, &node)?;
        return Ok(true);
    }
    Ok(false)
}

pub fn on_flow_end<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    flow_inc_id: i64,
) -> io::Result<()> {
    on_flow_end_with_options(ctx, flow_inc_id, &BTreeMap::new(), false)
}

pub fn on_flow_end_with_options<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    flow_inc_id: i64,
    option_infos: &BTreeMap<i32, FlowOptionInfoList>,
    is_skip: bool,
) -> io::Result<()> {
    let Some((
        matched_qid,
        matched_nid,
        matched_bt_type,
        matched_inc,
        owner_entity_id,
        prev_flow_name,
        prev_flow_id,
        prev_state_id,
    )) = ctx.scene.as_deref().map(|scene| {
        (
            scene.active_flow_quest_id,
            scene.active_flow_node_id,
            scene.active_flow_bt_type,
            scene.active_flow_inc_id,
            scene.active_flow_owner_entity_id,
            scene.active_flow_list_name.clone(),
            scene.active_flow_id,
            scene.active_flow_state_id,
        )
    }) else {
        return Ok(());
    };

    if matched_inc != 0 && (matched_inc == flow_inc_id || flow_inc_id == 0) {
        let replay = if !prev_flow_name.is_empty() && prev_flow_id > 0 && prev_state_id > 0 {
            replay_flow_state_on_end(
                ctx,
                &prev_flow_name,
                prev_flow_id,
                prev_state_id,
                matched_qid,
                matched_nid,
                option_infos,
                is_skip,
            )
            .unwrap_or_default()
        } else {
            FlowEndReplayOutcome::default()
        };

        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.clear_active_flow();
            scene.prologue_flow_started = false;
        }

        if replay.next_state_id > 0 && !prev_flow_name.is_empty() && prev_flow_id > 0 {
            start_flow_params_bt(
                ctx,
                matched_qid,
                matched_nid,
                matched_bt_type,
                &prev_flow_name,
                prev_flow_id,
                replay.next_state_id,
                false,
                false,
            )?;
            if let Some(scene) = ctx.scene.as_deref_mut() {
                scene.active_flow_owner_entity_id = owner_entity_id;
            }
            return Ok(());
        }

        super::triggers::restore_plot_main_role(ctx);
        ctx.sink.push(
            FlowEndNotify::MSG_ID,
            &FlowEndNotify {
                FlowIncId: flow_inc_id,
                Async: false,
                IsFinished: true,
            },
        );
        quest_actions::restore_flow_range_hidden_group(ctx)?;

        let interact_blocked_cancel =
            !is_skip && replay.has_finish_do_interact && !replay.finish_do_interact_executed;
        if matched_qid != 0 && !interact_blocked_cancel {
            if ctx.quest_comp.info.has_active_quest(matched_qid) {
                if let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(matched_qid) {
                    if ctx
                        .quest_comp
                        .info
                        .find_active_quest(matched_qid)
                        .is_some_and(|aq| aq.is_node_active(matched_nid))
                    {
                        if let Some(node) = find_node_by_id(&nodes, matched_nid).cloned() {
                            let mut ready_to_complete = true;
                            if let Some(cond) = get_child_condition(&node.data)
                                && json_str(cond.get("Type")) == Some("DoInteract")
                            {
                                let cfg_id = resolve_config_id(
                                    ctx.scene.as_deref(),
                                    owner_entity_id,
                                );
                                if let Some(matched_opt) =
                                    match_do_interact(cond, owner_entity_id, cfg_id, 0).cloned()
                                {
                                    ready_to_complete = mark_and_check_do_interact_complete(
                                        ctx,
                                        matched_qid,
                                        &node,
                                        cond,
                                        owner_entity_id,
                                        &matched_opt,
                                    )?;
                                }
                            }
                            if ready_to_complete {
                                complete_child_quest(ctx, matched_qid, &nodes, &node)?;
                            }
                            finalize_progression(ctx)?;
                        }
                    } else if let Some(node) = find_node_by_id(&nodes, matched_nid).cloned()
                        && (node.node_type == "Action" || node.node_type == "ActionWithResult")
                    {
                        let akey = action_node_progress_key(matched_qid, matched_nid);
                        let total_actions = count_node_actions(&node);
                        let next_idx = ctx
                            .scene
                            .as_deref()
                            .and_then(|s| s.action_node_next_idx.get(&akey).copied())
                            .unwrap_or(total_actions);
                        if next_idx < total_actions {
                            dispatch_post_flow_followups(ctx)?;
                            let should_advance = ctx.scene.as_deref().is_none_or(|s| {
                                s.active_flow_inc_id == 0
                                    && !s.has_pending_flow_for_node(matched_qid, matched_nid)
                            }) && ctx.quest_comp.info.has_active_quest(matched_qid);
                            if should_advance {
                                advance_quest_tree(ctx, matched_qid, true)?;
                                finalize_progression(ctx)?;
                            }
                            return Ok(());
                        } else if ctx
                            .scene
                            .as_deref()
                            .is_some_and(|s| s.has_pending_flow_for_node(matched_qid, matched_nid))
                        {
                            dispatch_post_flow_followups(ctx)?;
                            return Ok(());
                        }
                        if let Some(scene) = ctx.scene.as_deref_mut() {
                            scene.action_node_next_idx.remove(&akey);
                        }
                        finish_non_child_node(ctx, matched_qid, &node, true)?;
                        advance_quest_tree(ctx, matched_qid, true)?;
                        finalize_progression(ctx)?;
                    }
                }
            } else if matched_bt_type == BtType::BtTypeLevelPlay
                && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
            {
                level_play::on_flow_end(&mut lp_ctx, matched_qid, matched_nid)?;
            }
        }
        dispatch_post_flow_followups(ctx)?;
        return Ok(());
    }

    if flow_inc_id != 0
        && let Some(af) = ctx
            .scene
            .as_deref_mut()
            .and_then(|s| s.take_async_flow(flow_inc_id))
    {
        if !af.flow_list_name.is_empty() && af.flow_id > 0 && af.state_id > 0 {
            let _ = replay_flow_state_on_end(
                ctx,
                &af.flow_list_name,
                af.flow_id,
                af.state_id,
                af.quest_id,
                af.node_id,
                option_infos,
                is_skip,
            );
        }
        ctx.sink.push(
            FlowEndNotify::MSG_ID,
            &FlowEndNotify {
                FlowIncId: flow_inc_id,
                Async: true,
                IsFinished: true,
            },
        );
        return Ok(());
    }

    if matched_inc != 0
        || flow_inc_id != 0
        || (owner_entity_id != 0 && matched_qid == 0)
    {
        return Ok(());
    }

    let tracked = ctx.quest_comp.info.tracked_quest_id;
    if complete_first_active_play_flow(ctx, tracked)? {
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.clear_active_flow();
            scene.prologue_flow_started = false;
        }
        super::triggers::restore_plot_main_role(ctx);
        finalize_progression(ctx)?;
        dispatch_post_flow_followups(ctx)?;
        return Ok(());
    }

    let active_snapshot = snapshot_active_quest_ids(ctx);
    for qid in active_snapshot {
        if complete_first_active_play_flow(ctx, qid)? {
            if let Some(scene) = ctx.scene.as_deref_mut() {
                scene.clear_active_flow();
                scene.prologue_flow_started = false;
            }
            super::triggers::restore_plot_main_role(ctx);
            finalize_progression(ctx)?;
            dispatch_post_flow_followups(ctx)?;
            return Ok(());
        }
    }

    Ok(())
}

pub fn on_flow_select_option<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    flow_inc_id: i64,
    talk_id: i32,
    option_index: i32,
) -> io::Result<()> {
    let Some((flow_name, flow_id, state_id)) = ctx.scene.as_deref_mut().and_then(|scene| {
        if let Some(ae) = scene.find_async_flow(flow_inc_id) {
            Some((ae.flow_list_name.clone(), ae.flow_id, ae.state_id))
        } else if scene.active_flow_inc_id == flow_inc_id && !scene.active_flow_list_name.is_empty()
        {
            Some((
                scene.active_flow_list_name.clone(),
                scene.active_flow_id,
                scene.active_flow_state_id,
            ))
        } else {
            None
        }
    }) else {
        return Ok(());
    };

    let Ok(Some(state_val)) = ctx.assets.flow_states.get_flow_state(&flow_name, flow_id, state_id)
    else {
        return Ok(());
    };
    let Some(Value::Array(actions)) = state_val.get("Actions") else {
        return Ok(());
    };
    for act in actions {
        if json_str(act.get("Name")) != Some("ShowTalk") {
            continue;
        }
        let Some(Value::Object(params)) = act.get("Params") else {
            continue;
        };
        let Some(Value::Array(talk_items)) = params.get("TalkItems") else {
            continue;
        };
        for item in talk_items {
            if json_int(item.get("Id")) != Some(talk_id) {
                continue;
            }
            let Some(Value::Array(options)) = item.get("Options") else {
                continue;
            };
            if let Some(opt) = options.get(option_index as usize)
                && let Some(Value::Array(opt_actions)) = opt.get("Actions")
            {
                let mut cbs = super::action_callbacks::<S>();
                cbs.is_flow_action = true;
                quest_actions::execute_action_list(ctx, &mut cbs, 0, 0, opt_actions)?;
            }
            return Ok(());
        }
    }
    Ok(())
}

pub fn on_give_up_quest<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    tree_inc_id: i64,
    node_id: i32,
) -> io::Result<()> {
    let quest_id = if owner_id > 0 {
        owner_id
    } else {
        let Ok(v) = i32::try_from(tree_inc_id) else {
            return Ok(());
        };
        v
    };
    let mut flow_ids = Vec::new();
    if let Some(scene) = ctx.scene.as_deref_mut() {
        let _ = scene.remove_bt_timer(owner_id, tree_inc_id, "");
    }
    if let Some(ended_inc) = interrupt_active_flow_for_quest(ctx, quest_id) {
        flow_ids.push(ended_inc);
    }
    if quest_id > 0
        && node_id > 0
        && ctx
            .quest_comp
            .info
            .find_active_quest(quest_id)
            .is_some_and(|aq| aq.is_node_active(node_id))
    {
        ctx.sink.push(
            UpdateChildQuestNodeStatusNotify::MSG_ID,
            &UpdateChildQuestNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node_id,
                Status: Some(ChildQuestNodeStatus::Fail),
            },
        );
    }
    if quest_id > 0 && ctx.quest_comp.info.has_active_quest(quest_id) {
        let eff_inc_id = if tree_inc_id != 0 {
            tree_inc_id
        } else {
            i64::from(quest_id)
        };
        ctx.sink.push(
            QuestGiveUpNotify::MSG_ID,
            &QuestGiveUpNotify {
                FlowIncIds: flow_ids,
                QuestId: quest_id,
                TreeIncId: eff_inc_id,
            },
        );
        rollback_quest_to_node(ctx, quest_id, node_id, FailReason::GiveUpFail, false)?;
        finalize_progression(ctx)?;
    } else if quest_id > 0
        && ctx
            .quest_comp
            .info
            .progress
            .entered_level_plays
            .contains(&quest_id)
        && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
    {
        level_play::on_bt_rollback(&mut lp_ctx, quest_id, node_id, FailReason::GiveUpFail)?;
    }
    Ok(())
}

pub fn on_client_bt_rollback<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    tree_inc_id: i64,
    select: i32,
) -> io::Result<()> {
    if let Some(scene) = ctx.scene.as_deref_mut() {
        let _ = scene.remove_bt_timer(owner_id, tree_inc_id, "");
    }
    let target_qid = if owner_id > 0 {
        owner_id
    } else {
        let Ok(v) = i32::try_from(tree_inc_id) else {
            return Ok(());
        };
        v
    };
    if target_qid > 0 && ctx.quest_comp.info.has_active_quest(target_qid) {
        if let Some(scene) = ctx.scene.as_deref_mut()
            && scene.last_server_rollback_quest_id == target_qid
            && (now_millis() - scene.last_server_rollback_ms) < 5000
        {
            scene.last_server_rollback_quest_id = 0;
            return Ok(());
        }
        if select == 1 {
            ctx.sink.push(
                BtRollbackStartNotify::MSG_ID,
                &BtRollbackStartNotify {
                    TreeOwnerId: target_qid,
                    TreeIncId: i64::from(target_qid),
                    IsRollbackSubLevel: false,
                    IsRollbackPos: true,
                },
            );
        }
        rollback_quest_to_node(ctx, target_qid, 0, FailReason::DefaultFail, true)?;
        finalize_progression(ctx)?;
    } else if target_qid > 0
        && ctx
            .quest_comp
            .info
            .progress
            .entered_level_plays
            .contains(&target_qid)
        && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
    {
        level_play::on_bt_rollback_with_select(
            &mut lp_ctx,
            target_qid,
            0,
            FailReason::DefaultFail,
            select,
        )?;
    }
    Ok(())
}

pub fn on_level_sequence_mark_reached<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_config_id: i32,
    mark: &str,
) -> io::Result<()> {
    let progressed = advance_matching_child_nodes(
        ctx,
        &(entity_config_id, mark),
        |c, &(cfg_id, m), qid, _aq, nodes, node, cond, ctype| {
            if ctype != "WaitUntilLevelSequenceReachMark" {
                return Ok(false);
            }
            let eid = json_int(cond.get("EntityId")).unwrap_or(0);
            let want_mark = json_str(cond.get("Mark")).unwrap_or("");
            if (eid == 0 || eid == cfg_id) && (want_mark.is_empty() || want_mark == m) {
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
            Ok(false)
        },
    )?;
    if progressed {
        finalize_progression(ctx)?;
    }
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        level_play::on_level_sequence_mark_reached(&mut lp_ctx, entity_config_id, mark)?;
    }
    Ok(())
}

pub fn on_mail_read<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    mail_id: i32,
) -> io::Result<()> {
    if mail_id <= 0 {
        return Ok(());
    }
    append_unique_int(&mut ctx.quest_comp.info.progress.read_mails, mail_id);
    if complete_matching_child_quests(ctx, "ReadMail", "MailId", mail_id)? {
        finalize_progression(ctx)?;
    } else {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_phone_message_event<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    phone_msg_id: i32,
    telecom_id: i32,
) -> io::Result<()> {
    if phone_msg_id > 0 {
        append_unique_int(
            &mut ctx.quest_comp.info.progress.read_phone_messages,
            phone_msg_id,
        );
    }
    if telecom_id > 0 {
        append_unique_int(
            &mut ctx.quest_comp.info.progress.received_telecoms,
            telecom_id,
        );
    }

    let mut progressed = false;
    if phone_msg_id > 0
        && complete_matching_child_quests(ctx, "ReadPhoneMessage", "PhoneMessageId", phone_msg_id)?
    {
        progressed = true;
    }
    if telecom_id > 0
        && complete_matching_child_quests(ctx, "ReceiveTelecom", "TelecomId", telecom_id)?
    {
        progressed = true;
    }
    if ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID) {
        let eval_ctx = ctx.make_eval_context();
        let phone_qids: Vec<i32> = ctx
            .assets
            .tables
            .quest_data
            .items
            .iter()
            .filter_map(|q| {
                let d = &q.Data;
                (is_eligible_quest(ctx, d, false)
                    && d.DistributeType == "PhoneMessage"
                    && condition::eval_quest_provide_conditions(
                        &eval_ctx,
                        d.ProvideType.as_ref(),
                    ))
                .then_some(d.Id)
            })
            .collect();
        for qid in phone_qids {
            if activate_quest(ctx, qid, true)? {
                progressed = true;
            }
        }
    }
    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    } else {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_phone_message_read<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    msg_id: i32,
) -> io::Result<()> {
    on_phone_message_event(ctx, msg_id, 0)
}

pub fn on_telecom_received<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    telecom_id: i32,
) -> io::Result<()> {
    on_phone_message_event(ctx, 0, telecom_id)
}

pub fn on_npc_mail_sent<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    _mail_id: i32,
) -> io::Result<()> {
    if refresh_available_quests(ctx, true)? {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_time_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    hour: i32,
    minute: i32,
) -> io::Result<()> {
    let Some(_guard) = TimeChangeGuard::try_enter() else {
        return Ok(());
    };
    ctx.quest_comp.info.env.time_lock_hour = hour;
    ctx.quest_comp.info.env.time_lock_minute = minute;

    let cur_day = ctx
        .scene
        .as_deref()
        .map_or(1, |s| s.instance.map_time.day);
    let mut progressed = advance_matching_child_nodes(
        ctx,
        &(hour, minute, cur_day),
        |c, &(h, m, day), qid, _aq, nodes, node, cond, ctype| {
            if ctype == "ScheduleTime" && is_schedule_time_window_match(cond, h, m, day) {
                complete_child_quest(c, qid, nodes, node)?;
                return Ok(true);
            }
            Ok(false)
        },
    )?;
    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    } else {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_weather_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    _weather_id: i32,
) -> io::Result<()> {
    let mut progressed =
        advance_matching_child_nodes(ctx, &(), |c, _, qid, _aq, nodes, node, _cond, ctype| {
            if ctype != "UseWeatherSwitch" {
                return Ok(false);
            }
            complete_child_quest(c, qid, nodes, node)?;
            Ok(true)
        })?;
    if recheck_pending_conditions(ctx)? {
        progressed = true;
    }
    if check_active_quest_failed_monitors(ctx, None)? {
        progressed = true;
    }
    if progressed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_bvb_finish<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    bvb_id: i32,
) -> io::Result<()> {
    if bvb_id > 0 {
        append_unique_int(&mut ctx.quest_comp.info.progress.completed_bvb, bvb_id);
    }
    if complete_matching_child_quests(ctx, "FinishBvbChallenge", "BvbChallengeId", bvb_id)? {
        finalize_progression(ctx)?;
    } else {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_bt_set_timer_info<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    inc_id: i64,
    node_id: i32,
    timer_type: &str,
    second: i32,
    set_type: TimerSetType,
) -> io::Result<()> {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(());
    };
    let now_ms = now_millis();
    let bt = scene.adjust_bt_timer(owner_id, inc_id, node_id, timer_type, now_ms, second, set_type);
    ctx.sink.push(
        UpdateTimerInfoNotify::MSG_ID,
        &UpdateTimerInfoNotify {
            TreeOwnerId: owner_id,
            TreeIncId: inc_id,
            // EndTime in ms, like the server time stamp CountDownTimer.GetRemainTime subtracts.
            TimerInfo: Some(bt.to_proto()),
        },
    );
    Ok(())
}

pub fn on_bt_timer_end<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    inc_id: i64,
    timer_type: &str,
) -> io::Result<()> {
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.remove_bt_timer(owner_id, inc_id, timer_type);
    }
    if owner_id > 0 {
        if let Some(aq) = ctx.quest_comp.info.find_active_quest(owner_id).cloned() {
            let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(owner_id) else {
                return Ok(());
            };
            let mut progressed = false;
            for nid in aq.active_nodes {
                let Some(node) = find_node_by_id(&nodes, nid).cloned() else {
                    continue;
                };
                if node.node_type != "ChildQuest" {
                    continue;
                }
                let Some(cond) = get_child_condition(&node.data) else {
                    continue;
                };
                let ctype = json_str(cond.get("Type")).unwrap_or("");
                if ctype != "Timer" && ctype != "WaitTime" {
                    continue;
                }
                complete_child_quest(ctx, owner_id, &nodes, &node)?;
                progressed = true;
                break;
            }
            if progressed {
                finalize_progression(ctx)?;
            }
        } else if ctx.quest_comp.info.progress.entered_level_plays.contains(&owner_id)
            && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
        {
            let _ = level_play::on_bt_timer_end(&mut lp_ctx, owner_id, timer_type)?;
        }
    }
    Ok(())
}

pub fn on_condition_state_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let mut cond_prog = recheck_pending_conditions(ctx)?;
    let quest_added = refresh_available_quests(ctx, true)?;
    if check_active_quest_failed_monitors(ctx, None)? {
        cond_prog = true;
    }
    if super::tree::REFRESH_LOOP_DEPTH.with(|d| d.get()) == 0
        && super::tree::step_pending_control_nodes(ctx)?
    {
        cond_prog = true;
    }
    if cond_prog || quest_added {
        finalize_progression(ctx)?;
    }
    reevaluate_in_range_triggers(ctx)?;
    reevaluate_condition_listeners(ctx)?;
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        level_play::on_condition_state_changed(&mut lp_ctx)?;
    }
    Ok(())
}

pub fn on_player_transfer_or_leave_inst<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    is_leave_dungeon: bool,
) -> io::Result<()> {
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
            let Some(Value::Object(fc)) = node.data.get("FailedCondition") else {
                continue;
            };
            let is_transfer_fail = json_bool(fc.get("IsTransferFailure")).unwrap_or(false);
            let is_leave_fail = json_bool(fc.get("IsLeaveDungeonFailure")).unwrap_or(false);
            if is_leave_dungeon && is_leave_fail {
                rollback_quest_to_node(ctx, aq.quest_id, node.id, FailReason::LeaveInstFail, false)?;
                changed = true;
                break;
            }
            if is_transfer_fail {
                rollback_quest_to_node(ctx, aq.quest_id, node.id, FailReason::TransferFail, false)?;
                changed = true;
                break;
            }
        }
    }

    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        let entered_snap = lp_ctx.quest_comp.info.progress.entered_level_plays.clone();
        for lpid in entered_snap {
            if lp_ctx.quest_comp.info.completed_level_plays.contains(&lpid) {
                continue;
            }
            let Ok(nodes) = lp_ctx.assets.level_play_nodes.get_owner_nodes(lpid) else {
                continue;
            };
            for node in nodes.iter() {
                if node.node_type != "QuestFailed" {
                    continue;
                }
                if !level_play::is_lp_failed_monitor_active(
                    lp_ctx.progress_comp.as_deref(),
                    lpid,
                    &nodes,
                    node,
                ) {
                    continue;
                }
                let Some(Value::Object(fc)) = node.data.get("FailedCondition") else {
                    continue;
                };
                let is_transfer_fail = json_bool(fc.get("IsTransferFailure")).unwrap_or(false);
                let is_leave_fail = json_bool(fc.get("IsLeaveDungeonFailure")).unwrap_or(false);
                if ((is_leave_dungeon && is_leave_fail) || is_transfer_fail)
                    && level_play::on_fail_timer_due(&mut lp_ctx, lpid, node.id)?
                {
                    changed = true;
                }
                break;
            }
        }
    }

    if changed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_difficulty_changed<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) -> io::Result<()> {
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
            if let Some(Value::Object(fc)) = node.data.get("FailedCondition")
                && json_bool(fc.get("SwitchGameDifficulty")).unwrap_or(false)
            {
                rollback_quest_to_node(ctx, aq.quest_id, node.id, FailReason::DefaultFail, false)?;
                changed = true;
                break;
            }
        }
    }

    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        let entered_snap = lp_ctx.quest_comp.info.progress.entered_level_plays.clone();
        for lpid in entered_snap {
            if lp_ctx.quest_comp.info.completed_level_plays.contains(&lpid) {
                continue;
            }
            let Ok(nodes) = lp_ctx.assets.level_play_nodes.get_owner_nodes(lpid) else {
                continue;
            };
            for node in nodes.iter() {
                if node.node_type != "QuestFailed" {
                    continue;
                }
                if let Some(Value::Object(fc)) = node.data.get("FailedCondition")
                    && json_bool(fc.get("SwitchGameDifficulty")).unwrap_or(false)
                {
                    if level_play::on_fail_timer_due(&mut lp_ctx, lpid, node.id)? {
                        changed = true;
                    }
                    break;
                }
            }
        }
    }

    if changed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_sneak_failed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    owner_id: i32,
    tree_inc_id: i64,
) -> io::Result<()> {
    let target_qid = if owner_id > 0 {
        owner_id
    } else {
        i32::try_from(tree_inc_id).unwrap_or(0)
    };
    let mut changed = false;
    if target_qid > 0 && ctx.quest_comp.info.has_active_quest(target_qid) {
        rollback_quest_to_node(ctx, target_qid, 0, FailReason::SneakTimeoutFail, false)?;
        changed = true;
    } else {
        let aq_snapshot = ctx.quest_comp.info.active_quests.clone();
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
                if let Some(Value::Object(fc)) = node.data.get("FailedCondition")
                    && fc.get("SneakPlayCondition").is_some()
                {
                    rollback_quest_to_node(
                        ctx,
                        aq.quest_id,
                        node.id,
                        FailReason::SneakTimeoutFail,
                        false,
                    )?;
                    changed = true;
                    break;
                }
            }
        }
    }

    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx) {
        let entered_snap = lp_ctx.quest_comp.info.progress.entered_level_plays.clone();
        for lpid in entered_snap {
            if lp_ctx.quest_comp.info.completed_level_plays.contains(&lpid) {
                continue;
            }
            let Ok(nodes) = lp_ctx.assets.level_play_nodes.get_owner_nodes(lpid) else {
                continue;
            };
            for node in nodes.iter() {
                if node.node_type != "QuestFailed" {
                    continue;
                }
                if let Some(Value::Object(fc)) = node.data.get("FailedCondition")
                    && fc.get("SneakPlayCondition").is_some()
                {
                    if level_play::on_fail_timer_due(&mut lp_ctx, lpid, node.id)? {
                        changed = true;
                    }
                    break;
                }
            }
        }
    }

    if changed {
        finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn on_progression_unlock_changed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    on_condition_state_changed(ctx)?;
    quest_actions::push_func_open(ctx);
    let show_list = build_pre_show_quest_list(ctx);
    ctx.sink.push(
        sonoro_proto::pb::QuestShowListNotify::MSG_ID,
        &sonoro_proto::pb::QuestShowListNotify { QuestId: show_list },
    );
    push_ready_quest_list(ctx);
    Ok(())
}

pub fn on_portal_passed<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    portal_config_id: i32,
) -> io::Result<()> {
    if portal_config_id <= 0 {
        return Ok(());
    }
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(());
    };
    scene.last_passed_portal_id = portal_config_id;
    let res = on_condition_state_changed(ctx);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.last_passed_portal_id = 0;
    }
    res
}

pub fn on_random_interact<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    option_index: i32,
) -> io::Result<bool> {
    let (config_id, instance_id) = {
        let Some(scene) = ctx.scene.as_deref() else {
            return Ok(false);
        };
        let Some(&entity_index) = scene.net_id_map.get(&entity_id) else {
            return Ok(false);
        };
        let storage = &scene.entities[entity_index];
        let config_id = storage.config.config_id;
        if config_id <= 0 {
            return Ok(false);
        }
        let Some(interact_state) = storage.world_item.interact.as_ref() else {
            return Ok(false);
        };
        if !interact_state.random_option_indices.contains(&option_index) {
            return Ok(false);
        }
        (config_id, scene.instance_id)
    };

    let Some(level_entity) = ctx.assets.find_level_entity(instance_id, i64::from(config_id)) else {
        return Ok(false);
    };
    let mut components = level_entity.ComponentsData.clone();
    if let Some(template) = ctx.assets.find_template_config(&level_entity.BlueprintType) {
        template.ComponentsData.merge_into(&mut components);
    }
    let Some(interact) = components.InteractComponent else {
        return Ok(false);
    };
    if interact.Disabled.unwrap_or(false) {
        return Ok(false);
    }
    let Some(random) = interact.RandomInteract else {
        return Ok(false);
    };
    let Ok(index) = usize::try_from(option_index) else {
        return Ok(false);
    };
    let Some(opt_entry) = random.Options.get(index) else {
        return Ok(false);
    };
    let Some(option) = &opt_entry.Option else {
        return Ok(false);
    };
    let Some(option_type) = &option.Type else {
        return Ok(false);
    };

    if option_type.Type == "Flow" {
        let Some(flow) = &option_type.Flow else {
            return Ok(false);
        };
        let flow_name = flow.FlowListName.as_deref().unwrap_or("");
        let flow_id = flow.FlowId.unwrap_or(0);
        if flow_name.is_empty() || flow_id <= 0 {
            return Ok(false);
        }
        let state_id = flow.StateId.unwrap_or(1).max(1);
        return start_entity_interact_flow(
            ctx,
            entity_id,
            config_id,
            option_index,
            flow_name,
            flow_id,
            state_id,
            true,
        );
    }

    if option_type.Type == "Actions" {
        for action in &option_type.Actions {
            if action.Name != "PlayFlow" {
                continue;
            }
            let Some(params) = &action.Params else {
                continue;
            };
            let Some(flow) = params.Flow.as_ref().or(params.Flows.as_ref()) else {
                continue;
            };
            let flow_name = flow.FlowListName.as_deref().unwrap_or("");
            let flow_id = flow.FlowId.unwrap_or(0);
            if flow_name.is_empty() || flow_id <= 0 {
                continue;
            }
            let state_id = flow.StateId.unwrap_or(1).max(1);
            return start_entity_interact_flow(
                ctx,
                entity_id,
                config_id,
                option_index,
                flow_name,
                flow_id,
                state_id,
                true,
            );
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn start_entity_interact_flow<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    config_id: i32,
    option_index: i32,
    flow_name: &str,
    flow_id: i32,
    state_id: i32,
    is_random: bool,
) -> io::Result<bool> {
    {
        let Some(scene) = ctx.scene.as_deref() else {
            return Ok(false);
        };
        if scene.active_flow_inc_id != 0 {
            return Ok(false);
        }
    }
    let flow_inc_id = ctx.quest_comp.info.alloc_flow_inc_id();
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.set_active_flow(flow_inc_id, 0, 0, flow_name, flow_id, state_id);
        scene.active_flow_owner_entity_id = entity_id;
    }
    super::triggers::prepare_flow_state_entities(ctx, flow_name, flow_id, state_id);
    let entity_ctx = Some(sonoro_proto::pb::EntityCtxPb {
        ConfigId: config_id,
        IncId: entity_id,
    });
    let game_ctx = if is_random {
        sonoro_proto::pb::GameCtxPb {
            CtxType: Some(sonoro_proto::enums::GameCtxType::RandomInteract),
            CtxInfo: Some(sonoro_proto::pb::GameCtxPbCtxInfoOneof::RandomInteract(
                sonoro_proto::pb::RandomInteractCtxPb {
                    EntityCtx: entity_ctx,
                    OptionIndex: option_index,
                },
            )),
        }
    } else {
        sonoro_proto::pb::GameCtxPb {
            CtxType: Some(sonoro_proto::enums::GameCtxType::NormalInteract),
            CtxInfo: Some(sonoro_proto::pb::GameCtxPbCtxInfoOneof::NormalInteract(
                sonoro_proto::pb::NormalInteractCtxPb {
                    EntityCtx: entity_ctx,
                    OptionIndex: option_index,
                },
            )),
        }
    };
    ctx.sink.push(
        sonoro_proto::pb::FlowStartNotify::MSG_ID,
        &sonoro_proto::pb::FlowStartNotify {
            FlowIncId: flow_inc_id,
            FlowListName: flow_name.as_bytes().to_vec(),
            FlowId: flow_id,
            StateId: state_id,
            GameCtx: Some(game_ctx),
            ..Default::default()
        },
    );
    Ok(true)
}

pub fn on_forced_occupation<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    tree_inc_id: i64,
) -> io::Result<()> {
    let target_owner = i32::try_from(tree_inc_id).unwrap_or(0);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene
            .bt_occupations
            .retain(|occ| occ.inc_id == tree_inc_id || occ.owner_id == target_owner);
    }
    if target_owner > 0 && ctx.quest_comp.info.has_active_quest(target_owner) {
        advance_quest_tree(ctx, target_owner, true)?;
        finalize_progression(ctx)?;
    } else if target_owner > 0
        && ctx
            .quest_comp
            .info
            .progress
            .entered_level_plays
            .contains(&target_owner)
        && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(ctx)
    {
        level_play::step_entered_tree_by_id(&mut lp_ctx, target_owner)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detect_combat_state_is_client_judged_unlike_detect_combat_state2() {
        assert!(is_client_judged_condition("DetectCombatState"));
        assert!(!is_client_judged_condition("DetectCombatState2"));
        assert!(!is_client_judged_condition("WaitBattleCondition"));
        assert!(!is_client_judged_condition("Kill"));
    }

    #[test]
    fn combat_state2_holds_requires_listed_states_and_respects_count_and_fallback() {
        let one_of_two = json!({
            "Type": "DetectCombatState2",
            "Count": 1,
            "Conditions": [
                { "EntityId": 122000452, "State": "hacked" },
                { "EntityId": 122000453, "State": "hacked" }
            ]
        });
        let all_of_two = json!({
            "Type": "DetectCombatState2",
            "Count": 0,
            "Conditions": [
                { "EntityId": 122000452, "State": "hacked" },
                { "EntityId": 122000453, "State": "hacked" }
            ]
        });
        let empty_list = json!({
            "Type": "DetectCombatState2",
            "Count": 1,
            "Conditions": []
        });
        let no_list = json!({
            "Type": "DetectCombatState2",
            "EntityId": 310000014,
            "State": "phase two"
        });
        let fallback_cond = json!({
            "Type": "DetectCombatState2",
            "Count": 0,
            "Conditions": [{ "State": "hacked" }]
        });

        let has_122000453 = |eid: i32, st: &str| eid == 122000453 && st == "hacked";
        let nobody = |_: i32, _: &str| false;
        let has_122000452 = |eid: i32, st: &str| eid == 122000452 && st == "hacked";
        let boss_p2 = |eid: i32, st: &str| eid == 310000014 && st == "phase two";

        assert!(combat_state2_holds(one_of_two.as_object().unwrap(), 0, has_122000453));
        assert!(!combat_state2_holds(one_of_two.as_object().unwrap(), 0, nobody));
        assert!(!combat_state2_holds(all_of_two.as_object().unwrap(), 0, has_122000453));
        assert!(!combat_state2_holds(empty_list.as_object().unwrap(), 310000014, boss_p2));
        assert!(!combat_state2_holds(no_list.as_object().unwrap(), 310000014, boss_p2));
        assert!(combat_state2_holds(fallback_cond.as_object().unwrap(), 122000452, has_122000452));
        assert!(!combat_state2_holds(fallback_cond.as_object().unwrap(), 7, has_122000452));
    }

    #[test]
    fn entity_kill_relevance_does_not_match_unrelated_active_quests() {
        let mut aq = sonoro_gamesv_fs::quest_info::ActiveQuest {
            quest_id: 140_000_001,
            ..Default::default()
        };
        aq.mark_node_active(2);

        let unrelated_talk = sonoro_gamesv_data::lazy_node_store::NodeEntry {
            id: 2,
            parent_id: 1,
            node_type: "ChildQuest".to_string(),
            count: 0,
            reward_id: 0,
            data: json!({
                "Condition": {
                    "Type": "Talk",
                    "FlowListName": "TestFlow"
                }
            }),
        };
        let empty_mc = sonoro_gamesv_data::lazy_node_store::NodeEntry {
            id: 2,
            parent_id: 1,
            node_type: "ChildQuest".to_string(),
            count: 0,
            reward_id: 0,
            data: json!({
                "Condition": {
                    "Type": "MonsterCreator",
                    "MonsterCreatorEntityIds": []
                }
            }),
        };
        let matched_kill = sonoro_gamesv_data::lazy_node_store::NodeEntry {
            id: 2,
            parent_id: 1,
            node_type: "ChildQuest".to_string(),
            count: 0,
            reward_id: 0,
            data: json!({
                "Condition": {
                    "Type": "Kill",
                    "EntityId": 109_750_023
                }
            }),
        };

        assert!(!is_entity_kill_relevant_to_quest(&aq, &[unrelated_talk], 109_750_023));
        assert!(!is_entity_kill_relevant_to_quest(&aq, &[empty_mc], 109_750_023));
        assert!(is_entity_kill_relevant_to_quest(&aq, &[matched_kill], 109_750_023));
    }
}
