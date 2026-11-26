use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::io;

use serde_json::{Map, Value};
use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::GameConfig;
use sonoro_gamesv_data::asset_indexes::{entity_world_pos, entity_world_rot};
use sonoro_gamesv_data::lazy_flow_store;
use sonoro_gamesv_data::lazy_node_store::NodeEntry;
use sonoro_gamesv_data::tables::entity_components::extra::ClientTriggerComponent;
use sonoro_gamesv_data::tables::entity_components::range::ShapeConfig;
use sonoro_gamesv_data::tables::entity_components::trigger::TriggerComponent;
use sonoro_gamesv_data::tables::quest_data::DataEntry as QuestDataEntry;
use sonoro_gamesv_fs::guide_info::GuideInfo;
use sonoro_gamesv_fs::inventory_info::InventoryInfo;
use sonoro_gamesv_fs::quest_info::{ActiveQuest, QuestInfo};
use sonoro_gamesv_fs::special_item_incr::TRIAL_INCR_MIN;
use sonoro_proto::enums::{
    BtType, ChildQuestNodeStatus, EAttributeType, GameCtxType, NodeStatus, QuestFocusReason,
    QuestState,
};
use sonoro_proto::pb::{
    ActionGroupNodeActionCtxPb, BehaviorTreeCtxPb, BehaviorTreeInfoNotify,
    ChildQuestNodeEnterActionCtxPb, ChildQuestNodeFinishActionCtxPb, ChildQuestNodeInfo,
    ChildQuestNodeProgress, ChildQuestNodeProgressProgressOneof, CompleteInstProgress,
    DailyQuestStateNotify, DailyQuestUnlockAreaNotify, EntityAddDynamicInteractNotify,
    EntityAddNotify, EntityRemoveDynamicInteractNotify, EntityRemoveInfo, EntityRemoveNotify,
    EntityStateProgress, FightRoleInfo, FightRoleInfos, FlowStartNotify, FocusQuestSetNotify,
    GameCtxPb, GameCtxPbCtxInfoOneof, GetItemCount, GetItemProgress, GroupFormation,
    InteractProgress, KillProgress, LevelEventNotify, NodeInfo, NodeInfoExtraInfoOneof,
    QuestBranchUpdateNotify, QuestFinishListNotify, QuestInfo as PbQuestInfo,
    QuestListNotify, QuestReadyListNotify, QuestShowListNotify, SuccessNodeActionCtxPb,
    TimerInfoPb, TraceQuestNotify, TreeInfo, UpdateChildQuestNodeStatusNotify,
    UpdateGroupFormationNotify, UpdateNodeProgressNotify, UpdateNodeStatusNotify,
    UseItemProgress,
};
use sonoro_proto::wire::PbEnum;

use crate::component::comp_util;
use crate::component::player::PlayerProgressComponent;
use crate::helpers::condition;
use crate::helpers::entity_proto;
use crate::helpers::quest_actions;
use crate::helpers::quest_types::{
    QuestContext, json_bool, json_float, json_int, json_str, resolve_var_int_for_owner,
};
use crate::helpers::role;
use crate::scene::{DynamicInteractKind, Scene, now_millis};
use crate::sink::PushSink;
use crate::templates::role_entity_templates;

use super::{PROLOGUE_QUEST_ID, action_callbacks, events, extract_node_target_entity_ids, tree};

thread_local! {
    static REEVALUATING_RANGE_TRIGGERS: Cell<bool> = const { Cell::new(false) };
    static EVALUATING_CONDITION_LISTENERS: Cell<bool> = const { Cell::new(false) };
    static CHECKING_AUTO_INTERACTS: Cell<bool> = const { Cell::new(false) };
}

struct BoolResetGuard(&'static std::thread::LocalKey<Cell<bool>>);

impl BoolResetGuard {
    fn acquire(key: &'static std::thread::LocalKey<Cell<bool>>) -> Option<Self> {
        key.with(|flag| {
            if flag.get() {
                None
            } else {
                flag.set(true);
                Some(Self(key))
            }
        })
    }
}

impl Drop for BoolResetGuard {
    fn drop(&mut self) {
        self.0.with(|flag| flag.set(false));
    }
}

pub fn execute_actions<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    actions: &[Value],
) -> io::Result<()> {
    let mut cbs = action_callbacks::<S>();
    quest_actions::execute_action_list(ctx, &mut cbs, quest_id, node_id, actions)
}

pub fn execute_entity_actions<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    cfg_id: i32,
    actions: &[Value],
) -> io::Result<()> {
    if actions.is_empty() {
        return Ok(());
    }

    let prev_owner = if let Some(scene) = ctx.scene.as_deref_mut() {
        let mut owner_nid = entity_id;
        let prev = scene.active_flow_owner_entity_id;
        if scene.entity(owner_nid).is_none()
            && let Some(found) = scene.find_entity_net_id_by_config_id(cfg_id)
        {
            owner_nid = found;
        }
        scene.active_flow_owner_entity_id = owner_nid;
        prev
    } else {
        0
    };

    let res = execute_actions(ctx, 0, 0, actions);
    if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.active_flow_owner_entity_id = prev_owner;
    }
    res?;
    ctx.quest_comp.save(ctx.fs)?;
    Ok(())
}

pub fn handle_entity_range_trigger<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    cfg_id: i32,
    trig: &TriggerComponent,
    is_enter: bool,
) -> io::Result<bool> {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(false);
    };
    let state = scene.player_range_states.entry(entity_id).or_default();

    let mut progress_dirty = false;
    if is_enter {
        state.in_range = true;
        if !state.enter_fired {
            let max_times = trig.MaxTriggerTimes.unwrap_or(0);
            let mut can_fire = true;
            let mut cnt_key = String::new();
            let mut cur_cnt = 0i64;
            if max_times > 0
                && cfg_id > 0
                && let Some(pc) = ctx.progress_comp.as_deref()
            {
                cnt_key = format!("trig:cnt:{cfg_id}");
                cur_cnt = pc.info.get_key_int(&cnt_key);
                if cur_cnt >= i64::from(max_times) {
                    can_fire = false;
                }
            }
            if can_fire
                && execute_trigger_actions(
                    ctx,
                    entity_id,
                    cfg_id,
                    &trig.Condition,
                    &trig.Actions,
                )?
            {
                if let Some(scene) = ctx.scene.as_deref_mut()
                    && let Some(st) = scene.player_range_states.get_mut(&entity_id)
                {
                    st.enter_fired = true;
                }
                if max_times > 0
                    && cfg_id > 0
                    && let Some(pc) = ctx.progress_comp.as_deref_mut()
                {
                    pc.info.set_key_int(&cnt_key, cur_cnt + 1);
                    progress_dirty = true;
                }
            }
        }
    } else {
        let was_in_range = state.in_range;
        state.in_range = false;
        state.enter_fired = false;
        if was_in_range && let Value::Object(exit_obj) = &trig.ExitConfig {
            let exit_cond = exit_obj.get("Condition").unwrap_or(&Value::Null);
            let exit_acts = exit_obj.get("Actions").unwrap_or(&Value::Null);
            let _ = execute_trigger_actions(ctx, entity_id, cfg_id, exit_cond, exit_acts)?;
        }
    }
    Ok(progress_dirty)
}

pub fn reevaluate_in_range_triggers<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let Some(_guard) = BoolResetGuard::acquire(&REEVALUATING_RANGE_TRIGGERS) else {
        return Ok(());
    };
    let pending_ids: Vec<i64> = match ctx.scene.as_deref() {
        Some(scene) if !scene.player_range_states.is_empty() => scene
            .player_range_states
            .iter()
            .filter_map(|(&eid, st)| (st.in_range && !st.enter_fired).then_some(eid))
            .collect(),
        _ => return Ok(()),
    };
    if pending_ids.is_empty() {
        return Ok(());
    }

    let mut dirty = false;
    for eid in pending_ids {
        let (instance_id, resolved) = match ctx.scene.as_deref() {
            Some(scene) => (
                scene.instance_id,
                scene
                    .find_config_id_by_net_id(eid)
                    .map_or(eid, i64::from),
            ),
            None => break,
        };
        let Ok(cfg_id) = i32::try_from(resolved) else {
            continue;
        };
        let Some(lcfg) = ctx.assets.find_level_entity(instance_id, resolved) else {
            continue;
        };
        let mut components = lcfg.ComponentsData.clone();
        if let Some(tpl) = ctx.assets.find_template_config(&lcfg.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut components);
        }
        let Some(trig) = components.TriggerComponent.as_ref() else {
            continue;
        };
        if trig.Disabled.unwrap_or(false) {
            continue;
        }
        if handle_entity_range_trigger(ctx, eid, cfg_id, trig, true)? {
            dirty = true;
        }
    }
    if dirty && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    Ok(())
}

pub fn execute_trigger_actions<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    cfg_id: i32,
    cond_val: &Value,
    actions_val: &Value,
) -> io::Result<bool> {
    let Value::Array(items) = actions_val else {
        return Ok(false);
    };
    if items.is_empty() {
        return Ok(false);
    }
    if cond_val.is_object() {
        let mut eval_ctx = ctx.make_eval_context();
        eval_ctx.self_entity_config_id = cfg_id;
        if !condition::eval_inline_condition_block(&eval_ctx, cond_val) {
            return Ok(false);
        }
    }
    execute_entity_actions(ctx, entity_id, cfg_id, items)?;
    Ok(true)
}

pub fn handle_client_trigger_actions<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    entity_id: i64,
    cfg_id: i32,
    ct: &ClientTriggerComponent,
    is_enter: bool,
) -> io::Result<()> {
    if ct.Disabled.unwrap_or(false) {
        return Ok(());
    }
    let block_val = if is_enter {
        ct.OnTriggerEnter.as_ref()
    } else {
        ct.OnTriggerExit.as_ref()
    };
    let Some(Value::Object(block)) = block_val else {
        return Ok(());
    };
    let Some(Value::Array(items)) = block.get("Actions") else {
        return Ok(());
    };
    if items.is_empty() {
        return Ok(());
    }
    execute_entity_actions(ctx, entity_id, cfg_id, items)
}

pub fn reevaluate_condition_listeners<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let Some(_guard) = BoolResetGuard::acquire(&EVALUATING_CONDITION_LISTENERS) else {
        return Ok(());
    };
    let (instance_id, snaps): (i32, Vec<(i64, i32)>) = match ctx.scene.as_deref() {
        Some(scene) if !scene.entities.is_empty() => (
            scene.instance_id,
            scene
                .entities
                .iter()
                .filter_map(|s| (s.config.config_id > 0).then_some((s.net_id, s.config.config_id)))
                .collect(),
        ),
        _ => return Ok(()),
    };

    for (net_id, cfg_id) in snaps {
        let Some(lcfg) = ctx.assets.find_level_entity(instance_id, i64::from(cfg_id)) else {
            continue;
        };
        let mut components = lcfg.ComponentsData.clone();
        if let Some(tpl) = ctx.assets.find_template_config(&lcfg.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut components);
        }
        let Some(cl) = components.ConditionListenerComponent.as_ref() else {
            continue;
        };
        if cl.Disabled.unwrap_or(false) {
            continue;
        }
        let Value::Array(listeners) = &cl.Listeners else {
            continue;
        };
        if listeners.is_empty() {
            continue;
        }

        for (idx, listener_val) in listeners.iter().enumerate() {
            let Value::Object(lobj) = listener_val else {
                continue;
            };
            let Some(cond_val) = lobj.get("Condition") else {
                continue;
            };
            let Some(Value::Array(acts)) = lobj.get("Actions") else {
                continue;
            };
            if acts.is_empty() {
                continue;
            }
            let dedupe_key = ((cfg_id as u32 as u64) << 32) | ((idx as u32) as u64);
            let met = {
                let mut eval_ctx = ctx.make_eval_context();
                eval_ctx.self_entity_config_id = cfg_id;
                condition::eval_inline_condition_block(&eval_ctx, cond_val)
            };
            if met {
                let already_fired = ctx
                    .scene
                    .as_deref_mut()
                    .is_some_and(|sc| !sc.fired_condition_listeners.insert(dedupe_key));
                if already_fired {
                    continue;
                }
                execute_entity_actions(ctx, net_id, cfg_id, acts)?;
            } else if let Some(sc) = ctx.scene.as_deref_mut() {
                sc.fired_condition_listeners.remove(&dedupe_key);
            }
        }
    }
    Ok(())
}

pub fn check_auto_interact_triggers<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    pos: [f32; 3],
) -> io::Result<()> {
    let Some(_guard) = BoolResetGuard::acquire(&CHECKING_AUTO_INTERACTS) else {
        return Ok(());
    };
    let (instance_id, snaps): (i32, Vec<(i64, i32, [f32; 3])>) = match ctx.scene.as_deref() {
        Some(scene) if !scene.entities.is_empty() => (
            scene.instance_id,
            scene
                .entities
                .iter()
                .filter_map(|s| {
                    if s.config.config_id <= 0
                        || s.config.entity_type == crate::component::entity::small::EntityType::Player
                    {
                        return None;
                    }
                    s.position
                        .map(|p| (s.net_id, s.config.config_id, p.location))
                })
                .collect(),
        ),
        _ => return Ok(()),
    };

    for (net_id, cfg_id, ent_pos) in snaps {
        if !ctx
            .scene
            .as_deref()
            .is_some_and(|sc| sc.entity(net_id).is_some())
        {
            continue;
        }
        let Some(lcfg) = ctx.assets.find_level_entity(instance_id, i64::from(cfg_id)) else {
            continue;
        };
        let mut components = lcfg.ComponentsData.clone();
        if let Some(tpl) = ctx.assets.find_template_config(&lcfg.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut components);
        }
        let Some(ic) = components.InteractComponent.as_ref() else {
            continue;
        };
        let is_auto = ic.DoIntactType.as_deref() == Some("Auto")
            || ic
                .Options
                .iter()
                .any(|o| o.DoIntactType.as_deref() == Some("Auto"));
        if ic.Disabled.unwrap_or(false) || !is_auto {
            continue;
        }
        let r = ic.Range.filter(|&v| v > 0).unwrap_or(150) as f32;
        let dx = pos[0] - ent_pos[0];
        let dy = pos[1] - ent_pos[1];
        let dz = pos[2] - ent_pos[2];
        let in_range = (dx * dx + dy * dy + dz * dz) <= r * r;

        let changed = if let Some(scene) = ctx.scene.as_deref_mut() {
            let was_in_range = scene.auto_interact_states.get(&net_id).copied().unwrap_or(false);
            if in_range == was_in_range {
                false
            } else {
                scene.auto_interact_states.insert(net_id, in_range);
                true
            }
        } else {
            false
        };
        if !changed || !in_range {
            continue;
        }

        for (opt_idx, opt) in ic.Options.iter().enumerate() {
            if let Some(opt_cond) = &opt.Condition {
                let pass = {
                    let mut eval_ctx = ctx.make_eval_context();
                    eval_ctx.self_entity_config_id = cfg_id;
                    condition::eval_inline_condition_block(&eval_ctx, opt_cond)
                };
                if !pass {
                    continue;
                }
            }
            if events::on_interact_handled(ctx, net_id, opt_idx as i32)? {
                break;
            }
        }
    }
    Ok(())
}

pub fn complete_guide_by_id<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    guide_id: i32,
) -> io::Result<()> {
    if guide_id <= 0 {
        return Ok(());
    }
    if let Some(gc) = ctx.guide_comp.as_deref_mut() {
        gc.info.add_finished(guide_id);
        comp_util::save_struct(
            ctx.fs,
            &gc.info,
            &format!("player/{}/{}", gc.player_id, GuideInfo::DATA_PATH),
        )?;
    }
    let _ = tree::complete_matching_child_quests(ctx, "Guide", "GuideGroupId", guide_id)?;
    Ok(())
}

pub fn on_guide_finish<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    guide_group_id: i32,
) -> io::Result<()> {
    if guide_group_id <= 0 {
        return Ok(());
    }
    if tree::complete_matching_child_quests(ctx, "Guide", "GuideGroupId", guide_group_id)? {
        tree::finalize_progression(ctx)?;
    }
    Ok(())
}

pub fn action_node_progress_key(quest_id: i32, node_id: i32) -> u64 {
    ((quest_id as u32 as u64) << 32) | (node_id as u32 as u64)
}

pub fn extract_node_action_items<'a>(data: &'a Value, buf: &'a mut [Value; 1]) -> &'a [Value] {
    let Value::Object(obj) = data else {
        return &[];
    };
    if let Some(Value::Array(acts)) = obj.get("Actions") {
        return acts.as_slice();
    }
    if let Some(single @ Value::Object(_)) = obj.get("Action") {
        buf[0] = single.clone();
        return &buf[..1];
    }
    &[]
}

pub fn count_node_actions(node: &NodeEntry) -> usize {
    let mut single_buf = [Value::Null];
    extract_node_action_items(&node.data, &mut single_buf).len()
}

pub fn is_play_flow_action_async(act: &Value) -> bool {
    let Value::Object(obj) = act else {
        return false;
    };
    if let Some(b) = json_bool(obj.get("Async")) {
        return b;
    }
    if let Some(Value::Object(params)) = obj.get("Params")
        && let Some(b) = json_bool(params.get("Async"))
    {
        return b;
    }
    false
}

pub fn is_sync_play_flow_action(act: &Value) -> bool {
    let Value::Object(obj) = act else {
        return false;
    };
    json_str(obj.get("Name")) == Some("PlayFlow") && !is_play_flow_action_async(act)
}

pub fn has_async_play_flow_action(items: &[Value]) -> bool {
    let mut saw_play_flow = false;
    for act in items {
        let Value::Object(obj) = act else {
            continue;
        };
        if json_str(obj.get("Name")) != Some("PlayFlow") {
            continue;
        }
        saw_play_flow = true;
        if !is_play_flow_action_async(act) {
            return false;
        }
    }
    saw_play_flow
}

pub fn start_flow_from_obj<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    obj: &Map<String, Value>,
) -> io::Result<()> {
    start_flow_with_mode(ctx, quest_id, node_id, obj, true, None)
}

pub fn start_flow_with_mode<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    obj: &Map<String, Value>,
    enqueue_if_busy: bool,
    is_async_override: Option<bool>,
) -> io::Result<()> {
    let Some(flow_name) = json_str(obj.get("FlowListName")) else {
        return Ok(());
    };
    if flow_name.is_empty() {
        return Ok(());
    }
    let flow_id = json_int(obj.get("FlowId")).unwrap_or(1);
    let state_id = json_int(obj.get("StateId")).unwrap_or(1);
    let is_async = is_async_override.unwrap_or_else(|| json_bool(obj.get("Async")).unwrap_or(false));
    let bt_type = if quest_id > 0
        && ctx.quest_comp.info.find_active_quest(quest_id).is_none()
        && ctx.assets.tables.level_play_data.get_data_by_id(&quest_id).is_some()
    {
        BtType::BtTypeLevelPlay
    } else {
        BtType::BtTypeQuest
    };
    start_flow_params_bt(
        ctx,
        quest_id,
        node_id,
        bt_type,
        flow_name,
        flow_id,
        state_id,
        enqueue_if_busy,
        is_async,
    )
}

pub fn start_flow_params<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    flow_name: &str,
    flow_id: i32,
    state_id: i32,
    enqueue_if_busy: bool,
) -> io::Result<()> {
    let bt_type = if quest_id > 0
        && ctx.quest_comp.info.find_active_quest(quest_id).is_none()
        && ctx.assets.tables.level_play_data.get_data_by_id(&quest_id).is_some()
    {
        BtType::BtTypeLevelPlay
    } else {
        BtType::BtTypeQuest
    };
    start_flow_params_bt(
        ctx,
        quest_id,
        node_id,
        bt_type,
        flow_name,
        flow_id,
        state_id,
        enqueue_if_busy,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn start_flow_params_bt<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    bt_type: BtType,
    flow_name: &str,
    flow_id: i32,
    state_id: i32,
    enqueue_if_busy: bool,
    is_async: bool,
) -> io::Result<()> {
    if let Some(scene) = ctx.scene.as_deref_mut()
        && !is_async
        && scene.active_flow_inc_id != 0
    {
        if enqueue_if_busy {
            scene.enqueue_pending_flow(quest_id, node_id, flow_name, flow_id, state_id);
        }
        return Ok(());
    }

    let inc_id = ctx.quest_comp.info.alloc_flow_inc_id();
    if !is_async {
        if let Some(scene) = ctx.scene.as_deref_mut() {
            scene.set_active_flow(inc_id, quest_id, node_id, flow_name, flow_id, state_id);
            scene.active_flow_bt_type = bt_type;
            prepare_flow_state_entities(ctx, flow_name, flow_id, state_id);
        }
    } else if let Some(scene) = ctx.scene.as_deref_mut() {
        scene.record_async_flow(inc_id, quest_id, node_id, bt_type, flow_name, flow_id, state_id, 0);
        prepare_flow_state_entities(ctx, flow_name, flow_id, state_id);
    }

    let bt_inc_id = if quest_id != 0 {
        i64::from(quest_id)
    } else {
        inc_id
    };
    ctx.sink.push(
        FlowStartNotify::MSG_ID,
        &FlowStartNotify {
            FlowIncId: inc_id,
            FlowListName: flow_name.as_bytes().to_vec(),
            FlowId: flow_id,
            StateId: state_id,
            aAsync: is_async,
            GameCtx: Some(GameCtxPb {
                CtxType: Some(GameCtxType::BehaviorTree),
                CtxInfo: Some(GameCtxPbCtxInfoOneof::BehaviorTree(BehaviorTreeCtxPb {
                    IncId: bt_inc_id,
                    BtType: Some(bt_type),
                    BtId: quest_id,
                    NodeId: node_id,
                })),
            }),
            ..Default::default()
        },
    );
    Ok(())
}

pub fn prepare_flow_state_entities<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    flow_name: &str,
    flow_id: i32,
    state_id: i32,
) {
    let Ok(Some(state_val)) = ctx.assets.flow_states.get_flow_state(flow_name, flow_id, state_id)
    else {
        return;
    };

    let mut awake_ids = Vec::new();
    lazy_flow_store::collect_awake_entity_ids(&state_val, &mut awake_ids);
    for &eid in &awake_ids {
        quest_actions::ensure_quest_entity_awakened(ctx, eid);
    }
    if !awake_ids.is_empty() {
        let _ = quest_actions::send_quest_entity_visibility(ctx, &awake_ids, true);
    }

    let mut entity_ids = Vec::new();
    lazy_flow_store::collect_set_entity_visible_ids(&state_val, &mut entity_ids);
    for eid in entity_ids {
        quest_actions::ensure_quest_entity_spawned(ctx, eid);
    }

    if lazy_flow_store::does_flow_switch_main_role(&state_val) {
        ensure_plot_main_role(ctx);
    }
}

fn push_formation_group_update<S: PushSink + ?Sized>(
    sink: &mut S,
    scene: &Scene,
    cur_form: &sonoro_gamesv_fs::formation_info::Formation,
) {
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
                    LivingStatus: Some(role::formation_living_status(scene, cur_form)),
                    IsFixedLocation: false,
                }],
                CurrentGroupType: 1,
            }],
        },
    );
}

fn set_formation_visibility(scene: &mut Scene, desired_role_id: i32) {
    for s in scene.entities.iter_mut() {
        if s.config.entity_type != crate::component::entity::small::EntityType::Player
            || s.config.config_type != crate::component::entity::small::ConfigType::Character
        {
            continue;
        }
        s.visible = s.config.config_id == desired_role_id;
    }
}

fn remove_role_entity_and_concomitants<S: PushSink + ?Sized>(
    fs: &FileSystem,
    sink: &mut S,
    scene: &mut Scene,
    entity_id: i64,
) {
    if entity_id <= 0 {
        return;
    }
    let mut remove_infos = Vec::new();
    let concomitants: Vec<i64> = scene
        .entity(entity_id)
        .and_then(|s| s.concomitant.as_ref())
        .map(|c| {
            let mut ids = Vec::with_capacity(c.vision_entity_id.len() + c.custom_entity_ids.len());
            ids.extend_from_slice(&c.vision_entity_id);
            ids.extend_from_slice(&c.custom_entity_ids);
            ids
        })
        .unwrap_or_default();
    for cid in concomitants {
        let _ = scene.remove(fs, cid);
        remove_infos.push(EntityRemoveInfo {
            EntityId: cid,
            Type: None,
        });
    }
    let _ = scene.remove(fs, entity_id);
    remove_infos.push(EntityRemoveInfo {
        EntityId: entity_id,
        Type: None,
    });
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

pub fn ensure_plot_main_role<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    let gender = ctx.basic_comp.info.attributes.sex;
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    if scene.plot_added_rover_slot.is_some() {
        return;
    }
    let Some(role_comp) = ctx.role_comp.as_deref_mut() else {
        return;
    };
    if role::current_formation_main_role_id(ctx.assets, scene, gender).is_some() {
        return;
    }
    let Some(main_role_id) = role::selected_main_role_id(ctx.assets, role_comp, gender, 0)
        .or_else(|| role::main_role_id_for_element(ctx.assets, gender, 1))
    else {
        return;
    };
    if !role_comp.role_map.contains_key(&main_role_id) {
        return;
    }
    let Ok(cur_idx) = usize::try_from(scene.formation_info.cur_formation) else {
        return;
    };
    if cur_idx >= scene.formation_info.formations.len() {
        return;
    }
    let inst_dungeon = ctx
        .assets
        .tables
        .instance_dungeon
        .get_data_by_id(&scene.instance_id)
        .or_else(|| ctx.assets.tables.instance_dungeon.get_data_by_id(&8));
    let Some(inst_dungeon) = inst_dungeon else {
        return;
    };

    let target_slot = scene.formation_info.formations[cur_idx]
        .roles
        .iter()
        .position(|r| r.is_none())
        .unwrap_or(0);
    let saved_cur = scene.formation_info.formations[cur_idx].cur_role;
    let saved_slot_role = scene.formation_info.formations[cur_idx].roles[target_slot]
        .as_ref()
        .map_or(0, |r| r.role_id);
    let saved_slot_eid = scene.formation_info.formations[cur_idx].roles[target_slot]
        .as_ref()
        .map_or(-1, |r| r.entity_id);

    let Ok(rover_entity) = role_entity_templates::create_role_entity(
        ctx.fs,
        scene,
        scene.player_id,
        ctx.assets,
        role_comp,
        ctx.weapon_comp,
        ctx.echo_comp,
        &inst_dungeon,
        main_role_id,
    ) else {
        return;
    };

    scene.plot_added_rover_slot = Some(target_slot);
    scene.plot_saved_cur_role = saved_cur;
    scene.plot_saved_slot_role_id = saved_slot_role;
    scene.plot_saved_slot_entity_id = saved_slot_eid;

    scene.formation_info.formations[cur_idx].roles[target_slot] =
        Some(sonoro_gamesv_fs::formation_info::FormationRole {
            role_id: main_role_id,
            entity_id: rover_entity.net_id,
            on_stage_without_control: false,
        });
    scene.formation_info.formations[cur_idx].cur_role = main_role_id;
    set_formation_visibility(scene, main_role_id);

    let now_ms = now_millis();
    let Ok(role_pb) = entity_proto::build_optional(ctx.assets, scene, None, rover_entity.net_id, now_ms)
    else {
        return;
    };
    let concom_ids: Vec<i64> = scene
        .entity(rover_entity.net_id)
        .and_then(|s| s.concomitant.as_ref())
        .map(|c| {
            let mut ids = Vec::with_capacity(c.vision_entity_id.len() + c.custom_entity_ids.len());
            ids.extend_from_slice(&c.vision_entity_id);
            ids.extend_from_slice(&c.custom_entity_ids);
            ids
        })
        .unwrap_or_default();
    let add_concom_pbs: Vec<_> = concom_ids
        .into_iter()
        .filter_map(|cid| entity_proto::build_optional(ctx.assets, scene, None, cid, now_ms).ok())
        .collect();

    ctx.sink.push(
        EntityAddNotify::MSG_ID,
        &EntityAddNotify {
            EntityPbs: vec![role_pb],
            RemoveTagIds: false,
        },
    );
    if !add_concom_pbs.is_empty() {
        ctx.sink.push(
            EntityAddNotify::MSG_ID,
            &EntityAddNotify {
                EntityPbs: add_concom_pbs,
                RemoveTagIds: false,
            },
        );
    }
    let cur_form = scene.formation_info.formations[cur_idx].clone();
    push_formation_group_update(ctx.sink, scene, &cur_form);
}

pub fn restore_plot_main_role<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    let Some(slot_idx) = scene.plot_added_rover_slot else {
        return;
    };
    let saved_cur = scene.plot_saved_cur_role;
    let saved_slot_role = scene.plot_saved_slot_role_id;
    let saved_slot_eid = scene.plot_saved_slot_entity_id;
    scene.plot_added_rover_slot = None;
    scene.plot_saved_cur_role = 0;
    scene.plot_saved_slot_role_id = 0;
    scene.plot_saved_slot_entity_id = -1;

    let Ok(cur_idx) = usize::try_from(scene.formation_info.cur_formation) else {
        return;
    };
    if cur_idx >= scene.formation_info.formations.len() {
        return;
    }
    if slot_idx < scene.formation_info.formations[cur_idx].roles.len() {
        if let Some(ref temp_slot) = scene.formation_info.formations[cur_idx].roles[slot_idx] {
            let temp_eid = temp_slot.entity_id;
            remove_role_entity_and_concomitants(ctx.fs, ctx.sink, scene, temp_eid);
        }
        scene.formation_info.formations[cur_idx].roles[slot_idx] = if saved_slot_role != 0 {
            Some(sonoro_gamesv_fs::formation_info::FormationRole {
                role_id: saved_slot_role,
                entity_id: saved_slot_eid,
                on_stage_without_control: false,
            })
        } else {
            None
        };
    }
    if saved_cur != 0 {
        scene.formation_info.formations[cur_idx].cur_role = saved_cur;
    }
    let cur_role = scene.formation_info.formations[cur_idx].cur_role;
    set_formation_visibility(scene, cur_role);
    let cur_form = scene.formation_info.formations[cur_idx].clone();
    push_formation_group_update(ctx.sink, scene, &cur_form);
}

pub fn collect_active_ancestors(
    nodes: &[NodeEntry],
    active_nodes: &[i32],
    finished_nodes: &[i32],
) -> Vec<i32> {
    let mut result = Vec::new();
    for &nid in active_nodes {
        let mut chain = Vec::new();
        let mut cur = find_node_by_id(nodes, nid);
        while let Some(entry) = cur {
            if entry.parent_id <= 0 {
                break;
            }
            chain.push(entry.parent_id);
            cur = find_node_by_id(nodes, entry.parent_id);
        }
        for &pid in chain.iter().rev() {
            if active_nodes.contains(&pid) || finished_nodes.contains(&pid) {
                continue;
            }
            if !result.contains(&pid) {
                result.push(pid);
            }
        }
    }
    result
}

pub fn build_child_node_progress<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    aq: &ActiveQuest,
    node: &NodeEntry,
) -> Option<ChildQuestNodeProgress> {
    let cond = get_child_condition(&node.data)?;
    let ctype = json_str(cond.get("Type"))?;

    if ctype == "Kill" || ctype == "MonsterCreator" {
        let target_ids = extract_node_target_entity_ids(node);
        let killed_count = target_ids
            .iter()
            .filter(|&&tid| aq.has_killed_entity(tid))
            .count() as i32;
        let total_count = if target_ids.is_empty() {
            1
        } else {
            target_ids.len() as i32
        };
        return Some(ChildQuestNodeProgress {
            Progress: Some(ChildQuestNodeProgressProgressOneof::Kill(KillProgress {
                MonId: target_ids,
                PrefabNum: total_count,
                CurrNum: killed_count,
                TotalNum: total_count,
            })),
        });
    }

    if ctype == "DoInteract" {
        let target_ids = extract_node_target_entity_ids(node);
        let mut npc_ids = Vec::new();
        if let Some(pc) = ctx.progress_comp.as_deref() {
            for tid in target_ids {
                if pc
                    .info
                    .has_claim(&format!("interact_done:{}:{}:{tid}", aq.quest_id, node.id))
                {
                    npc_ids.push(tid);
                }
            }
        }
        return Some(ChildQuestNodeProgress {
            Progress: Some(ChildQuestNodeProgressProgressOneof::Interact(
                InteractProgress { NpcId: npc_ids },
            )),
        });
    }

    if ctype == "GetItem" {
        let mut list = Vec::new();
        if let Some(Value::Array(items)) = cond.get("Items") {
            for it in items {
                let Value::Object(obj) = it else {
                    continue;
                };
                let Some(iid) = json_int(obj.get("ItemId")) else {
                    continue;
                };
                let req = json_int(obj.get("Count")).unwrap_or(1);
                let owned = ctx.inventory_comp.info.normal_item_count(iid);
                list.push(GetItemCount {
                    ItemId: iid,
                    Count: req.min(owned),
                });
            }
        }
        return Some(ChildQuestNodeProgress {
            Progress: Some(ChildQuestNodeProgressProgressOneof::GetItem(
                GetItemProgress { Info: list },
            )),
        });
    }

    if ctype == "UseItem" {
        let iid = json_int(cond.get("ItemId")).unwrap_or(0);
        if iid > 0 {
            let used = ctx.progress_comp.as_deref().map_or(0, |pc| {
                pc.info
                    .get_key_int(&format!("use_item:{}:{}:{iid}", aq.quest_id, node.id))
                    .clamp(0, i64::from(i32::MAX)) as i32
            });
            return Some(ChildQuestNodeProgress {
                Progress: Some(ChildQuestNodeProgressProgressOneof::UseItem(
                    UseItemProgress {
                        ItemId: iid,
                        Count: used,
                    },
                )),
            });
        }
    }

    if ctype == "CheckEntityState" {
        let mut eids = Vec::new();
        if let Some(Value::Array(cs)) = cond.get("Conditions") {
            for c in cs {
                let Value::Object(obj) = c else {
                    continue;
                };
                let Some(tid) = json_int(obj.get("EntityId")) else {
                    continue;
                };
                if tid <= 0 {
                    continue;
                }
                let want_state = json_str(obj.get("State")).unwrap_or("");
                if want_state.is_empty() || tree::entity_has_state_tag(ctx, tid, want_state) {
                    eids.push(tid);
                }
            }
        } else {
            eids = extract_node_target_entity_ids(node);
        }
        return Some(ChildQuestNodeProgress {
            Progress: Some(ChildQuestNodeProgressProgressOneof::EntityStateList(
                EntityStateProgress { EntityId: eids },
            )),
        });
    }

    if ctype == "CheckLevelPlay" {
        let mut done = 0i32;
        if let Some(Value::Array(ids)) = cond.get("LevelPlayIds") {
            for v in ids {
                if let Some(lpid) = json_int(Some(v))
                    && ctx.quest_comp.info.completed_level_plays.contains(&lpid)
                {
                    done += 1;
                }
            }
        }
        return Some(ChildQuestNodeProgress {
            Progress: Some(ChildQuestNodeProgressProgressOneof::levelPlayCount(done)),
        });
    }

    if ctype == "FinishDungeon" {
        let target = nested_dungeon_id(cond, "Condition");
        let count = i32::from(
            target > 0 && ctx.quest_comp.info.completed_dungeons.contains(&target),
        );
        return Some(ChildQuestNodeProgress {
            Progress: Some(ChildQuestNodeProgressProgressOneof::CompleteInst(
                CompleteInstProgress {
                    InstId: target,
                    Count: count,
                },
            )),
        });
    }

    None
}

pub fn push_node_progress_notify<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    aq: &ActiveQuest,
    node: &NodeEntry,
) {
    let Some(progress) = build_child_node_progress(ctx, aq, node) else {
        return;
    };
    ctx.sink.push(
        UpdateNodeProgressNotify::MSG_ID,
        &UpdateNodeProgressNotify {
            TreeOwnerId: aq.quest_id,
            TreeIncId: i64::from(aq.quest_id),
            NodeId: node.id,
            Progress: Some(progress),
        },
    );
}

pub fn build_tree_info<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    aq: &ActiveQuest,
) -> TreeInfo {
    let nodes = ctx
        .assets
        .quest_nodes
        .get_owner_nodes(aq.quest_id)
        .unwrap_or_default();
    let ancestors = collect_active_ancestors(&nodes, &aq.active_nodes, &aq.finished_nodes);

    let mut tree_nodes = BTreeMap::new();
    let mut timer_infos = Vec::new();
    let mut seen = HashSet::new();

    for &fid in &aq.finished_nodes {
        if !seen.insert(fid) {
            continue;
        }
        let is_child = find_node_by_id(&nodes, fid).is_some_and(|n| n.node_type == "ChildQuest");
        tree_nodes.insert(
            fid,
            NodeInfo {
                Status: Some(NodeStatus::CompletedSuccess),
                ExtraInfo: is_child.then_some(NodeInfoExtraInfoOneof::ChildQuestNodeInfo(
                    ChildQuestNodeInfo {
                        Status: Some(ChildQuestNodeStatus::Finished),
                        Progress: None,
                    },
                )),
            },
        );
    }

    for aid in ancestors {
        if !seen.insert(aid) {
            continue;
        }
        tree_nodes.insert(
            aid,
            NodeInfo {
                Status: Some(NodeStatus::Activate),
                ExtraInfo: None,
            },
        );
    }

    for &nid in &aq.active_nodes {
        if !seen.insert(nid) {
            continue;
        }
        let progress =
            find_node_by_id(&nodes, nid).and_then(|n| build_child_node_progress(ctx, aq, n));
        if let Some(scene) = ctx.scene.as_deref()
            && let Some(due_ms) = scene.quest_timers.get_due_ms(aq.quest_id, nid)
        {
            timer_infos.push(TimerInfoPb {
                TimerType: b"WaitTime".to_vec(),
                NodeId: nid,
                EndTime: due_ms / 1000,
                PauseTime: 0,
            });
        }
        tree_nodes.insert(
            nid,
            NodeInfo {
                Status: Some(NodeStatus::Activate),
                ExtraInfo: Some(NodeInfoExtraInfoOneof::ChildQuestNodeInfo(
                    ChildQuestNodeInfo {
                        Status: Some(ChildQuestNodeStatus::Progress),
                        Progress: progress,
                    },
                )),
            },
        );
    }

    init_tree_info(aq.quest_id, tree_nodes, timer_infos)
}

pub fn init_tree_info(
    quest_id: i32,
    nodes: BTreeMap<i32, NodeInfo>,
    timer_infos: Vec<TimerInfoPb>,
) -> TreeInfo {
    TreeInfo {
        TreeOwnerId: quest_id,
        TreeIncId: i64::from(quest_id),
        BtType: Some(BtType::BtTypeQuest),
        BlackboardId: quest_id,
        Nodes: nodes,
        TimerInfos: timer_infos,
        ..Default::default()
    }
}

pub fn push_ancestor_activations<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    nodes: &[NodeEntry],
    parent_id: i32,
) {
    if parent_id <= 0 {
        return;
    }
    let mut chain = Vec::new();
    let mut pid = parent_id;
    while pid > 0 {
        chain.push(pid);
        let Some(parent) = find_node_by_id(nodes, pid) else {
            break;
        };
        pid = parent.parent_id;
    }
    for &nid in chain.iter().rev() {
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: nid,
                Status: Some(NodeStatus::Activate),
            },
        );
    }
}

pub fn build_pre_show_quest_list<S: PushSink + ?Sized>(ctx: &QuestContext<'_, S>) -> Vec<i32> {
    if !ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID) {
        return Vec::new();
    }
    let eval_ctx = ctx.make_eval_context();
    let mut list = Vec::new();
    for q in &ctx.assets.tables.quest_data.items {
        let d = &q.Data;
        if !tree::is_eligible_quest(ctx, d, false) {
            continue;
        }
        if condition::eval_quest_provide_conditions(&eval_ctx, d.ProvideType.as_ref()) {
            continue;
        }
        let Some(Value::Object(psi)) = &d.PreShowInfo else {
            continue;
        };
        let Some(cond) = psi.get("PreShowCondition") else {
            continue;
        };
        if condition::eval_quest_provide_conditions(&eval_ctx, Some(cond)) {
            list.push(d.Id);
        }
    }
    list
}

pub fn build_ready_quest_list<S: PushSink + ?Sized>(ctx: &QuestContext<'_, S>) -> Vec<i32> {
    if !ctx.quest_comp.info.has_finished_quest(PROLOGUE_QUEST_ID) {
        return Vec::new();
    }
    let cur_map = ctx
        .scene
        .as_deref()
        .map(|s| s.instance_id)
        .or_else(|| ctx.scene_comp.as_deref().map(|sc| sc.last_scene_info.instance_id))
        .unwrap_or(8);
    let eval_ctx = ctx.make_eval_context();
    let mut list = Vec::new();
    for q in &ctx.assets.tables.quest_data.items {
        let d = &q.Data;
        if d.DistributeType != "Interact" {
            continue;
        }
        if d.DungeonId > 0 && d.DungeonId != cur_map {
            continue;
        }
        if let Some(Value::Object(aio)) = &d.AddInteractOption
            && let Some(eid) = json_int(aio.get("EntityId"))
            && eid > 0
            && ctx.assets.find_level_entity(cur_map, i64::from(eid)).is_none()
        {
            continue;
        }
        if !tree::is_eligible_quest(ctx, d, false) {
            continue;
        }
        if condition::eval_quest_provide_conditions(&eval_ctx, d.ProvideType.as_ref()) {
            list.push(d.Id);
        }
    }
    list
}

pub fn push_ready_quest_list<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    let Some(scene) = ctx.scene.as_deref() else {
        return;
    };
    if !scene.join_scene_pushed {
        return;
    }
    let ready_list = build_ready_quest_list(ctx);
    if !ready_list.is_empty() {
        ctx.sink.push(
            QuestReadyListNotify::MSG_ID,
            &QuestReadyListNotify {
                QuestId: ready_list,
            },
        );
    }
}

pub fn push_daily_and_branch_notifies<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    let mut all_quest = BTreeMap::new();
    for &dq in &ctx.quest_comp.info.daily.daily_quests {
        let status = if ctx.quest_comp.info.daily.claimed_daily_rewards.contains(&dq) {
            2
        } else if ctx.quest_comp.info.has_finished_quest(dq) {
            1
        } else {
            0
        };
        all_quest.insert(dq, status);
    }
    ctx.sink.push(
        DailyQuestStateNotify::MSG_ID,
        &DailyQuestStateNotify {
            AllQuest: all_quest,
            GetReward: ctx.quest_comp.info.daily.claimed_daily_rewards.clone(),
        },
    );

    ctx.sink.push(
        DailyQuestUnlockAreaNotify::MSG_ID,
        &DailyQuestUnlockAreaNotify {
            CurrChoosedArea: ctx.quest_comp.info.daily.preferred_daily_area,
            UnlockAreaIds: vec![1],
            IsInit: true,
            UnlockInfluenceIds: Vec::new(),
            CurrChoosedInfluence: ctx.quest_comp.info.daily.preferred_daily_influence,
        },
    );

    if !ctx.quest_comp.info.progress.quest_branches.is_empty() {
        let branch_info = super::build_quest_branch_info(ctx);
        ctx.sink.push(
            QuestBranchUpdateNotify::MSG_ID,
            &QuestBranchUpdateNotify {
                QuestBranchInfo: Some(branch_info),
                ErrorCode: Some(sonoro_proto::enums::ErrorCode::Success),
            },
        );
    }
}

pub fn push_quest_state_notifies<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) -> io::Result<()> {
    push_quest_state_notifies_with_enter(ctx, false, false)
}

pub fn push_quest_state_notifies_with_enter<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    send_enter: bool,
    send_trees: bool,
) -> io::Result<()> {
    let mut quest_list = Vec::new();
    let mut tree_infos = Vec::new();
    for aq in &ctx.quest_comp.info.active_quests {
        quest_list.push(PbQuestInfo {
            QuestId: aq.quest_id,
            Status: Some(QuestState::InProgress),
        });
        if send_trees {
            tree_infos.push(build_tree_info(ctx, aq));
        }
    }
    let finish_list = ctx.quest_comp.info.finished_quests.clone();
    let show_list = build_pre_show_quest_list(ctx);

    ctx.sink
        .push(QuestListNotify::MSG_ID, &QuestListNotify { Quests: quest_list });
    push_ready_quest_list(ctx);
    ctx.sink.push(
        QuestShowListNotify::MSG_ID,
        &QuestShowListNotify { QuestId: show_list },
    );
    ctx.sink.push(
        QuestFinishListNotify::MSG_ID,
        &QuestFinishListNotify {
            QuestId: finish_list,
        },
    );
    if send_trees && !tree_infos.is_empty() {
        ctx.sink.push(
            BehaviorTreeInfoNotify::MSG_ID,
            &BehaviorTreeInfoNotify {
                TreeInfos: tree_infos,
            },
        );
    }

    if !send_trees {
        let active_snap = ctx.quest_comp.info.active_quests.clone();
        for aq in &active_snap {
            let nodes = ctx
                .assets
                .quest_nodes
                .get_owner_nodes(aq.quest_id)
                .unwrap_or_default();
            let ancestors = collect_active_ancestors(&nodes, &aq.active_nodes, &aq.finished_nodes);
            for aid in ancestors {
                ctx.sink.push(
                    UpdateNodeStatusNotify::MSG_ID,
                    &UpdateNodeStatusNotify {
                        TreeOwnerId: aq.quest_id,
                        TreeIncId: i64::from(aq.quest_id),
                        NodeId: aid,
                        Status: Some(NodeStatus::Activate),
                    },
                );
            }
            for &nid in &aq.active_nodes {
                let n = find_node_by_id(&nodes, nid);
                if is_node_hide_ui(n) {
                    resync_active_node(ctx, aq.quest_id, nid, n, send_enter)?;
                }
            }
            for &nid in &aq.active_nodes {
                let n = find_node_by_id(&nodes, nid);
                if !is_node_hide_ui(n) {
                    resync_active_node(ctx, aq.quest_id, nid, n, send_enter)?;
                }
            }
        }
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
    push_daily_and_branch_notifies(ctx);
    Ok(())
}

fn is_node_hide_ui(node: Option<&NodeEntry>) -> bool {
    node.and_then(|entry| entry.data.get("HideUi"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub fn node_action_items_pub<'a>(node_opt: Option<&'a NodeEntry>, field: &str) -> &'a [Value] {
    node_opt
        .and_then(|entry| entry.data.get(field))
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn is_play_flow_client_flow<S: PushSink + ?Sized>(ctx: &QuestContext<'_, S>, act: &Value) -> bool {
    let Some(Value::Object(params)) = act.get("Params") else {
        return false;
    };
    let Some(flow_name) = json_str(params.get("FlowListName")) else {
        return false;
    };
    let flow_id = json_int(params.get("FlowId")).unwrap_or(1);
    let state_id = json_int(params.get("StateId")).unwrap_or(1);
    let Ok(Some(state_val)) = ctx.assets.flow_states.get_flow_state(flow_name, flow_id, state_id)
    else {
        return false;
    };
    json_bool(state_val.get("IsClientFlow")).unwrap_or(false)
}

const CLIENT_EXECUTABLE_BT_ACTIONS: &[&str] = &[
    "AddTrialCharacter",
    "AdjustPlayerCamera",
    "BvbPlayDialog",
    "BvbPlayerOperationConstraint",
    "CameraLookAt",
    "ChangeEntityState",
    "ChangeNpcPerformState",
    "CharacterLookAt",
    "CharacterMoveToPoint",
    "ClientChangeTeamPosition",
    "ClientModifyTargetTag",
    "ClientPreEnableSubLevels",
    "ClientSetPlayerPos",
    "ClientSetSeasonState",
    "ClientUnlockAchievementSystemItem",
    "CommonTip",
    "CommonTip2",
    "CompleteGuide",
    "DisableEntityLookAt",
    "DrinksRollRoleRequirement",
    "DriveMotorInSpecConfig",
    "EnableAI",
    "EnableActor",
    "EnableClientFunction",
    "EnableEntityLookAt",
    "EnableHostility",
    "EnableKey4Func",
    "EnableNearbyTracking",
    "EnableSplineMoveModel",
    "EndCommonTip",
    "EnterMovieCamera",
    "EnterMovieMode",
    "EnterOrbitalCamera",
    "EntityLookAt",
    "EntityTurnTo",
    "ExecClientBattleAction",
    "ExitDungeon",
    "ExitMovieCamera",
    "ExitMovieMode",
    "ExitOrbitalCamera",
    "FadeInScreen",
    "FadeOutScreen",
    "FocusOnMapMark",
    "GiantTriggerResponse",
    "GuestOperateUiAnimation",
    "GuideTrigger",
    "HideTargetRange",
    "LockEntity",
    "Log",
    "MoveSceneItem",
    "MoveWithSpline",
    "NewMoveWithSpline",
    "NpcLeisureInteract",
    "OpenQte",
    "OpenQuickHackSystem",
    "OpenSimpleGameplay",
    "OpenSystemBoard",
    "OpenSystemBoardWithReturn",
    "OverrideAiBehaviorTree",
    "PhoneSystemInteract",
    "PlayBubble",
    "PlayDynamicSettlement",
    "PlayEffect",
    "PlayEffect2",
    "PlayMontage",
    "PlayRegisteredMontage",
    "PlayerInput",
    "PlayerLookAt",
    "PostAkEvent",
    "Preload",
    "Prompt",
    "PromptQuestChapterUI",
    "RemoveNpcGroupPerform",
    "ResetPlayerCameraFocus",
    "RestoreAiBehaviorTree",
    "RestorePlayerCameraAdjustment",
    "RunActions",
    "SetBattleState",
    "SetEntityClientVisibleSave",
    "SetNpcGroupPerform",
    "SetRegionConfig",
    "SetSportsState",
    "SetSubScreenMode",
    "SetTimeScale",
    "SetWuYinQuState",
    "ShowTargetRange",
    "SlideRailEnd",
    "StartRoadNetworkNavigation",
    "StopCameraLookAt",
    "StopNewMoveWithSpline",
    "StopUiScreenEffect",
    "SwitchDataLayers",
    "SwitchSubLevels",
    "TakePlotPhoto",
    "TeleportDungeon",
    "ToggleAirWall",
    "ToggleDragActorPlay",
    "ToggleHighlightExploreUi",
    "ToggleMapMarkState",
    "ToggleScanSplineEffect",
    "TrapDefenseChangeMiniMap",
    "TrapDefensePlayerOperationConstraint",
    "TriggerCameraShake",
    "TriggerGamepadShake",
    "TriggerSpecificScanEffect",
    "UnlockEntity",
    "UsePhantomSkill",
    "VehicleMoveWithPathLine",
    "Wait",
    "WaitBattleCondition",
    "WaitUntilLevelSequenceReachMark",
];

pub fn is_client_action_name_registered(name: &str) -> bool {
    CLIENT_EXECUTABLE_BT_ACTIONS.binary_search(&name).is_ok()
}

pub fn is_safe_bt_leisure_interact(act: &Value) -> bool {
    let Some(Value::Object(params)) = act.get("Params") else {
        return false;
    };
    let Some(Value::Object(option)) = params.get("Option") else {
        return false;
    };
    let Some(option_type) = json_str(option.get("Type")) else {
        return false;
    };
    if option_type == "SitDown" || option_type == "SitOnGround" {
        return json_int(params.get("SceneEntity")).unwrap_or(0) > 0;
    }
    if option_type == "MotorEnterSlide" {
        return json_int(option.get("EntityId")).unwrap_or(0) > 0;
    }
    matches!(
        option_type,
        "CustomSkill"
            | "GetUp"
            | "FailurePose"
            | "Soar"
            | "StandControl2"
            | "MoneHackingDoor"
            | "LandSlideEnd"
            | "Glide"
    )
}

fn is_client_executable_bt_action<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    act: &Value,
) -> bool {
    let Value::Object(obj) = act else {
        return false;
    };
    let Some(name) = json_str(obj.get("Name")) else {
        return false;
    };
    if name == "PlayFlow" {
        return is_play_flow_client_flow(ctx, act);
    }
    if name == "LeisureInteract" {
        return is_safe_bt_leisure_interact(act);
    }
    is_client_action_name_registered(name)
}

pub fn eval_inline_condition<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    owner_id: i32,
    cb: &Value,
) -> bool {
    let eval_ctx = ctx.make_eval_context_for_quest(owner_id);
    condition::eval_inline_condition_block(&eval_ctx, cb)
}

pub fn eval_child_condition_auto<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    owner_id: i32,
    node: &NodeEntry,
) -> bool {
    should_auto_complete_child_quest(ctx, owner_id, node)
}

pub fn get_child_condition_pub(data: &Value) -> Option<&Map<String, Value>> {
    get_child_condition(data)
}

pub fn push_bt_level_events_for_type<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    bt_type: BtType,
    owner_id: i32,
    node_id: i32,
    ctx_type: GameCtxType,
    actions: &[Value],
) -> io::Result<()> {
    if actions.is_empty() {
        return Ok(());
    }
    let bt_ctx = BehaviorTreeCtxPb {
        IncId: i64::from(owner_id),
        BtType: Some(bt_type),
        BtId: owner_id,
        NodeId: node_id,
    };
    let ctx_info = match ctx_type {
        GameCtxType::ChildQuestNodeEnterAction => {
            GameCtxPbCtxInfoOneof::ChildQuestNodeEnterAction(ChildQuestNodeEnterActionCtxPb {
                BehaviorTreeCtx: Some(bt_ctx),
            })
        }
        GameCtxType::ChildQuestNodeFinishAction => {
            GameCtxPbCtxInfoOneof::ChildQuestNodeFinishAction(ChildQuestNodeFinishActionCtxPb {
                BehaviorTreeCtx: Some(bt_ctx),
            })
        }
        GameCtxType::ActionGroupNodeAction => {
            GameCtxPbCtxInfoOneof::ActionGroupNodeAction(ActionGroupNodeActionCtxPb {
                BehaviorTreeCtx: Some(bt_ctx),
            })
        }
        GameCtxType::SuccessNodeAction => {
            GameCtxPbCtxInfoOneof::SuccessNodeAction(SuccessNodeActionCtxPb {
                BehaviorTreeCtx: Some(bt_ctx),
            })
        }
        _ => return Ok(()),
    };
    let game_ctx = GameCtxPb {
        CtxType: Some(ctx_type),
        CtxInfo: Some(ctx_info),
    };
    let total_count = actions.len() as i32;
    let player_id = ctx.basic_comp.player_id;
    let mut i = 0usize;
    while i < actions.len() {
        if !is_client_executable_bt_action(ctx, &actions[i]) {
            i += 1;
            continue;
        }
        let start_idx = i;
        while i < actions.len() && is_client_executable_bt_action(ctx, &actions[i]) {
            i += 1;
        }
        let end_idx = i - 1;
        ctx.sink.push(
            LevelEventNotify::MSG_ID,
            &LevelEventNotify {
                PlayerId: player_id,
                IncId: owner_id,
                GameCtx: Some(game_ctx.clone()),
                TotalCount: total_count,
                StartIndex: start_idx as i32,
                EndIndex: end_idx as i32,
                NeedFinishReq: false,
            },
        );
    }
    Ok(())
}

pub fn push_bt_level_events<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    ctx_type: GameCtxType,
    actions: &[Value],
) -> io::Result<()> {
    push_bt_level_events_for_type(ctx, BtType::BtTypeQuest, quest_id, node_id, ctx_type, actions)
}

fn resync_active_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    node_opt: Option<&NodeEntry>,
    send_enter: bool,
) -> io::Result<()> {
    if send_enter {
        push_node_status(
            ctx,
            quest_id,
            node_id,
            NodeStatus::Activate,
            ChildQuestNodeStatus::Progress,
        )?;
        if let Some(node) = node_opt
            && let Some(scene) = ctx.scene.as_deref_mut()
        {
            push_do_interact_options_for_node(scene, ctx.sink, quest_id, node, None)?;
        }
        return Ok(());
    }
    ctx.sink.push(
        UpdateNodeStatusNotify::MSG_ID,
        &UpdateNodeStatusNotify {
            TreeOwnerId: quest_id,
            TreeIncId: i64::from(quest_id),
            NodeId: node_id,
            Status: Some(NodeStatus::Activate),
        },
    );
    if let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id).cloned()
        && let Some(node) = node_opt
    {
        push_node_progress_notify(ctx, &aq, node);
    }
    if let Some(node) = node_opt
        && let Some(scene) = ctx.scene.as_deref_mut()
    {
        push_do_interact_options_for_node(scene, ctx.sink, quest_id, node, None)?;
    }
    Ok(())
}

pub fn sync_dynamic_interact_options_for_entity<S: PushSink + ?Sized>(
    scene: &mut Scene,
    assets: &Assets,
    sink: &mut S,
    quest_info: &QuestInfo,
    progress_comp: Option<&PlayerProgressComponent>,
    cfg_id: i64,
) -> io::Result<()> {
    sync_dynamic_interact_options_for_entity_mode(
        scene,
        assets,
        sink,
        quest_info,
        progress_comp,
        cfg_id,
        true,
    )
}

pub fn register_dynamic_interact_options_for_entity(
    scene: &mut Scene,
    assets: &Assets,
    quest_info: &QuestInfo,
    progress_comp: Option<&PlayerProgressComponent>,
    cfg_id: i64,
) -> io::Result<()> {
    let mut noop = crate::sink::NoopSink;
    sync_dynamic_interact_options_for_entity_mode(
        scene,
        assets,
        &mut noop,
        quest_info,
        progress_comp,
        cfg_id,
        false,
    )
}

fn sync_dynamic_interact_options_for_entity_mode<S: PushSink + ?Sized>(
    scene: &mut Scene,
    assets: &Assets,
    sink: &mut S,
    quest_info: &QuestInfo,
    progress_comp: Option<&PlayerProgressComponent>,
    cfg_id: i64,
    push_notifies: bool,
) -> io::Result<()> {
    if cfg_id <= 0 {
        return Ok(());
    }
    for aq in &quest_info.active_quests {
        let Ok(nodes) = assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            if let Some(node) = find_node_by_id(&nodes, nid) {
                push_do_interact_options_for_bt_node_mode(
                    scene,
                    sink,
                    BtType::BtTypeQuest,
                    aq.quest_id,
                    node,
                    Some(cfg_id),
                    push_notifies,
                )?;
            }
        }
    }
    crate::helpers::level_play::sync_dynamic_interact_options_for_entity_mode(
        scene,
        assets,
        sink,
        quest_info,
        progress_comp,
        Some(cfg_id),
        push_notifies,
    )
}

pub fn sync_active_dynamic_interact_options<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
) -> io::Result<()> {
    let aq_snap: Vec<(i32, Vec<i32>)> = ctx
        .quest_comp
        .info
        .active_quests
        .iter()
        .map(|aq| (aq.quest_id, aq.active_nodes.clone()))
        .collect();
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return Ok(());
    };
    for (quest_id, active_nodes) in aq_snap {
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
            continue;
        };
        for nid in active_nodes {
            if let Some(node) = find_node_by_id(&nodes, nid) {
                push_do_interact_options_for_node(scene, ctx.sink, quest_id, node, None)?;
            }
        }
    }
    crate::helpers::level_play::sync_active_dynamic_interact_options(
        scene,
        ctx.assets,
        ctx.sink,
        &ctx.quest_comp.info,
        ctx.progress_comp.as_deref(),
    )
}

pub fn push_do_interact_options_for_node<S: PushSink + ?Sized>(
    scene: &mut Scene,
    sink: &mut S,
    quest_id: i32,
    node: &NodeEntry,
    filter_cfg_id: Option<i64>,
) -> io::Result<()> {
    push_do_interact_options_for_bt_node(
        scene,
        sink,
        BtType::BtTypeQuest,
        quest_id,
        node,
        filter_cfg_id,
    )
}

pub fn push_do_interact_options_for_bt_node<S: PushSink + ?Sized>(
    scene: &mut Scene,
    sink: &mut S,
    bt_type: BtType,
    owner_id: i32,
    node: &NodeEntry,
    filter_cfg_id: Option<i64>,
) -> io::Result<()> {
    push_do_interact_options_for_bt_node_mode(
        scene,
        sink,
        bt_type,
        owner_id,
        node,
        filter_cfg_id,
        true,
    )
}

pub fn push_do_interact_options_for_bt_node_mode<S: PushSink + ?Sized>(
    scene: &mut Scene,
    sink: &mut S,
    bt_type: BtType,
    owner_id: i32,
    node: &NodeEntry,
    filter_cfg_id: Option<i64>,
    push_notifies: bool,
) -> io::Result<()> {
    if node.node_type != "ChildQuest" {
        return Ok(());
    }
    let Some(cond) = get_child_condition(&node.data) else {
        return Ok(());
    };
    let Some(ctype) = json_str(cond.get("Type")) else {
        return Ok(());
    };
    if ctype == "DoInteract" {
        let Some(Value::Array(add_opts)) = cond.get("AddOptions") else {
            return Ok(());
        };
        for entry in add_opts {
            push_dynamic_interact_entry(
                scene,
                sink,
                bt_type,
                owner_id,
                node.id,
                entry,
                filter_cfg_id,
                push_notifies,
            )?;
        }
    } else if ctype == "HandInItems" {
        if let Some(ao) = cond.get("AddOption") {
            push_dynamic_interact_entry(
                scene,
                sink,
                bt_type,
                owner_id,
                node.id,
                ao,
                filter_cfg_id,
                push_notifies,
            )?;
        } else if let Some(Value::Object(hi)) = cond.get("HandInItems")
            && let Some(ao) = hi.get("AddOption")
        {
            push_dynamic_interact_entry(
                scene,
                sink,
                bt_type,
                owner_id,
                node.id,
                ao,
                filter_cfg_id,
                push_notifies,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_dynamic_interact_entry<S: PushSink + ?Sized>(
    scene: &mut Scene,
    sink: &mut S,
    bt_type: BtType,
    quest_id: i32,
    node_id: i32,
    entry: &Value,
    filter_cfg_id: Option<i64>,
    push_notifies: bool,
) -> io::Result<()> {
    let Value::Object(obj) = entry else {
        return Ok(());
    };
    let Some(cfg_id) = json_int(obj.get("EntityId")) else {
        return Ok(());
    };
    if cfg_id <= 0 {
        return Ok(());
    }
    if let Some(want_id) = filter_cfg_id
        && i64::from(cfg_id) != want_id
    {
        return Ok(());
    }
    let Some(net_id) = scene.find_entity_net_id_by_config_id(cfg_id) else {
        return Ok(());
    };
    let Some(Value::Object(opt_val)) = obj.get("Option") else {
        return Ok(());
    };
    let Some(guid) = json_str(opt_val.get("Guid")) else {
        return Ok(());
    };
    if guid.is_empty() {
        return Ok(());
    }
    let text = json_str(opt_val.get("TidContent")).unwrap_or("");

    let mut kind = DynamicInteractKind::None;
    let mut flow_name = String::new();
    let mut flow_id = 0;
    let mut state_id = 0;
    let mut actions_json = String::new();

    if let Some(Value::Object(tv)) = opt_val.get("Type") {
        let tname = json_str(tv.get("Type")).unwrap_or("");
        if tname == "Flow" {
            if let Some(Value::Object(fv)) = tv.get("Flow") {
                flow_name = json_str(fv.get("FlowListName")).unwrap_or("").to_string();
                flow_id = json_int(fv.get("FlowId")).unwrap_or(0);
                state_id = json_int(fv.get("StateId")).unwrap_or(0);
                if !flow_name.is_empty() && flow_id > 0 {
                    kind = DynamicInteractKind::Flow;
                }
            }
        } else if tname == "Actions" {
            kind = DynamicInteractKind::Actions;
            actions_json = match tv.get("Actions") {
                Some(av @ Value::Array(arr)) if !arr.is_empty() => {
                    serde_json::to_string(av).unwrap_or_else(|_| "[]".to_string())
                }
                _ => "[]".to_string(),
            };
        }
    }

    if kind == DynamicInteractKind::None {
        kind = DynamicInteractKind::Actions;
        actions_json = "[]".to_string();
    }

    let same_existing = scene.find_dynamic_interact_option(guid).is_some_and(|existing| {
        existing.entity_net_id == net_id
            && existing.bt_type == bt_type
            && existing.quest_id == quest_id
            && existing.node_id == node_id
            && existing.kind == kind
            && existing.flow_list_name == flow_name
            && existing.flow_id == flow_id
            && existing.state_id == state_id
            && existing.actions_json == actions_json
            && existing.text == text
    });
    if same_existing {
        return Ok(());
    }

    if push_notifies
        && let Some(existing) = scene.find_dynamic_interact_option(guid)
        && existing.entity_net_id == net_id
    {
        sink.push(
            EntityRemoveDynamicInteractNotify::MSG_ID,
            &EntityRemoveDynamicInteractNotify {
                OptionGuid: guid.as_bytes().to_vec(),
                EntityId: net_id,
            },
        );
    }

    scene.upsert_dynamic_interact_option_with_bt(
        guid,
        net_id,
        cfg_id,
        bt_type,
        quest_id,
        node_id,
        kind,
        &flow_name,
        flow_id,
        state_id,
        &actions_json,
        text,
    );

    if push_notifies {
        sink.push(
            EntityAddDynamicInteractNotify::MSG_ID,
            &EntityAddDynamicInteractNotify {
                GameCtx: Some(GameCtxPb {
                    CtxType: Some(GameCtxType::BehaviorTree),
                    CtxInfo: Some(GameCtxPbCtxInfoOneof::BehaviorTree(BehaviorTreeCtxPb {
                        IncId: i64::from(quest_id),
                        BtType: Some(bt_type),
                        BtId: quest_id,
                        NodeId: node_id,
                    })),
                }),
                EntityId: net_id,
                DelayRemove: false,
                Text: text.as_bytes().to_vec(),
                OptionGuid: guid.as_bytes().to_vec(),
            },
        );
    }
    Ok(())
}

pub fn remove_do_interact_options_for_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
) {
    remove_do_interact_options_for_bt_node(ctx, BtType::BtTypeQuest, quest_id, node_id);
}

pub fn remove_do_interact_options_for_bt_node<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    bt_type: BtType,
    owner_id: i32,
    node_id: i32,
) {
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    let removed = scene.take_dynamic_interact_options_for_bt_node(bt_type, owner_id, node_id);
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

pub fn push_bt_child_node_status<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    bt_type: BtType,
    owner_id: i32,
    node: &NodeEntry,
    node_status: NodeStatus,
    child_status: ChildQuestNodeStatus,
) -> io::Result<()> {
    if node_status == NodeStatus::Activate && child_status == ChildQuestNodeStatus::Progress {
        for msg_status in [ChildQuestNodeStatus::Enter, ChildQuestNodeStatus::EnterAction] {
            if msg_status == ChildQuestNodeStatus::Enter {
                ctx.sink.push(
                    UpdateNodeStatusNotify::MSG_ID,
                    &UpdateNodeStatusNotify {
                        TreeOwnerId: owner_id,
                        TreeIncId: i64::from(owner_id),
                        NodeId: node.id,
                        Status: Some(node_status),
                    },
                );
            }
            ctx.sink.push(
                UpdateChildQuestNodeStatusNotify::MSG_ID,
                &UpdateChildQuestNodeStatusNotify {
                    TreeOwnerId: owner_id,
                    TreeIncId: i64::from(owner_id),
                    NodeId: node.id,
                    Status: Some(msg_status),
                },
            );
        }
        push_bt_level_events_for_type(
            ctx,
            bt_type,
            owner_id,
            node.id,
            GameCtxType::ChildQuestNodeEnterAction,
            node_action_items_pub(Some(node), "EnterActions"),
        )?;
        ctx.sink.push(
            UpdateChildQuestNodeStatusNotify::MSG_ID,
            &UpdateChildQuestNodeStatusNotify {
                TreeOwnerId: owner_id,
                TreeIncId: i64::from(owner_id),
                NodeId: node.id,
                Status: Some(child_status),
            },
        );
    } else if child_status == ChildQuestNodeStatus::Finished {
        push_bt_level_events_for_type(
            ctx,
            bt_type,
            owner_id,
            node.id,
            GameCtxType::ChildQuestNodeFinishAction,
            node_action_items_pub(Some(node), "FinishActions"),
        )?;
        ctx.sink.push(
            UpdateChildQuestNodeStatusNotify::MSG_ID,
            &UpdateChildQuestNodeStatusNotify {
                TreeOwnerId: owner_id,
                TreeIncId: i64::from(owner_id),
                NodeId: node.id,
                Status: Some(child_status),
            },
        );
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: owner_id,
                TreeIncId: i64::from(owner_id),
                NodeId: node.id,
                Status: Some(node_status),
            },
        );
    } else {
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: owner_id,
                TreeIncId: i64::from(owner_id),
                NodeId: node.id,
                Status: Some(node_status),
            },
        );
        ctx.sink.push(
            UpdateChildQuestNodeStatusNotify::MSG_ID,
            &UpdateChildQuestNodeStatusNotify {
                TreeOwnerId: owner_id,
                TreeIncId: i64::from(owner_id),
                NodeId: node.id,
                Status: Some(child_status),
            },
        );
    }
    Ok(())
}

pub fn push_node_status<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    node_status: NodeStatus,
    child_status: ChildQuestNodeStatus,
) -> io::Result<()> {
    let nodes = ctx
        .assets
        .quest_nodes
        .get_owner_nodes(quest_id)
        .unwrap_or_default();
    let node_opt = find_node_by_id(&nodes, node_id);

    if node_status == NodeStatus::Activate && child_status == ChildQuestNodeStatus::Progress {
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node_id,
                Status: Some(node_status),
            },
        );
        for msg_status in [ChildQuestNodeStatus::Enter, ChildQuestNodeStatus::EnterAction] {
            ctx.sink.push(
                UpdateChildQuestNodeStatusNotify::MSG_ID,
                &UpdateChildQuestNodeStatusNotify {
                    TreeOwnerId: quest_id,
                    TreeIncId: i64::from(quest_id),
                    NodeId: node_id,
                    Status: Some(msg_status),
                },
            );
        }
        push_bt_level_events(
            ctx,
            quest_id,
            node_id,
            GameCtxType::ChildQuestNodeEnterAction,
            node_action_items_pub(node_opt, "EnterActions"),
        )?;
        if let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id).cloned()
            && let Some(node) = node_opt
        {
            push_node_progress_notify(ctx, &aq, node);
        }
        ctx.sink.push(
            UpdateChildQuestNodeStatusNotify::MSG_ID,
            &UpdateChildQuestNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node_id,
                Status: Some(child_status),
            },
        );
    } else if child_status == ChildQuestNodeStatus::Finished {
        push_bt_level_events(
            ctx,
            quest_id,
            node_id,
            GameCtxType::ChildQuestNodeFinishAction,
            node_action_items_pub(node_opt, "FinishActions"),
        )?;
        ctx.sink.push(
            UpdateChildQuestNodeStatusNotify::MSG_ID,
            &UpdateChildQuestNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node_id,
                Status: Some(child_status),
            },
        );
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node_id,
                Status: Some(node_status),
            },
        );
    } else {
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node_id,
                Status: Some(node_status),
            },
        );
        ctx.sink.push(
            UpdateChildQuestNodeStatusNotify::MSG_ID,
            &UpdateChildQuestNodeStatusNotify {
                TreeOwnerId: quest_id,
                TreeIncId: i64::from(quest_id),
                NodeId: node_id,
                Status: Some(child_status),
            },
        );
    }
    Ok(())
}

pub fn ensure_tracked_focus_valid<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    let focus = ctx.quest_comp.info.focus_quest_id;
    if focus > 0 && !ctx.quest_comp.info.has_active_quest(focus) {
        ctx.quest_comp.info.focus_quest_id = 0;
    }

    let cur = ctx.quest_comp.info.tracked_quest_id;
    if cur <= 0 {
        ctx.quest_comp.info.tracked_quest_id = 0;
        return;
    }
    if ctx.quest_comp.info.has_active_quest(cur) {
        return;
    }

    let mut best = 0;
    for aq in &ctx.quest_comp.info.active_quests {
        if best == 0 {
            best = aq.quest_id;
        }
        if let Some(q) = ctx.assets.tables.quest_data.get_data_by_id(&aq.quest_id)
            && q.Data.Type == 1
        {
            best = aq.quest_id;
            break;
        }
    }
    ctx.quest_comp.info.tracked_quest_id = best;
}

pub fn sync_scene_current_node<S: PushSink + ?Sized>(ctx: &mut QuestContext<'_, S>) {
    let tracked_qid = ctx.quest_comp.info.tracked_quest_id;
    let first_active_node = ctx
        .quest_comp
        .info
        .find_active_quest(tracked_qid)
        .and_then(|aq| aq.active_nodes.first().copied())
        .unwrap_or(0);
    let Some(scene) = ctx.scene.as_deref_mut() else {
        return;
    };
    scene.has_quest_move_loc = false;
    scene.current_quest_node = first_active_node;
}

pub fn mark_and_check_do_interact_complete<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
    cond: &Map<String, Value>,
    entity_id: i64,
    opt_obj: &Map<String, Value>,
) -> io::Result<bool> {
    let target_eid = json_int(opt_obj.get("EntityId"))
        .unwrap_or_else(|| resolve_config_id(ctx.scene.as_deref(), entity_id));
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return Ok(true);
    };
    if target_eid > 0 {
        pc.info
            .mark_claim(&format!("interact_done:{quest_id}:{}:{target_eid}", node.id));
        pc.save(ctx.fs)?;
    }

    let mut all_done = true;
    if let Some(Value::Array(opts)) = cond.get("AddOptions")
        && opts.len() > 1
    {
        for o in opts {
            let Value::Object(oo) = o else {
                continue;
            };
            let Some(eid) = json_int(oo.get("EntityId")) else {
                continue;
            };
            if !pc
                .info
                .has_claim(&format!("interact_done:{quest_id}:{}:{eid}", node.id))
            {
                all_done = false;
                break;
            }
        }
    }
    if !all_done && let Some(aq) = ctx.quest_comp.info.find_active_quest(quest_id).cloned() {
        push_node_progress_notify(ctx, &aq, node);
    }
    Ok(all_done)
}

pub fn find_do_interact_option_by_guid<'a>(
    cond: &'a Map<String, Value>,
    guid: &str,
) -> Option<&'a Map<String, Value>> {
    if guid.is_empty() {
        return None;
    }
    let Value::Array(opts) = cond.get("AddOptions")? else {
        return None;
    };
    for entry in opts {
        let Value::Object(entry_obj) = entry else {
            continue;
        };
        let Some(Value::Object(inner)) = entry_obj.get("Option") else {
            continue;
        };
        if json_str(inner.get("Guid")) == Some(guid) {
            return Some(entry_obj);
        }
    }
    None
}

/// Advances the active `DoInteract` ChildQuest that owns a dynamic interact option.
/// `on_submit_node` intentionally rejects `DoInteract` so clients cannot self-submit it;
/// a server-validated dynamic interact request completes it here instead.
pub fn complete_do_interact_from_dynamic_option<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_id: i32,
    entity_id: i64,
    option_guid: &str,
) -> io::Result<bool> {
    if !ctx
        .quest_comp
        .info
        .find_active_quest(quest_id)
        .is_some_and(|aq| aq.is_node_active(node_id))
    {
        return Ok(false);
    }
    let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(quest_id) else {
        return Ok(false);
    };
    let Some(node) = find_node_by_id(&nodes, node_id).cloned() else {
        return Ok(false);
    };
    if node.node_type != "ChildQuest" {
        return Ok(false);
    }
    let Some(cond) = get_child_condition(&node.data).cloned() else {
        return Ok(false);
    };
    if json_str(cond.get("Type")) != Some("DoInteract") {
        return Ok(false);
    }

    let cfg_id = resolve_config_id(ctx.scene.as_deref(), entity_id);
    let Some(opt_obj) = find_do_interact_option_by_guid(&cond, option_guid)
        .or_else(|| match_do_interact(&cond, entity_id, cfg_id, 0))
        .cloned()
    else {
        return Ok(false);
    };

    if mark_and_check_do_interact_complete(ctx, quest_id, &node, &cond, entity_id, &opt_obj)? {
        super::tree::complete_child_quest(ctx, quest_id, &nodes, &node)?;
    }
    super::tree::finalize_progression(ctx)?;
    Ok(true)
}

pub fn handle_interact_option<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    nodes: &[NodeEntry],
    node: &NodeEntry,
    cond: &Map<String, Value>,
    entity_id: i64,
    opt_obj: &Map<String, Value>,
) -> io::Result<bool> {
    let Some(Value::Object(inner)) = opt_obj.get("Option") else {
        if mark_and_check_do_interact_complete(ctx, quest_id, node, cond, entity_id, opt_obj)? {
            tree::complete_child_quest(ctx, quest_id, nodes, node)?;
            return Ok(true);
        }
        return Ok(false);
    };

    if let Some(Value::Object(tval)) = inner.get("Type") {
        let tstr = json_str(tval.get("Type")).unwrap_or("");
        if tstr == "Flow" {
            if let Some(Value::Object(fv)) = tval.get("Flow") {
                if !json_bool(fv.get("Async")).unwrap_or(false)
                    && let Some(scene) = ctx.scene.as_deref_mut()
                    && scene.active_flow_inc_id != 0
                {
                    scene.clear_active_flow();
                }
                start_flow_from_obj(ctx, quest_id, node.id, fv)?;
                if let Some(scene) = ctx.scene.as_deref_mut()
                    && scene.active_flow_inc_id != 0
                    && scene.active_flow_quest_id == quest_id
                    && scene.active_flow_node_id == node.id
                {
                    scene.active_flow_owner_entity_id = entity_id;
                }
                return Ok(false);
            }
        } else if tstr == "Actions"
            && let Some(Value::Array(acts)) = tval.get("Actions")
        {
            let prev_owner = if let Some(scene) = ctx.scene.as_deref_mut() {
                let prev = scene.active_flow_owner_entity_id;
                scene.active_flow_owner_entity_id = entity_id;
                prev
            } else {
                0
            };
            let res = execute_actions(ctx, quest_id, node.id, acts);
            if let Some(scene) = ctx.scene.as_deref_mut() {
                scene.active_flow_owner_entity_id = prev_owner;
            }
            res?;
        }
    }

    if mark_and_check_do_interact_complete(ctx, quest_id, node, cond, entity_id, opt_obj)? {
        tree::complete_child_quest(ctx, quest_id, nodes, node)?;
        return Ok(true);
    }
    Ok(false)
}

pub fn consume_hand_in_items<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    cond: &Map<String, Value>,
) -> io::Result<bool> {
    let Some(Value::Object(hi)) = cond.get("HandInItems") else {
        return Ok(false);
    };
    let Some(Value::Array(items)) = hi.get("Items") else {
        return Ok(true);
    };

    for it in items {
        let Value::Object(obj) = it else {
            continue;
        };
        let count = json_int(obj.get("Count")).unwrap_or(1);
        if count <= 0 {
            continue;
        }
        let Some(Value::Array(id_vals)) = obj.get("ItemIds") else {
            return Ok(false);
        };
        let mut avail = 0i64;
        for id_v in id_vals {
            if let Some(iid) = json_int(Some(id_v)) {
                avail += i64::from(ctx.inventory_comp.info.normal_item_count(iid));
            }
        }
        if avail < i64::from(count) {
            return Ok(false);
        }
    }

    let mut removed_any = false;
    for it in items {
        let Value::Object(obj) = it else {
            continue;
        };
        let count = json_int(obj.get("Count")).unwrap_or(1);
        if count <= 0 {
            continue;
        }
        if let Some(Value::Array(id_vals)) = obj.get("ItemIds") {
            let mut rem = count;
            for id_v in id_vals {
                if rem <= 0 {
                    break;
                }
                let Some(iid) = json_int(Some(id_v)) else {
                    continue;
                };
                let owned = ctx.inventory_comp.info.normal_item_count(iid);
                if owned > 0 {
                    let take = owned.min(rem);
                    if ctx.inventory_comp.info.consume_normal_item(iid, take) {
                        rem -= take;
                        removed_any = true;
                    }
                }
            }
        }
    }

    if removed_any {
        let path = format!("player/{}/{}", ctx.basic_comp.player_id, InventoryInfo::DATA_PATH);
        crate::component::comp_util::save_struct(ctx.fs, &ctx.inventory_comp.info, &path)?;
    }
    Ok(true)
}

pub fn extract_ui_gameplay_key(cfg: &Map<String, Value>) -> Option<String> {
    for k in ["MorseCodeId", "CipherId", "ReignsId", "Id"] {
        match cfg.get(k) {
            Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            Some(Value::Number(n)) => {
                if let Some(iv) = n.as_i64() {
                    return Some(iv.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

pub fn matches_check_ui_game_key(cond: &Map<String, Value>, gameplay_key: &str) -> bool {
    let Some(Value::Array(cfgs)) = cond.get("GameplayConfigs") else {
        return false;
    };
    cfgs.iter().any(|item| {
        matches!(item, Value::Object(obj) if extract_ui_gameplay_key(obj).as_deref() == Some(gameplay_key))
    })
}

pub fn has_completed_ui_gameplay(pc: &PlayerProgressComponent, cond: &Map<String, Value>) -> bool {
    let Some(Value::Array(cfgs)) = cond.get("GameplayConfigs") else {
        return false;
    };
    if cfgs.is_empty() {
        return false;
    }
    for item in cfgs {
        let Value::Object(obj) = item else {
            continue;
        };
        let Some(k) = extract_ui_gameplay_key(obj) else {
            return false;
        };
        if !pc.info.has_claim(&format!("ui_gp:{k}")) {
            return false;
        }
    }
    true
}

pub fn snapshot_active_quest_ids<S: PushSink + ?Sized>(ctx: &QuestContext<'_, S>) -> Vec<i32> {
    ctx.quest_comp
        .info
        .active_quests
        .iter()
        .map(|aq| aq.quest_id)
        .collect()
}

pub fn find_root_node(nodes: &[NodeEntry]) -> Option<&NodeEntry> {
    nodes.iter().find(|n| n.parent_id == 0)
}

pub fn find_node_by_id(nodes: &[NodeEntry], node_id: i32) -> Option<&NodeEntry> {
    nodes.iter().find(|n| n.id == node_id)
}

pub fn get_child_condition(data: &Value) -> Option<&Map<String, Value>> {
    match data {
        Value::Object(obj) => match obj.get("Condition") {
            Some(Value::Object(cond)) => Some(cond),
            _ => None,
        },
        _ => None,
    }
}

pub fn resolve_config_id(scene_opt: Option<&Scene>, net_or_cfg: i64) -> i32 {
    if let Some(scene) = scene_opt
        && let Some(s) = scene.entity(net_or_cfg)
    {
        return s.config.config_id;
    }
    i32::try_from(net_or_cfg)
        .ok()
        .filter(|&id| id > 0)
        .unwrap_or(0)
}

fn matches_reach_role_option<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cond: &Map<String, Value>,
) -> bool {
    let Some(Value::Array(opts)) = cond.get("MatchRoleOption") else {
        return true;
    };
    if opts.is_empty() {
        return true;
    }
    let Some(scene) = ctx.scene.as_deref() else {
        return false;
    };
    let Ok(form_idx) = usize::try_from(scene.formation_info.cur_formation) else {
        return false;
    };
    let Some(formation) = scene.formation_info.formations.get(form_idx) else {
        return false;
    };
    for opt in opts {
        let Value::Object(obj) = opt else {
            continue;
        };
        let otype = json_str(obj.get("Type")).unwrap_or("");
        if otype == "Player" {
            if formation.cur_role > 0 {
                return true;
            }
        } else if otype == "Phantom" {
            let pid = json_int(obj.get("Id")).unwrap_or(0);
            if pid <= 0 || !scene.trial_role_ids.is_empty() {
                return true;
            }
            if formation
                .roles
                .iter()
                .flatten()
                .any(|r| r.role_id == pid)
            {
                return true;
            }
            if !ctx.echo_comp.echo_map.is_empty() {
                return true;
            }
        } else {
            return true;
        }
    }
    false
}

fn check_reach_entity_range<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    eid: i32,
    check_pos: [f32; 3],
    default_range: f32,
) -> bool {
    let mut ent_pos: Option<[f32; 3]> = None;
    let mut ent_rot = [0.0f32; 3];
    if let Some(scene) = ctx.scene.as_deref() {
        for s in &scene.entities {
            if s.config.config_id == eid
                && let Some(p) = s.position
            {
                ent_pos = Some(p.location);
                ent_rot = p.rotation;
                break;
            }
        }
    }
    let mut shape = ShapeConfig::default();
    if let Some(scene) = ctx.scene.as_deref()
        && let Some(lcfg) = ctx.assets.find_level_entity(scene.instance_id, i64::from(eid))
    {
        if ent_pos.is_none() {
            ent_pos = entity_world_pos(&lcfg);
            ent_rot = entity_world_rot(&lcfg);
        }
        if let Some(sh) = lcfg
            .ComponentsData
            .RangeComponent
            .as_ref()
            .and_then(|rc| rc.Shape.clone())
            .or_else(|| {
                ctx.assets
                    .find_template_config(&lcfg.BlueprintType)
                    .and_then(|tpl| {
                        tpl.ComponentsData
                            .RangeComponent
                            .as_ref()
                            .and_then(|rc| rc.Shape.clone())
                    })
            })
        {
            shape = sh;
        }
    }
    let Some(base) = ent_pos else {
        return false;
    };
    condition::is_pos_inside_range_shape(check_pos, base, ent_rot, &shape, default_range, 600.0)
}

pub fn matches_reach_area_pub<S: PushSink + ?Sized>(
    ctx: &mut QuestContext<'_, S>,
    quest_id: i32,
    node_data: &Value,
    cond: &Map<String, Value>,
    pos: [f32; 3],
) -> bool {
    matches_reach_area(ctx, quest_id, node_data, cond, pos)
}

pub fn matches_reach_area<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    quest_id: i32,
    node_data: &Value,
    cond: &Map<String, Value>,
    pos: [f32; 3],
) -> bool {
    if let Some(pre_val) = cond.get("PreConditions")
        && !condition::eval_inline_condition_block(
            &ctx.make_eval_context_for_quest(quest_id),
            pre_val,
        )
    {
        return false;
    }
    if !matches_reach_role_option(ctx, cond) {
        return false;
    }

    let mut check_pos = pos;
    if let Some(target_eid) = json_int(cond.get("EntityId"))
        && target_eid > 0
    {
        let Some(scene) = ctx.scene.as_deref() else {
            return false;
        };
        let Some(found_pos) = scene
            .entities
            .iter()
            .find(|s| s.config.config_id == target_eid)
            .and_then(|s| s.position.map(|p| p.location))
        else {
            return false;
        };
        check_pos = found_pos;
    }

    let range = json_float(cond.get("Range")).unwrap_or(1000.0).max(600.0);

    if let Some(Value::Array(re)) = cond.get("RangeEntities")
        && !re.is_empty()
    {
        return re.iter().any(|ev| {
            json_int(Some(ev)).is_some_and(|eid| check_reach_entity_range(ctx, eid, check_pos, range))
        });
    }
    if let Some(reid) = json_int(cond.get("RangeEntityId"))
        && reid > 0
    {
        return check_reach_entity_range(ctx, reid, check_pos, range);
    }
    if let Some(Value::Object(pv)) = cond.get("Pos")
        && check_point(pv, check_pos, range)
    {
        return true;
    }
    if let Some(Value::Object(cv)) = cond.get("Center")
        && check_point(cv, check_pos, range)
    {
        return true;
    }
    if check_point_array(cond.get("Locations"), check_pos, range) {
        return true;
    }
    if let Some(Value::Object(tt)) = node_data.get("TrackTarget")
        && let Some(Value::Object(track_type)) = tt.get("TrackType")
        && check_point_array(track_type.get("Locations"), check_pos, range)
    {
        return true;
    }
    false
}

fn check_point_array(arr_opt: Option<&Value>, pos: [f32; 3], range: f32) -> bool {
    let Some(Value::Array(items)) = arr_opt else {
        return false;
    };
    items
        .iter()
        .any(|pt| matches!(pt, Value::Object(obj) if check_point(obj, pos, range)))
}

fn check_point(pt: &Map<String, Value>, pos: [f32; 3], range: f32) -> bool {
    let Some(tx) = json_float(pt.get("X")) else {
        return false;
    };
    let Some(ty) = json_float(pt.get("Y")) else {
        return false;
    };
    let tz = json_float(pt.get("Z")).unwrap_or(pos[2]);
    let dx = pos[0] - tx;
    let dy = pos[1] - ty;
    let dz = pos[2] - tz;
    dx * dx + dy * dy + dz * dz <= range * range
}

pub fn matches_quest_interact_starter<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    d: &QuestDataEntry,
    entity_id: i64,
    cfg_id: i32,
) -> bool {
    if let Some(Value::Object(aio)) = &d.AddInteractOption
        && let Some(eid) = json_int(aio.get("EntityId"))
        && (eid == cfg_id || i64::from(eid) == entity_id)
    {
        return true;
    }
    let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(d.Id) else {
        return false;
    };
    for n in nodes.iter() {
        if n.node_type != "ChildQuest" {
            continue;
        }
        let Some(cond) = get_child_condition(&n.data) else {
            continue;
        };
        if json_str(cond.get("Type")) == Some("DoInteract")
            && match_do_interact(cond, entity_id, cfg_id, 0).is_some()
        {
            return true;
        }
    }
    false
}

pub fn match_do_interact(
    cond: &Map<String, Value>,
    entity_id: i64,
    cfg_id: i32,
    option_index: i32,
) -> Option<&Map<String, Value>> {
    let Some(Value::Array(opts)) = cond.get("AddOptions") else {
        return None;
    };
    let mut matched_any = None;
    let mut match_i = 0i32;
    for opt in opts {
        let Value::Object(obj) = opt else {
            continue;
        };
        let Some(eid) = json_int(obj.get("EntityId")) else {
            continue;
        };
        if eid == cfg_id || i64::from(eid) == entity_id {
            if match_i == option_index {
                return Some(obj);
            }
            if matched_any.is_none() {
                matched_any = Some(obj);
            }
            match_i += 1;
        }
    }
    matched_any
}

pub fn matches_hand_in_entity(cond: &Map<String, Value>, entity_id: i64, cfg_id: i32) -> bool {
    let add_opt = cond.get("AddOption").or_else(|| {
        cond.get("HandInItems")
            .and_then(|v| v.as_object())
            .and_then(|hi| hi.get("AddOption"))
    });
    let Some(Value::Object(obj)) = add_opt else {
        return false;
    };
    let Some(eid) = json_int(obj.get("EntityId")) else {
        return false;
    };
    eid == cfg_id || i64::from(eid) == entity_id
}

pub fn is_kill_satisfied(aq: &ActiveQuest, cond: &Map<String, Value>, ctype: &str) -> bool {
    if ctype == "MonsterCreator" {
        let Some(Value::Array(list)) = cond.get("MonsterCreatorEntityIds") else {
            return !aq.killed_entities.is_empty();
        };
        if list.is_empty() {
            return !aq.killed_entities.is_empty();
        }
        return list
            .iter()
            .filter_map(|v| json_int(Some(v)))
            .all(|eid| aq.has_killed_entity(eid));
    }

    let mut total = 0usize;
    let mut killed = 0usize;
    for field in ["TargetsToAwake", "ExistTargets"] {
        if let Some(Value::Array(arr)) = cond.get(field) {
            for v in arr {
                let Some(eid) = json_int(Some(v)) else {
                    continue;
                };
                total += 1;
                if aq.has_killed_entity(eid) {
                    killed += 1;
                }
            }
        }
    }
    if total == 0 {
        !aq.killed_entities.is_empty()
    } else {
        killed >= total
    }
}

pub fn eval_compare_var_list<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cond: &Map<String, Value>,
) -> bool {
    let Some(Value::Array(conds)) = cond.get("Conditions") else {
        return true;
    };
    if conds.is_empty() {
        return true;
    }
    let needed = json_int(cond.get("Count")).unwrap_or(0);
    let eval_ctx = ctx.make_eval_context();
    let passed = conds
        .iter()
        .filter_map(|item| item.as_object())
        .filter(|obj| condition::eval_compare_var_condition(&eval_ctx, obj))
        .count() as i32;
    if needed <= 0 {
        passed == conds.len() as i32
    } else {
        passed >= needed
    }
}

pub fn is_level_play_satisfied<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cond: &Map<String, Value>,
) -> bool {
    let Some(Value::Array(ids)) = cond.get("LevelPlayIds") else {
        return false;
    };
    if ids.is_empty() {
        return false;
    }
    let req = json_int(cond.get("CompleteNumber")).unwrap_or(1).max(1);
    let done = ids
        .iter()
        .filter_map(|v| json_int(Some(v)))
        .filter(|&lpid| ctx.quest_comp.info.completed_level_plays.contains(&lpid))
        .count() as i32;
    done >= req
}

pub fn has_all_items<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    items_opt: Option<&Value>,
) -> bool {
    let Some(Value::Array(arr)) = items_opt else {
        return true;
    };
    for it in arr {
        let Value::Object(obj) = it else {
            continue;
        };
        let Some(iid) = json_int(obj.get("ItemId")) else {
            continue;
        };
        let cnt = json_int(obj.get("Count")).unwrap_or(1);
        let owned = if iid == 2 {
            ctx.basic_comp.info.attributes.coin
        } else if iid == 3 {
            ctx.basic_comp
                .info
                .attributes
                .rare_coin
                .max(ctx.inventory_comp.info.normal_item_count(3))
        } else {
            ctx.inventory_comp.info.normal_item_count(iid)
        };
        if owned < cnt {
            return false;
        }
    }
    true
}

pub fn nested_dungeon_id(cond: &Map<String, Value>, field: &str) -> i32 {
    cond.get(field)
        .and_then(|v| v.as_object())
        .and_then(|sub| json_int(sub.get("DungeonId")))
        .unwrap_or(0)
}

pub fn is_check_entity_state_satisfied<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cond: &Map<String, Value>,
) -> bool {
    let Some(Value::Array(cs)) = cond.get("Conditions") else {
        return true;
    };
    if cs.is_empty() {
        return true;
    }
    for item in cs {
        let Value::Object(obj) = item else {
            continue;
        };
        let Some(eid) = json_int(obj.get("EntityId")) else {
            continue;
        };
        let Some(want_state) = json_str(obj.get("State")) else {
            continue;
        };
        if !tree::entity_has_state_tag(ctx, eid, want_state) {
            return false;
        }
    }
    true
}

pub fn entity_has_tag_id<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    target_cfg: i32,
    tag_id: i32,
) -> bool {
    if target_cfg <= 0 || tag_id == 0 {
        return false;
    }
    let Some(scene) = ctx.scene.as_deref() else {
        return false;
    };
    scene.entities.iter().any(|s| {
        s.config.config_id == target_cfg
            && s.tag
                .as_ref()
                .is_some_and(|t| t.has_tag(&ctx.assets.tables.gameplay_tag_parent, tag_id))
    })
}

pub fn entity_hp_percent<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    target_cfg: i32,
) -> Option<f32> {
    if target_cfg <= 0 {
        return None;
    }
    let scene = ctx.scene.as_deref()?;
    let life_idx = EAttributeType::Life.to_i32() as usize;
    let max_idx = EAttributeType::LifeMax.to_i32() as usize;
    for s in &scene.entities {
        if s.config.config_id != target_cfg {
            continue;
        }
        let Some(attr) = &s.attribute else {
            continue;
        };
        let max_hp = attr.attributes[max_idx].current;
        if max_hp <= 0 {
            continue;
        }
        let cur_hp = attr.attributes[life_idx].current;
        return Some(cur_hp as f32 / max_hp as f32 * 100.0);
    }
    None
}

pub fn is_battle_attr_or_wait_satisfied<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cond: &Map<String, Value>,
    ctype: &str,
    hit_cfg_id: i32,
    hp_ratio: f32,
) -> bool {
    let Some(Value::Array(opts)) = cond.get("Option") else {
        return true;
    };
    if opts.is_empty() {
        return true;
    }
    let req_count = json_int(cond.get("Count")).unwrap_or(0);
    let mut passed = 0i32;

    for o in opts {
        let Value::Object(obj) = o else {
            continue;
        };
        if ctype == "CheckTargetBattleAttribute" {
            let inner = obj.get("Option").and_then(|v| v.as_object());
            let min_pct = inner.and_then(|iv| json_float(iv.get("Min"))).unwrap_or(0.0);
            let max_pct = inner
                .and_then(|iv| json_float(iv.get("Max")))
                .unwrap_or(100.0);
            if let Some(Value::Array(eids)) = obj.get("EntityIds") {
                for v in eids {
                    let Some(eid) = json_int(Some(v)) else {
                        continue;
                    };
                    let pct_opt = if hit_cfg_id != 0 && eid == hit_cfg_id {
                        Some(hp_ratio * 100.0)
                    } else {
                        entity_hp_percent(ctx, eid)
                    };
                    if let Some(pct) = pct_opt
                        && pct >= min_pct
                        && pct <= max_pct
                    {
                        passed += 1;
                        break;
                    }
                }
            } else if json_str(obj.get("Type")) == Some("Player") {
                passed += 1;
            }
        } else {
            let eid = json_int(obj.get("EntityId")).unwrap_or(0);
            let state_str = json_str(obj.get("State")).unwrap_or("");
            let tag_cfg_id = json_int(obj.get("TagConfigId")).unwrap_or(0);
            let has_tag_or_state = !state_str.is_empty() || tag_cfg_id != 0;
            if (!state_str.is_empty() && tree::entity_has_state_tag(ctx, eid, state_str))
                || (tag_cfg_id != 0 && entity_has_tag_id(ctx, eid, tag_cfg_id))
                || (!has_tag_or_state
                    && hit_cfg_id != 0
                    && (eid == 0 || eid == hit_cfg_id)
                    && hp_ratio <= 0.75)
            {
                passed += 1;
            }
        }
    }

    let needed = if req_count <= 0 {
        opts.len() as i32
    } else {
        req_count
    };
    passed >= needed
}

pub fn is_schedule_time_satisfied<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    cond: &Map<String, Value>,
) -> bool {
    let h = if ctx.quest_comp.info.env.time_locked {
        ctx.quest_comp.info.env.time_lock_hour
    } else if let Some(s) = ctx.scene.as_deref() {
        s.instance.map_time.hour
    } else {
        ctx.quest_comp.info.env.time_lock_hour
    };
    let m = if ctx.quest_comp.info.env.time_locked {
        ctx.quest_comp.info.env.time_lock_minute
    } else if let Some(s) = ctx.scene.as_deref() {
        s.instance.map_time.minute
    } else {
        ctx.quest_comp.info.env.time_lock_minute
    };
    let d = ctx
        .scene
        .as_deref()
        .map_or(1, |s| s.instance.map_time.day);
    is_schedule_time_window_match(cond, h, m, d)
}

pub fn is_schedule_time_window_match(
    cond: &Map<String, Value>,
    cur_hour: i32,
    cur_min: i32,
    cur_day: u32,
) -> bool {
    if let Some(req_day) = json_int(cond.get("Day"))
        && req_day > 0
        && cur_day < req_day as u32
    {
        return false;
    }
    let Some(Value::Object(start_obj)) = cond.get("StartTime") else {
        return true;
    };
    let sh = json_int(start_obj.get("Hour")).unwrap_or(0);
    let sm = json_int(start_obj.get("Minute")).unwrap_or(0);
    let cur_total = cur_hour.rem_euclid(24) * 60 + cur_min.rem_euclid(60);
    let start_total = sh.rem_euclid(24) * 60 + sm.rem_euclid(60);

    if let Some(Value::Object(end_obj)) = cond.get("EndTime") {
        let eh = json_int(end_obj.get("Hour")).unwrap_or(sh);
        let em = json_int(end_obj.get("Minute")).unwrap_or(sm);
        let end_total = eh.rem_euclid(24) * 60 + em.rem_euclid(60);
        if start_total <= end_total {
            return cur_total >= start_total && cur_total <= end_total;
        }
        return cur_total >= start_total || cur_total <= end_total;
    }
    cur_total >= start_total
}

pub fn matches_prologue_single_role_switch_guide<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    quest_id: i32,
    guide_group_id: i32,
) -> bool {
    if quest_id != PROLOGUE_QUEST_ID || !matches!(guide_group_id, 10043 | 10044) {
        return false;
    }
    ctx.scene.as_deref().is_some_and(|scene| {
        let idx = usize::try_from(scene.formation_info.cur_formation).unwrap_or(0);
        scene.formation_info.formations.get(idx).is_some_and(|f| {
            f.roles
                .iter()
                .flatten()
                .filter(|fr| fr.entity_id > 0 && fr.role_id > 0)
                .count()
                < 2
        })
    })
}

pub(super) fn should_auto_complete_child_quest<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    quest_id: i32,
    node: &NodeEntry,
) -> bool {
    let Some(cond) = get_child_condition(&node.data) else {
        return true;
    };
    let Some(ctype) = json_str(cond.get("Type")) else {
        return true;
    };

    if matches!(
        ctype,
        "ReachArea"
            | "PlayFlow"
            | "DoInteract"
            | "HandInItems"
            | "UseSkill"
            | "FinishDungeon"
            | "DetectCombatState"
            | "DetectCombatState2"
            | "UseItem"
            | "Parkour"
            | "RhythmGame"
            | "SlidingBlocks"
            | "Deliver"
            | "Communicate"
            | "CheckPlayerInput"
            | "FinishArcadeGameplay"
            | "SpringManorGameplayClose"
            | "CompareActorVar"
            | "CrossSceneSyncKillMonster"
            | "FinishFlipper"
            | "FinishKuroSimpleCombatKillMonster"
            | "FinishKurotato"
            | "FinishRoleTower"
            | "FinishRollBlock"
            | "FinishSurvivorsRouge"
            | "FinishTetris"
            | "FinishTrapDefense"
            | "NewHandInItems"
            | "ParallaxAlign"
            | "PlaybackShowTimeline"
            | "TakePicturesWithTimeScale"
            | "WalkingPattern"
            | "UseWeatherSwitch"
            | "WaitUntilLevelSequenceReachMark"
    ) {
        return false;
    }

    if let Some(delay_ms) = tree::parse_timer_delay_ms(cond) {
        return ctx.scene.is_none() || delay_ms <= 0;
    }

    match ctype {
        "ScheduleTime" => is_schedule_time_satisfied(ctx, cond),
        "EnterDungeon" => {
            let target = nested_dungeon_id(cond, "EnterType");
            ctx.scene
                .as_deref()
                .is_some_and(|s| target > 0 && s.instance_id == target)
        }
        "LeaveDungeon" => ctx.scene.as_deref().is_none_or(|s| s.instance_id == 8),
        "Kill" | "MonsterCreator" => ctx
            .quest_comp
            .info
            .find_active_quest(quest_id)
            .is_some_and(|aq| is_kill_satisfied(aq, cond, ctype)),
        "Guide" => {
            if GameConfig::embedded().autocomplete_guides {
                return true;
            }
            let gid = json_int(cond.get("GuideGroupId")).unwrap_or(0);
            if matches_prologue_single_role_switch_guide(ctx, quest_id, gid) {
                return true;
            }
            if let Some(gc) = ctx.guide_comp.as_deref()
                && gid > 0
                && gc.info.has_finished(gid)
            {
                return true;
            }
            false
        }
        "CheckConditionGroup" | "CheckClientConditionGroup" => {
            let eval_ctx = ctx.make_eval_context_for_quest(quest_id);
            if let Some(sub) = cond.get("Condition") {
                condition::eval_inline_condition_block(&eval_ctx, sub)
            } else if let Some(gid) = json_int(cond.get("GroupId")) {
                condition::eval_condition_group(&eval_ctx, gid)
            } else {
                false
            }
        }
        "CompareVar" => eval_compare_var_list(ctx, cond),
        "CheckLevelPlay" => is_level_play_satisfied(ctx, cond),
        "GetItem" => has_all_items(ctx, cond.get("Items")),
        "CheckEntityState" => is_check_entity_state_satisfied(ctx, cond),
        "TakePhoto" | "TakePhoto2" => ctx
            .quest_comp
            .info
            .has_photo_node_completed(quest_id, node.id),
        "ReadMail" => json_int(cond.get("MailId"))
            .filter(|&m| m > 0)
            .is_none_or(|mid| ctx.quest_comp.info.progress.read_mails.contains(&mid)),
        "ListenToEvent" => {
            let Some(Value::Object(et)) = cond.get("EventType") else {
                return false;
            };
            let et_type = json_str(et.get("Type")).unwrap_or("");
            if et_type == "EntityStateChange" {
                let eid = json_int(et.get("EntityId")).unwrap_or(0);
                let want_state = json_str(et.get("State")).unwrap_or("");
                if eid > 0
                    && ctx.scene.is_some()
                    && let Some(st) = ctx.quest_comp.info.get_entity_state(i64::from(eid))
                {
                    return want_state.is_empty() || want_state == st;
                }
            } else if et_type == "VarValueChange"
                && let Some(Value::Object(vobj)) = et.get("Var")
            {
                let cur = resolve_var_int_for_owner(ctx, quest_id, Some(&Value::Object(vobj.clone())));
                if let Some(want_v) = et.get("Value") {
                    return json_int(Some(want_v)).is_some_and(|w| w == cur);
                }
                return cur != 0;
            }
            false
        }
        "AwakeAndLoadEntity" => ctx
            .progress_comp
            .as_deref()
            .is_some_and(|pc| pc.info.get_key_int(&format!("awake_node:{quest_id}")) == i64::from(node.id)),
        "CheckUiGame" => ctx
            .progress_comp
            .as_deref()
            .is_some_and(|pc| has_completed_ui_gameplay(pc, cond)),
        "AtomicProcess" | "CheckSpringFestivalGameplayCompleted" => ctx
            .progress_comp
            .as_deref()
            .is_some_and(|pc| pc.info.has_claim(&format!("quest_sub:{quest_id}:{}", node.id))),
        "CheckActivityState" => {
            let Some(aid) = json_int(cond.get("ActivityId")) else {
                return true;
            };
            let want = json_int(cond.get("ActivityState")).unwrap_or(2);
            let cmp = json_str(cond.get("Compare")).unwrap_or("Eq");
            let cur = if condition::is_activity_open(&ctx.make_eval_context(), aid) {
                2
            } else {
                0
            };
            if cmp == "Ne" || cmp == "NotEq" {
                cur != want
            } else {
                cur == want
            }
        }
        "CheckTargetBattleAttribute" | "WaitBattleCondition" => {
            is_battle_attr_or_wait_satisfied(ctx, cond, ctype, 0, 1.0)
        }
        "VisionSystem" => {
            if let Some(Value::Object(qv)) = cond.get("Quest") {
                match json_str(qv.get("Type")).unwrap_or("") {
                    "HasVisionGetTargetLevel" => {
                        let req_lv = json_int(qv.get("TargetLevel")).unwrap_or(1);
                        return condition::current_calabash_level(
                            ctx.assets,
                            ctx.basic_comp,
                            ctx.echo_comp,
                        ) >= req_lv;
                    }
                    "VisionSetActivated" => {
                        if ctx
                            .progress_comp
                            .as_deref()
                            .is_some_and(|pc| pc.info.has_claim("vision:activated"))
                        {
                            return true;
                        }
                        return !ctx.echo_comp.echo_map.is_empty();
                    }
                    "DoVisionIdentify" => {
                        if ctx
                            .progress_comp
                            .as_deref()
                            .is_some_and(|pc| pc.info.has_claim("vision:identified"))
                        {
                            return true;
                        }
                        return !ctx.echo_comp.echo_map.is_empty();
                    }
                    _ => {}
                }
            }
            false
        }
        "ShowUi" => {
            if let Some(Value::Object(ut)) = cond.get("UiType") {
                let utype = json_str(ut.get("Type")).unwrap_or("");
                if matches!(
                    utype,
                    "All" | "CiacconaAvgBoard" | "ItemObtain" | "Cook" | "Synthetic" | "Forging"
                ) {
                    return false;
                }
            }
            true
        }
        "ProgramSpecialProcess" => ctx
            .progress_comp
            .as_deref()
            .is_some_and(|pc| pc.info.has_claim(&format!("quest_psp:{quest_id}:{}", node.id))),
        "InformationViewCheck" => json_int(cond.get("InformationViewId"))
            .filter(|&id| id > 0)
            .is_none_or(|id| {
                ctx.progress_comp
                    .as_deref()
                    .is_some_and(|pc| pc.info.has_claim(&format!("display_read:{id}")))
            }),
        "GetSkill" => json_int(cond.get("SkillId"))
            .filter(|&id| id > 0)
            .is_none_or(|sid| {
                ctx.progress_comp
                    .as_deref()
                    .is_some_and(|pc| pc.info.has_claim(&format!("skill_unlocked:{sid}")))
                    || ctx.scene.as_deref().is_some_and(|s| {
                        s.explore_tools_info.active_explore_skill == sid
                            || s.explore_tools_info.unlocked_explore_skills.contains(&sid)
                    })
                    || (json_str(cond.get("SkillType")) == Some("NormalSkill")
                        && owns_echo_with_phantom_skill(ctx, sid))
            }),
        "FinishBvbChallenge" => json_int(cond.get("BvbChallengeId"))
            .filter(|&id| id > 0)
            .is_none_or(|bvb_id| ctx.quest_comp.info.progress.completed_bvb.contains(&bvb_id)),
        "ReadPhoneMessage" => json_int(cond.get("PhoneMessageId"))
            .filter(|&id| id > 0)
            .is_some_and(|pmid| {
                ctx.quest_comp.info.progress.read_phone_messages.contains(&pmid)
            }),
        "ReceiveTelecom" => json_int(cond.get("TelecomId"))
            .filter(|&id| id > 0)
            .is_some_and(|tid| {
                ctx.quest_comp.info.progress.received_telecoms.contains(&tid)
            }),
        "CheckFinishLoading"
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
        | "CheckRoverRogueAbilitySelect" => {
            let eval_ctx = ctx.make_eval_context_for_quest(quest_id);
            condition::eval_single_inline_condition_obj(&eval_ctx, cond)
        }
        _ => false,
    }
}

/// `PhantomItem.SkillId` is the echo's `PhantomSkill.PhantomSkillId`; a non-trial echo
/// owning it means the player has that phantom skill.
fn owns_echo_with_phantom_skill<S: PushSink + ?Sized>(
    ctx: &QuestContext<'_, S>,
    skill_id: i32,
) -> bool {
    ctx.echo_comp.echo_map.iter().any(|(&inc_id, echo)| {
        inc_id < TRIAL_INCR_MIN
            && ctx
                .assets
                .tables
                .phantom_item
                .get_data_by_id(&echo.id)
                .is_some_and(|item| item.SkillId == skill_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn find_do_interact_option_by_guid_matches_owning_entry() {
        let cond = json!({
            "Type": "DoInteract",
            "AddOptions": [
                { "EntityId": 1, "Option": { "Type": { "Type": "Actions", "Actions": [] }, "Guid": "aaa" } },
                { "EntityId": 122000891, "Option": { "Type": { "Type": "Actions", "Actions": [] }, "Guid": "30e12028d6dd44c4ad30c72d7afcc113" } }
            ]
        });
        let cond_obj = cond.as_object().unwrap();
        let matched = find_do_interact_option_by_guid(cond_obj, "30e12028d6dd44c4ad30c72d7afcc113")
            .expect("expected matching option");
        assert_eq!(json_int(matched.get("EntityId")), Some(122000891));
        assert!(find_do_interact_option_by_guid(cond_obj, "missing").is_none());
        assert!(find_do_interact_option_by_guid(cond_obj, "").is_none());
    }
}
