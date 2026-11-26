use std::cell::Cell;
use std::collections::HashSet;
use std::io;

use serde_json::{Map, Value};
use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::asset_indexes::{entity_world_pos, entity_world_rot};
use sonoro_gamesv_data::lazy_node_store::NodeEntry;
use sonoro_gamesv_data::tables::entity_components::range::ShapeConfig;
use sonoro_gamesv_data::tables::level_play_data::LevelPlayData;
use sonoro_gamesv_fs::quest_info::append_unique_int;
use sonoro_gamesv_fs::quest_info::remove_int_value;
use sonoro_proto::enums::{BtType, ChildQuestNodeStatus, FailReason, GameCtxType, NodeStatus};
use sonoro_proto::pb::{
    BtRollbackNotify, BtRollbackStartNotify, BtVarUpdateNotify, BehaviorTreeInfoNotify,
    ChildQuestNodeInfo, EnterLevelPlayNotify, EntityAddNotify, EntityRemoveInfo,
    EntityRemoveNotify, LeaveLevelPlayNotify, LevelPlayFirstNotify, LevelPlayInfo,
    LevelPlayInfoNotify, LevelPlayStateNotify, NodeInfo, NodeInfoExtraInfoOneof, TimerInfoPb,
    TreeInfo, UpdateNodeStatusNotify,
};

use crate::component::entity::small::{
    ConfigComponent, ConfigType, EntityState, EntityType, MonsterCaptureComponent,
    PositionComponent, StateTagComponent,
};
use crate::component::entity::{EntityComponentStorage, TagComponent};
use crate::component::player::{
    PlayerBasicComponent, PlayerEchoComponent, PlayerGuideComponent, PlayerInventoryComponent,
    PlayerMailComponent, PlayerMapComponent, PlayerProgressComponent, PlayerQuestComponent,
    PlayerRoleComponent, PlayerSceneComponent, PlayerWeaponComponent,
};
use crate::event_queue::EventQueue;
use crate::helpers::condition::{self, EvalContext};
use crate::helpers::gameplay_tags;
use crate::helpers::prologue_quest;
use crate::helpers::quest_actions;
use crate::helpers::quest_engine;
use crate::helpers::quest_types::{
    ActivityNotifyFn, DeferredBtVar, QuestContext, json_float, json_int, json_str,
    repeater_cooldown_key, subtree_wait_delay_ms,
};
use crate::helpers::reward::{self, RewardContext, RewardItem};
use crate::scene::{Scene, now_millis};
use crate::schedulers::quest_timer_queue::QuestTimerEntry;
use crate::sink::PushSink;

const NORMAL_MONSTER_RESPAWN_MS: i64 = 60_000;
const BOSS_MONSTER_RESPAWN_MS: i64 = 180_000;
const COLLECTIBLE_RESPAWN_MS: i64 = 300_000;
const ANIMAL_RESPAWN_MS: i64 = 60_000;
const MAX_STEPS_PER_ADVANCE: usize = 64;

thread_local! {
    static LP_STEP_DEPTH: Cell<u32> = const { Cell::new(0) };
    static LP_RECHECK_PENDING: Cell<bool> = const { Cell::new(false) };
}

struct StepDepthGuard;

impl StepDepthGuard {
    fn enter() -> Self {
        LP_STEP_DEPTH.with(|d| d.set(d.get().saturating_add(1)));
        Self
    }
}

impl Drop for StepDepthGuard {
    fn drop(&mut self) {
        LP_STEP_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

pub struct LevelPlayContext<'a, S: PushSink + ?Sized> {
    pub assets: &'a Assets,
    pub fs: &'a FileSystem,
    pub sink: &'a mut S,
    pub scene: &'a mut Scene,
    pub scene_comp: Option<&'a mut PlayerSceneComponent>,
    pub events: &'a mut EventQueue,
    pub basic_comp: &'a mut PlayerBasicComponent,
    pub inventory_comp: &'a mut PlayerInventoryComponent,
    pub weapon_comp: &'a mut PlayerWeaponComponent,
    pub echo_comp: &'a mut PlayerEchoComponent,
    pub map_comp: &'a mut PlayerMapComponent,
    pub quest_comp: &'a mut PlayerQuestComponent,
    pub role_comp: Option<&'a mut PlayerRoleComponent>,
    pub guide_comp: Option<&'a mut PlayerGuideComponent>,
    pub mail_comp: Option<&'a mut PlayerMailComponent>,
    pub progress_comp: Option<&'a mut PlayerProgressComponent>,
    pub activity_notify: Option<ActivityNotifyFn<S>>,
}

pub struct ResolvedEntityConfig {
    pub blueprint_type: String,
    pub components: sonoro_gamesv_data::tables::entity_components::Components,
}

impl<'a, S: PushSink + ?Sized> LevelPlayContext<'a, S> {
    pub fn from_quest_ctx<'b>(
        qctx: &'b mut QuestContext<'a, S>,
    ) -> Option<LevelPlayContext<'b, S>> {
        let scene = qctx.scene.as_deref_mut()?;
        Some(LevelPlayContext {
            assets: qctx.assets,
            fs: qctx.fs,
            sink: qctx.sink,
            scene,
            scene_comp: qctx.scene_comp.as_deref_mut(),
            events: qctx.events,
            basic_comp: qctx.basic_comp,
            inventory_comp: qctx.inventory_comp,
            weapon_comp: qctx.weapon_comp,
            echo_comp: qctx.echo_comp,
            map_comp: qctx.map_comp,
            quest_comp: qctx.quest_comp,
            role_comp: qctx.role_comp.as_deref_mut(),
            guide_comp: qctx.guide_comp.as_deref_mut(),
            mail_comp: qctx.mail_comp.as_deref_mut(),
            progress_comp: qctx.progress_comp.as_deref_mut(),
            activity_notify: qctx.activity_notify,
        })
    }

    pub fn as_quest_ctx(&mut self) -> QuestContext<'_, S> {
        QuestContext {
            assets: self.assets,
            fs: self.fs,
            sink: self.sink,
            scene: Some(self.scene),
            scene_comp: self.scene_comp.as_deref_mut(),
            events: self.events,
            basic_comp: self.basic_comp,
            inventory_comp: self.inventory_comp,
            weapon_comp: self.weapon_comp,
            echo_comp: self.echo_comp,
            map_comp: self.map_comp,
            quest_comp: self.quest_comp,
            role_comp: self.role_comp.as_deref_mut(),
            guide_comp: self.guide_comp.as_deref_mut(),
            mail_comp: self.mail_comp.as_deref_mut(),
            progress_comp: self.progress_comp.as_deref_mut(),
            activity_notify: self.activity_notify,
        }
    }

    pub fn as_reward_ctx(&mut self) -> RewardContext<'_, S> {
        RewardContext {
            fs: self.fs,
            assets: self.assets,
            sink: self.sink,
            basic_comp: self.basic_comp,
            inventory_comp: self.inventory_comp,
            weapon_comp: self.weapon_comp,
            echo_comp: self.echo_comp,
            progress_comp: self.progress_comp.as_deref_mut(),
        }
    }

    pub fn make_eval_context(&self) -> EvalContext<'_> {
        EvalContext {
            assets: Some(self.assets),
            basic_comp: self.basic_comp,
            echo_comp: self.echo_comp,
            inventory_comp: self.inventory_comp,
            map_comp: self.map_comp,
            quest_comp: self.quest_comp,
            role_comp: self.role_comp.as_deref(),
            weapon_comp: Some(&*self.weapon_comp),
            guide_comp: self.guide_comp.as_deref(),
            progress_comp: self.progress_comp.as_deref(),
            scene: Some(&*self.scene),
            owner_quest_id: 0,
            target_role_id: 0,
            self_entity_config_id: 0,
        }
    }
}

pub fn resolve_entity_config(
    assets: &Assets,
    scene_map_id: i32,
    config_id: i64,
) -> Option<ResolvedEntityConfig> {
    if config_id <= 0 {
        return None;
    }
    if let Some(cfg) = assets
        .find_level_entity(scene_map_id, config_id)
        .or_else(|| assets.find_level_entity_any_map(config_id))
    {
        let mut components = cfg.ComponentsData.clone();
        if let Some(tpl) = assets.find_template_config(&cfg.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut components);
        }
        return Some(ResolvedEntityConfig {
            blueprint_type: cfg.BlueprintType.clone(),
            components,
        });
    }
    let tid = i32::try_from(config_id).ok()?;
    let tpl = assets.tables.template_config.get_data_by_id(&tid)?;
    Some(ResolvedEntityConfig {
        blueprint_type: tpl.BlueprintType.clone(),
        components: tpl.ComponentsData.clone(),
    })
}

fn matches_scene_instance(assets: &Assets, lp: &LevelPlayData, scene_instance_id: i32) -> bool {
    let expected_inst = if lp.Data.InstanceId != 0 {
        lp.Data.InstanceId
    } else {
        lp.Data.LevelId
    };
    if expected_inst == 0 || expected_inst == scene_instance_id {
        return true;
    }
    assets.resolve_map_config_id(scene_instance_id) == expected_inst
}

fn json_array_contains_int(arr_opt: Option<&Value>, needle: i32) -> bool {
    let Some(Value::Array(items)) = arr_opt else {
        return false;
    };
    items.iter().any(|item| json_int(Some(item)) == Some(needle))
}

fn is_target_still_alive_in_scene(scene: &Scene, cfg_id: i32, exclude_net_id: i64) -> bool {
    scene.entities.iter().any(|s| {
        s.net_id != exclude_net_id
            && !scene.rewarded_kills.contains(&s.net_id)
            && s.config.config_id == cfg_id
    })
}

fn ensure_entity_spawned<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    config_id: i32,
) {
    if config_id <= 0 {
        return;
    }
    if ctx
        .scene
        .entities
        .iter()
        .any(|s| s.config.config_id == config_id)
    {
        return;
    }
    if ctx.quest_comp.info.has_opened_chest(config_id) {
        return;
    }
    prologue_quest::spawn_quest_entity(
        ctx.events,
        ctx.scene,
        ctx.fs,
        ctx.assets,
        ctx.sink,
        ctx.basic_comp,
        i64::from(config_id),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LpStepResult {
    Running,
    Completed,
    Failed,
    TreeEnded,
}

fn lp_key(prefix: &str, a: i32, b: i32) -> String {
    format!("{prefix}:{a}:{b}")
}

fn is_lp_node_active(
    progress_comp: Option<&PlayerProgressComponent>,
    lp_id: i32,
    node_id: i32,
) -> bool {
    let Some(pc) = progress_comp else {
        return false;
    };
    pc.info.get_key_int(&lp_key("lp_act", lp_id, node_id)) != 0
}

fn set_lp_node_active<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
    active: bool,
    dirty_pc: &mut bool,
) {
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return;
    };
    let key = lp_key("lp_act", lp_id, node_id);
    let val = i64::from(active);
    if pc.info.get_key_int(&key) != val {
        pc.info.set_key_int(&key, val);
        *dirty_pc = true;
    }
}

fn is_level_play_node_marked(
    progress_comp: Option<&PlayerProgressComponent>,
    lp_id: i32,
    node_id: i32,
) -> bool {
    let Some(pc) = progress_comp else {
        return false;
    };
    let key = lp_key("lp_node", lp_id, node_id);
    pc.info.has_claim(&key) || pc.info.get_key_int(&key) != 0
}

fn mark_lp_node_finished<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
    dirty_pc: &mut bool,
) {
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return;
    };
    let node_key = lp_key("lp_node", lp_id, node_id);
    if pc.info.mark_claim(&node_key) {
        *dirty_pc = true;
    }
    if pc.info.get_key_int(&node_key) == 0 {
        pc.info.set_key_int(&node_key, 1);
        *dirty_pc = true;
    }
    let act_key = lp_key("lp_act", lp_id, node_id);
    if pc.info.get_key_int(&act_key) != 0 {
        pc.info.set_key_int(&act_key, 0);
        *dirty_pc = true;
    }
}

fn clear_lp_node_finished<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
    dirty_pc: &mut bool,
) {
    set_lp_node_active(ctx, lp_id, node_id, false, dirty_pc);
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return;
    };
    let node_key = lp_key("lp_node", lp_id, node_id);
    if pc.info.remove_claim(&node_key) {
        *dirty_pc = true;
    }
    if pc.info.get_key_int(&node_key) != 0 {
        pc.info.set_key_int(&node_key, 0);
        *dirty_pc = true;
    }
}

fn is_lp_bt_booted(progress_comp: Option<&PlayerProgressComponent>, lp_id: i32) -> bool {
    let Some(pc) = progress_comp else {
        return false;
    };
    pc.info.has_claim(&format!("lp_bt:{lp_id}"))
}

fn mark_lp_bt_booted<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    dirty_pc: &mut bool,
) {
    let Some(pc) = ctx.progress_comp.as_deref_mut() else {
        return;
    };
    if pc.info.mark_claim(&format!("lp_bt:{lp_id}")) {
        *dirty_pc = true;
    }
}

fn find_lp_node_by_id(nodes: &[NodeEntry], node_id: i32) -> Option<&NodeEntry> {
    nodes.iter().find(|n| n.id == node_id)
}

fn find_lp_root_node(nodes: &[NodeEntry]) -> Option<&NodeEntry> {
    nodes.iter().find(|n| n.parent_id == 0)
}

/// Level-play counterpart of `is_subtree_active_for_monitor`: a `QuestFailed` monitor only
/// runs while `step_level_play_node` reaches it, i.e. no ancestor has finished and, under a
/// `Sequence`, every earlier sibling has completed (a Sequence stops at its first unfinished
/// child).
pub fn is_lp_failed_monitor_active(
    progress_comp: Option<&PlayerProgressComponent>,
    lp_id: i32,
    nodes: &[NodeEntry],
    node: &NodeEntry,
) -> bool {
    if is_level_play_node_marked(progress_comp, lp_id, node.id) {
        return false;
    }
    let mut cur = node;
    for _ in 0..32 {
        if cur.parent_id <= 0 {
            return true;
        }
        let Some(parent) = find_lp_node_by_id(nodes, cur.parent_id) else {
            return false;
        };
        if is_level_play_node_marked(progress_comp, lp_id, parent.id) {
            return false;
        }
        if parent.node_type == "Sequence"
            && nodes.iter().take_while(|n| n.id != cur.id).any(|n| {
                n.parent_id == parent.id && !is_level_play_node_marked(progress_comp, lp_id, n.id)
            })
        {
            return false;
        }
        cur = parent;
    }
    false
}

fn resolve_level_play_anchor_pos<S: PushSink + ?Sized>(
    ctx: &LevelPlayContext<'_, S>,
    lp: &LevelPlayData,
) -> Option<[f32; 3]> {
    let mut cfg_id = lp.Data.LevelPlayEntityId;
    if cfg_id <= 0
        && let Some(rc) = &lp.Data.LevelPlayRewardConfig
    {
        cfg_id = rc.RewardEntityId;
    }
    if cfg_id <= 0 {
        return None;
    }
    for s in &ctx.scene.entities {
        if s.config.config_id == cfg_id
            && let Some(pos) = s.position
        {
            return Some(pos.location);
        }
    }
    let map_id = if lp.Data.LevelId > 0 {
        lp.Data.LevelId
    } else {
        ctx.scene.instance_id
    };
    ctx.assets
        .find_level_entity(map_id, i64::from(cfg_id))
        .and_then(|cfg| entity_world_pos(&cfg))
}

fn current_player_pos<S: PushSink + ?Sized>(ctx: &LevelPlayContext<'_, S>) -> Option<[f32; 3]> {
    let cur_eid = ctx.scene.current_role_entity_id();
    if cur_eid > 0
        && let Some(s) = ctx.scene.entity(cur_eid)
        && let Some(pos) = s.position
    {
        return Some(pos.location);
    }
    ctx.scene.instance.players.first().map(|p| p.location)
}

fn is_player_within_reach_area<S: PushSink + ?Sized>(
    ctx: &LevelPlayContext<'_, S>,
    node_data: &Value,
    cond: &Map<String, Value>,
    pos: [f32; 3],
) -> bool {
    matches_reach_area_inner(ctx, node_data, cond, pos)
}

fn matches_reach_area_inner<S: PushSink + ?Sized>(
    ctx: &LevelPlayContext<'_, S>,
    node_data: &Value,
    cond: &Map<String, Value>,
    pos: [f32; 3],
) -> bool {
    if let Some(pre) = cond.get("PreConditions")
        && !condition::eval_inline_condition_block(&ctx.make_eval_context(), pre)
    {
        return false;
    }

    let mut check_pos = pos;
    if let Some(target_eid) = json_int(cond.get("EntityId"))
        && target_eid > 0
    {
        let found = ctx.scene.entities.iter().find_map(|s| {
            (s.config.config_id == target_eid)
                .then(|| s.position.map(|p| p.location))
                .flatten()
        });
        let Some(p) = found else {
            return false;
        };
        check_pos = p;
    }

    let raw_range = json_float(cond.get("Range")).unwrap_or(1000.0);
    let range = raw_range.max(600.0);

    if let Some(Value::Array(entities)) = cond.get("RangeEntities")
        && !entities.is_empty()
    {
        return entities.iter().any(|ev| {
            let Some(eid) = json_int(Some(ev)) else {
                return false;
            };
            check_reach_entity_range(ctx, eid, check_pos, range)
        });
    }
    if let Some(reid) = json_int(cond.get("RangeEntityId"))
        && reid > 0
    {
        return check_reach_entity_range(ctx, reid, check_pos, range);
    }
    if let Some(Value::Object(pt)) = cond.get("Pos")
        && check_point(pt, check_pos, range)
    {
        return true;
    }
    if let Some(Value::Object(center)) = cond.get("Center")
        && check_point(center, check_pos, range)
    {
        return true;
    }
    if check_point_array(cond.get("Locations"), check_pos, range) {
        return true;
    }
    if let Some(Value::Object(obj)) = Some(node_data)
        && let Some(Value::Object(tt)) = obj.get("TrackTarget")
        && let Some(Value::Object(track_type)) = tt.get("TrackType")
        && check_point_array(track_type.get("Locations"), check_pos, range)
    {
        return true;
    }
    false
}

fn check_reach_entity_range<S: PushSink + ?Sized>(
    ctx: &LevelPlayContext<'_, S>,
    eid: i32,
    check_pos: [f32; 3],
    default_range: f32,
) -> bool {
    let mut ent_pos: Option<[f32; 3]> = None;
    let mut ent_rot = [0.0f32; 3];
    for s in &ctx.scene.entities {
        if s.config.config_id == eid
            && let Some(p) = s.position
        {
            ent_pos = Some(p.location);
            ent_rot = p.rotation;
            break;
        }
    }
    let mut shape = ShapeConfig::default();
    if let Some(lcfg) = ctx.assets.find_level_entity(ctx.scene.instance_id, i64::from(eid)) {
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

#[allow(clippy::too_many_arguments)]
pub fn sync_active_dynamic_interact_options<S: PushSink + ?Sized>(
    scene: &mut Scene,
    assets: &Assets,
    sink: &mut S,
    quest_info: &sonoro_gamesv_fs::quest_info::QuestInfo,
    progress_comp: Option<&PlayerProgressComponent>,
) -> io::Result<()> {
    sync_dynamic_interact_options_for_entity_mode(
        scene,
        assets,
        sink,
        quest_info,
        progress_comp,
        None,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn sync_dynamic_interact_options_for_entity<S: PushSink + ?Sized>(
    scene: &mut Scene,
    assets: &Assets,
    sink: &mut S,
    quest_info: &sonoro_gamesv_fs::quest_info::QuestInfo,
    progress_comp: Option<&PlayerProgressComponent>,
    cfg_id: i64,
) -> io::Result<()> {
    sync_dynamic_interact_options_for_entity_mode(
        scene,
        assets,
        sink,
        quest_info,
        progress_comp,
        Some(cfg_id),
        true,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn sync_dynamic_interact_options_for_entity_mode<S: PushSink + ?Sized>(
    scene: &mut Scene,
    assets: &Assets,
    sink: &mut S,
    quest_info: &sonoro_gamesv_fs::quest_info::QuestInfo,
    progress_comp: Option<&PlayerProgressComponent>,
    filter_cfg_id: Option<i64>,
    push_notifies: bool,
) -> io::Result<()> {
    if filter_cfg_id.is_some_and(|id| id <= 0) {
        return Ok(());
    }
    for &lp_id in &quest_info.progress.entered_level_plays {
        if quest_info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        for node in nodes.iter() {
            if !is_lp_node_active(progress_comp, lp_id, node.id) {
                continue;
            }
            quest_engine::push_do_interact_options_for_bt_node_mode(
                scene,
                sink,
                BtType::BtTypeLevelPlay,
                lp_id,
                node,
                filter_cfg_id,
                push_notifies,
            )?;
        }
    }
    Ok(())
}

fn condition_references_level_play_id(cond: &Map<String, Value>, lp_id: i32) -> bool {
    if json_int(cond.get("LevelPlayId")) == Some(lp_id) {
        return true;
    }
    if json_array_contains_int(cond.get("LevelPlayIds"), lp_id) {
        return true;
    }
    if let Some(Value::Array(items)) = cond.get("Conditions") {
        for item in items {
            if let Value::Object(obj) = item
                && condition_references_level_play_id(obj, lp_id)
            {
                return true;
            }
        }
    }
    false
}

fn append_condition_level_play_ids(out: &mut Vec<i32>, cond: &Map<String, Value>) {
    if let Some(lp_id) = json_int(cond.get("LevelPlayId"))
        && lp_id > 0
    {
        let _ = append_unique_int(out, lp_id);
    }
    if let Some(Value::Array(items)) = cond.get("LevelPlayIds") {
        for item in items {
            if let Some(lp_id) = json_int(Some(item))
                && lp_id > 0
            {
                let _ = append_unique_int(out, lp_id);
            }
        }
    }
}

fn is_quest_checking_level_play<S: PushSink + ?Sized>(
    ctx: &LevelPlayContext<'_, S>,
    lp_id: i32,
) -> bool {
    for aq in &ctx.quest_comp.info.active_quests {
        if aq.active_nodes.is_empty() {
            continue;
        }
        let Ok(nodes) = ctx.assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            let Some(node) = find_lp_node_by_id(&nodes, nid) else {
                continue;
            };
            if node.node_type != "ChildQuest" {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
                continue;
            };
            if json_str(cond.get("Type")) != Some("CheckLevelPlay") {
                continue;
            }
            if condition_references_level_play_id(cond, lp_id) {
                return true;
            }
        }
    }
    false
}

fn can_enter_level_play<S: PushSink + ?Sized>(
    ctx: &LevelPlayContext<'_, S>,
    lp: &LevelPlayData,
) -> bool {
    let lp_id = lp.LevelPlayId;
    if lp_id <= 0 || ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return false;
    }
    if !matches_scene_instance(ctx.assets, lp, ctx.scene.instance_id) {
        return false;
    }
    if lp.Data.Type != "Quest" {
        return true;
    }
    if ctx
        .progress_comp
        .as_deref()
        .is_some_and(|pc| pc.info.get_key_int(&format!("lp_state:{lp_id}")) == 2)
    {
        return true;
    }
    is_quest_checking_level_play(ctx, lp_id)
}

fn is_lp_child_quest_satisfied_now<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node: &NodeEntry,
    player_pos_opt: Option<[f32; 3]>,
) -> bool {
    let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
        return true;
    };
    let Some(ctype) = json_str(cond.get("Type")) else {
        return true;
    };
    if ctype == "ReachArea" {
        let Some(pos) = player_pos_opt.or_else(|| current_player_pos(ctx)) else {
            return false;
        };
        return is_player_within_reach_area(ctx, &node.data, cond, pos);
    }
    let qctx = ctx.as_quest_ctx();
    quest_engine::eval_child_condition_auto(&qctx, lp_id, node)
}

fn push_level_play_behavior_tree_info<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    nodes: &[NodeEntry],
) -> io::Result<()> {
    let mut tree_nodes = std::collections::BTreeMap::new();
    let mut timer_infos = Vec::new();
    for node in nodes {
        let finished = is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, node.id);
        let active = !finished && is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id);
        if !finished && !active {
            continue;
        }
        let status = if finished {
            NodeStatus::CompletedSuccess
        } else {
            NodeStatus::Activate
        };
        let extra = if node.node_type == "ChildQuest" {
            if active
                && let Some(due_ms) = ctx.scene.lp_timers.get_due_ms(lp_id, node.id)
            {
                timer_infos.push(TimerInfoPb {
                    TimerType: b"WaitTime".to_vec(),
                    NodeId: node.id,
                    EndTime: due_ms / 1000,
                    PauseTime: 0,
                });
            }
            Some(NodeInfoExtraInfoOneof::ChildQuestNodeInfo(
                ChildQuestNodeInfo {
                    Status: Some(if finished {
                        ChildQuestNodeStatus::Finished
                    } else {
                        ChildQuestNodeStatus::Progress
                    }),
                    Progress: None,
                },
            ))
        } else {
            None
        };
        tree_nodes.insert(
            node.id,
            NodeInfo {
                ExtraInfo: extra,
                Status: Some(status),
            },
        );
    }

    ctx.sink.push(
        BehaviorTreeInfoNotify::MSG_ID,
        &BehaviorTreeInfoNotify {
            TreeInfos: vec![TreeInfo {
                TreeOwnerId: lp_id,
                TreeIncId: i64::from(lp_id),
                BtType: Some(BtType::BtTypeLevelPlay),
                BlackboardId: lp_id,
                Nodes: tree_nodes,
                TimerInfos: timer_infos,
                ..Default::default()
            }],
        },
    );
    ctx.scene.sent_lp_bt_trees.insert(lp_id);
    Ok(())
}

fn finish_lp_non_child_node<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node: &NodeEntry,
    push_updates: bool,
    dirty_pc: &mut bool,
    mut deferred_bt_vars: Option<&mut Vec<DeferredBtVar>>,
) {
    ctx.scene.release_occupations_for_node(lp_id, node.id);
    if node.reward_id > 0 {
        let _ = reward::grant_drop_package(&mut ctx.as_reward_ctx(), node.reward_id, 6003);
    }
    mark_lp_node_finished(ctx, lp_id, node.id, dirty_pc);
    if push_updates {
        ctx.sink.push(
            UpdateNodeStatusNotify::MSG_ID,
            &UpdateNodeStatusNotify {
                TreeOwnerId: lp_id,
                TreeIncId: i64::from(lp_id),
                NodeId: node.id,
                Status: Some(NodeStatus::CompletedSuccess),
            },
        );
    }
    if let Value::Object(obj) = &node.data {
        if let Some(Value::Object(occ)) = obj.get("OccupationConfig")
            && let Some(exit_acts) = occ.get("ExitActions")
        {
            run_level_play_actions(
                ctx,
                lp_id,
                node.id,
                Some(exit_acts),
                deferred_bt_vars.as_deref_mut(),
            );
        }
        run_level_play_actions(ctx, lp_id, node.id, obj.get("FinishActions"), deferred_bt_vars);
    }
}

fn reset_lp_subtree_finished_nodes<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    nodes: &[NodeEntry],
    parent_id: i32,
    dirty_pc: &mut bool,
) {
    for n in nodes {
        if n.parent_id != parent_id {
            continue;
        }
        clear_lp_node_finished(ctx, lp_id, n.id, dirty_pc);
        reset_lp_subtree_finished_nodes(ctx, lp_id, nodes, n.id, dirty_pc);
    }
}

fn deactivate_lp_subtree<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    nodes: &[NodeEntry],
    parent_id: i32,
    push_updates: bool,
    dirty_pc: &mut bool,
) {
    for n in nodes {
        if n.parent_id != parent_id {
            continue;
        }
        ctx.scene.lp_timers.cancel(lp_id, n.id);
        ctx.scene.lp_fail_timers.cancel(lp_id, n.id);
        ctx.scene.release_occupations_for_node(lp_id, n.id);
        let was_active = is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, n.id);
        set_lp_node_active(ctx, lp_id, n.id, false, dirty_pc);
        let is_child = n.node_type == "ChildQuest";
        let mut qctx = ctx.as_quest_ctx();
        quest_engine::remove_do_interact_options_for_bt_node(
            &mut qctx,
            BtType::BtTypeLevelPlay,
            lp_id,
            n.id,
        );
        if push_updates {
            if was_active && is_child {
                let _ = quest_engine::push_bt_child_node_status(
                    &mut qctx,
                    BtType::BtTypeLevelPlay,
                    lp_id,
                    n,
                    NodeStatus::Destroy,
                    ChildQuestNodeStatus::NotActive,
                );
            } else if was_active || !is_child {
                ctx.sink.push(
                    UpdateNodeStatusNotify::MSG_ID,
                    &UpdateNodeStatusNotify {
                        TreeOwnerId: lp_id,
                        TreeIncId: i64::from(lp_id),
                        NodeId: n.id,
                        Status: Some(NodeStatus::Destroy),
                    },
                );
            }
        }
        deactivate_lp_subtree(ctx, lp_id, nodes, n.id, push_updates, dirty_pc);
    }
}

#[allow(clippy::too_many_arguments)]
fn complete_lp_child_quest<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node: &NodeEntry,
    push_updates: bool,
    dirty_pc: &mut bool,
    deferred_bt_vars: Option<&mut Vec<DeferredBtVar>>,
) -> io::Result<()> {
    ctx.scene.lp_timers.cancel(lp_id, node.id);
    if is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, node.id) {
        return Ok(());
    }
    mark_lp_node_finished(ctx, lp_id, node.id, dirty_pc);
    if *dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        let _ = pc.save(ctx.fs);
        *dirty_pc = false;
    }

    {
        let mut qctx = ctx.as_quest_ctx();
        quest_engine::remove_do_interact_options_for_bt_node(
            &mut qctx,
            BtType::BtTypeLevelPlay,
            lp_id,
            node.id,
        );
        if push_updates {
            quest_engine::push_bt_child_node_status(
                &mut qctx,
                BtType::BtTypeLevelPlay,
                lp_id,
                node,
                NodeStatus::CompletedSuccess,
                ChildQuestNodeStatus::Finished,
            )?;
        }
    }

    if let Value::Object(obj) = &node.data {
        run_level_play_actions(ctx, lp_id, node.id, obj.get("FinishActions"), deferred_bt_vars);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn step_level_play_node<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    nodes: &[NodeEntry],
    node: &NodeEntry,
    defer_bt_notifies: bool,
    push_updates: bool,
    player_pos_opt: Option<[f32; 3]>,
    newly_activated: &mut Vec<NodeEntry>,
    newly_actions: &mut Vec<NodeEntry>,
    deferred_bt_vars: &mut Option<&mut Vec<DeferredBtVar>>,
    dirty_pc: &mut bool,
    steps: &mut usize,
) -> io::Result<LpStepResult> {
    if *steps >= MAX_STEPS_PER_ADVANCE {
        return Ok(LpStepResult::Running);
    }
    *steps += 1;

    if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(LpStepResult::TreeEnded);
    }
    if is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, node.id) {
        return Ok(LpStepResult::Completed);
    }

    {
        let mut qctx = ctx.as_quest_ctx();
        if quest_engine::check_node_occupation_and_save_pub(
            &mut qctx,
            BtType::BtTypeLevelPlay,
            lp_id,
            node,
            push_updates,
        )? {
            return Ok(LpStepResult::Running);
        }
    }

    match node.node_type.as_str() {
        "Sequence" => {
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                let res = step_level_play_node(
                    ctx,
                    lp_id,
                    nodes,
                    child,
                    defer_bt_notifies,
                    push_updates,
                    player_pos_opt,
                    newly_activated,
                    newly_actions,
                    deferred_bt_vars,
                    dirty_pc,
                    steps,
                )?;
                match res {
                    LpStepResult::Running
                    | LpStepResult::Failed
                    | LpStepResult::TreeEnded => return Ok(res),
                    LpStepResult::Completed => continue,
                }
            }
            finish_lp_non_child_node(
                ctx,
                lp_id,
                node,
                push_updates,
                dirty_pc,
                deferred_bt_vars.as_deref_mut(),
            );
            Ok(LpStepResult::Completed)
        }
        "Select" => {
            let mut any_child = false;
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                any_child = true;
                let res = step_level_play_node(
                    ctx,
                    lp_id,
                    nodes,
                    child,
                    defer_bt_notifies,
                    push_updates,
                    player_pos_opt,
                    newly_activated,
                    newly_actions,
                    deferred_bt_vars,
                    dirty_pc,
                    steps,
                )?;
                match res {
                    LpStepResult::TreeEnded | LpStepResult::Running => return Ok(res),
                    LpStepResult::Completed => {
                        finish_lp_non_child_node(
                            ctx,
                            lp_id,
                            node,
                            push_updates,
                            dirty_pc,
                            deferred_bt_vars.as_deref_mut(),
                        );
                        return Ok(LpStepResult::Completed);
                    }
                    LpStepResult::Failed => continue,
                }
            }
            if any_child && node.parent_id != 0 {
                return Ok(LpStepResult::Failed);
            }
            finish_lp_non_child_node(
                ctx,
                lp_id,
                node,
                push_updates,
                dirty_pc,
                deferred_bt_vars.as_deref_mut(),
            );
            Ok(LpStepResult::Completed)
        }
        "Parallel" | "ParallelSelect" => {
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
                if is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, child.id) {
                    finished_count += 1;
                    continue;
                }
                let res = step_level_play_node(
                    ctx,
                    lp_id,
                    nodes,
                    child,
                    defer_bt_notifies,
                    push_updates,
                    player_pos_opt,
                    newly_activated,
                    newly_actions,
                    deferred_bt_vars,
                    dirty_pc,
                    steps,
                )?;
                match res {
                    LpStepResult::TreeEnded => {
                        any_ended = true;
                        break;
                    }
                    LpStepResult::Completed => {
                        finished_count += 1;
                        if !is_parallel_all && finished_count >= needed_sel {
                            break;
                        }
                    }
                    LpStepResult::Failed => failed_count += 1,
                    LpStepResult::Running => {}
                }
            }
            if any_ended || ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
                return Ok(LpStepResult::TreeEnded);
            }
            let needed = if is_parallel_all {
                total_children
            } else {
                needed_sel
            };
            if is_parallel_all && failed_count > 0 {
                return Ok(LpStepResult::Failed);
            }
            if finished_count >= needed {
                if !is_parallel_all {
                    deactivate_lp_subtree(ctx, lp_id, nodes, node.id, push_updates, dirty_pc);
                }
                finish_lp_non_child_node(
                    ctx,
                    lp_id,
                    node,
                    push_updates,
                    dirty_pc,
                    deferred_bt_vars.as_deref_mut(),
                );
                return Ok(LpStepResult::Completed);
            }
            if !is_parallel_all
                && node.parent_id != 0
                && total_children > 0
                && (total_children - failed_count) < needed
            {
                return Ok(LpStepResult::Failed);
            }
            Ok(LpStepResult::Running)
        }
        "AlwaysPending" | "AlwaysFalse" => {
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                if step_level_play_node(
                    ctx,
                    lp_id,
                    nodes,
                    child,
                    defer_bt_notifies,
                    push_updates,
                    player_pos_opt,
                    newly_activated,
                    newly_actions,
                    deferred_bt_vars,
                    dirty_pc,
                    steps,
                )? == LpStepResult::TreeEnded
                {
                    return Ok(LpStepResult::TreeEnded);
                }
            }
            Ok(LpStepResult::Running)
        }
        "AlwaysTrue" => {
            for child in nodes {
                if child.parent_id != node.id {
                    continue;
                }
                let res = step_level_play_node(
                    ctx,
                    lp_id,
                    nodes,
                    child,
                    defer_bt_notifies,
                    push_updates,
                    player_pos_opt,
                    newly_activated,
                    newly_actions,
                    deferred_bt_vars,
                    dirty_pc,
                    steps,
                )?;
                if res == LpStepResult::TreeEnded || res == LpStepResult::Running {
                    return Ok(res);
                }
            }
            finish_lp_non_child_node(
                ctx,
                lp_id,
                node,
                push_updates,
                dirty_pc,
                deferred_bt_vars.as_deref_mut(),
            );
            Ok(LpStepResult::Completed)
        }
        "QuestSucceed" => {
            mark_lp_node_finished(ctx, lp_id, node.id, dirty_pc);
            if *dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
                let _ = pc.save(ctx.fs);
                *dirty_pc = false;
            }
            if !defer_bt_notifies {
                complete_level_play(ctx, lp_id)?;
            }
            Ok(LpStepResult::TreeEnded)
        }
        "QuestFailed" => {
            // QuestFailed monitor node checks active failure conditions
            let fail_reason = {
                let mut lp_fail_timers = std::mem::take(&mut ctx.scene.lp_fail_timers);
                let res = {
                    let mut qctx = ctx.as_quest_ctx();
                    quest_engine::eval_quest_failed_condition_pub(
                        &mut qctx,
                        lp_id,
                        node,
                        player_pos_opt,
                        &mut lp_fail_timers,
                    )
                };
                ctx.scene.lp_fail_timers = lp_fail_timers;
                res?
            };
            if let Some(reason) = fail_reason {
                if let Some(fa) = node.data.get("FinishActions") {
                    run_level_play_actions(
                        ctx,
                        lp_id,
                        node.id,
                        Some(fa),
                        deferred_bt_vars.as_deref_mut(),
                    );
                }
                on_bt_rollback(ctx, lp_id, node.id, reason)?;
                return Ok(LpStepResult::TreeEnded);
            }
            Ok(LpStepResult::Running)
        }
        "Action" | "ActionWithResult" => {
            let act_items: Vec<Value> = match &node.data {
                Value::Object(obj) => match obj.get("Actions") {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => match obj.get("Action") {
                        Some(single @ Value::Object(_)) => vec![single.clone()],
                        _ => Vec::new(),
                    },
                },
                _ => Vec::new(),
            };
            if push_updates && !act_items.is_empty() {
                ctx.sink.push(
                    UpdateNodeStatusNotify::MSG_ID,
                    &UpdateNodeStatusNotify {
                        TreeOwnerId: lp_id,
                        TreeIncId: i64::from(lp_id),
                        NodeId: node.id,
                        Status: Some(NodeStatus::Activate),
                    },
                );
                let mut qctx = ctx.as_quest_ctx();
                let _ = quest_engine::push_bt_level_events_for_type(
                    &mut qctx,
                    BtType::BtTypeLevelPlay,
                    lp_id,
                    node.id,
                    GameCtxType::ActionGroupNodeAction,
                    &act_items,
                );
            }
            if !act_items.is_empty() {
                if defer_bt_notifies {
                    newly_actions.push(node.clone());
                }
                let is_async_flow = quest_engine::has_async_play_flow_action(&act_items);
                let items_val = Value::Array(act_items);
                run_level_play_actions(
                    ctx,
                    lp_id,
                    node.id,
                    Some(&items_val),
                    deferred_bt_vars.as_deref_mut(),
                );
                if !is_async_flow
                    && ((ctx.scene.active_flow_inc_id != 0
                        && ctx.scene.active_flow_quest_id == lp_id
                        && ctx.scene.active_flow_node_id == node.id)
                        || ctx.scene.has_pending_flow_for_node(lp_id, node.id))
                {
                    set_lp_node_active(ctx, lp_id, node.id, true, dirty_pc);
                    return Ok(LpStepResult::Running);
                }
            }
            finish_lp_non_child_node(
                ctx,
                lp_id,
                node,
                push_updates,
                dirty_pc,
                deferred_bt_vars.as_deref_mut(),
            );
            Ok(LpStepResult::Completed)
        }
        "Condition" => {
            let wait_true = node
                .data
                .get("WaitUntilTrue")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let ok = if let Some(cb) = node.data.get("Condition") {
                let qctx = ctx.as_quest_ctx();
                quest_engine::eval_inline_condition(&qctx, lp_id, cb)
            } else {
                true
            };
            if ok {
                finish_lp_non_child_node(
                    ctx,
                    lp_id,
                    node,
                    push_updates,
                    dirty_pc,
                    deferred_bt_vars.as_deref_mut(),
                );
                return Ok(LpStepResult::Completed);
            }
            Ok(if wait_true {
                LpStepResult::Running
            } else {
                LpStepResult::Failed
            })
        }
        "ConditionSelector" => {
            if let Some(Value::Array(slots)) = node.data.get("Slots") {
                for slot in slots {
                    let Value::Object(sobj) = slot else {
                        continue;
                    };
                    let pass = if let Some(sc) = sobj.get("Condition") {
                        let qctx = ctx.as_quest_ctx();
                        quest_engine::eval_inline_condition(&qctx, lp_id, sc)
                    } else {
                        true
                    };
                    if !pass {
                        continue;
                    }
                    let Some(Value::Object(sn)) = sobj.get("Node") else {
                        continue;
                    };
                    let Some(cid) = json_int(sn.get("Id")) else {
                        continue;
                    };
                    let Some(cnode) = find_lp_node_by_id(nodes, cid) else {
                        continue;
                    };
                    let res = step_level_play_node(
                        ctx,
                        lp_id,
                        nodes,
                        cnode,
                        defer_bt_notifies,
                        push_updates,
                        player_pos_opt,
                        newly_activated,
                        newly_actions,
                        deferred_bt_vars,
                        dirty_pc,
                        steps,
                    )?;
                    match res {
                        LpStepResult::Completed => {
                            finish_lp_non_child_node(
                                ctx,
                                lp_id,
                                node,
                                push_updates,
                                dirty_pc,
                                deferred_bt_vars.as_deref_mut(),
                            );
                            return Ok(LpStepResult::Completed);
                        }
                        LpStepResult::Running | LpStepResult::TreeEnded => return Ok(res),
                        LpStepResult::Failed => continue,
                    }
                }
            }
            Ok(LpStepResult::Running)
        }
        "Repeater" => {
            let cd_key = repeater_cooldown_key(lp_id, node.id);
            while *steps < MAX_STEPS_PER_ADVANCE {
                if let Some(ec) = node.data.get("ExitCondition") {
                    let qctx = ctx.as_quest_ctx();
                    if quest_engine::eval_inline_condition(&qctx, lp_id, ec) {
                        deactivate_lp_subtree(ctx, lp_id, nodes, node.id, push_updates, dirty_pc);
                        finish_lp_non_child_node(
                            ctx,
                            lp_id,
                            node,
                            push_updates,
                            dirty_pc,
                            deferred_bt_vars.as_deref_mut(),
                        );
                        return Ok(LpStepResult::Completed);
                    }
                }
                if let Some(&ready_ms) = ctx.scene.lp_repeater_cooldowns.get(&cd_key)
                    && now_millis() < ready_ms
                {
                    return Ok(LpStepResult::Running);
                }
                let mut any_child = false;
                let mut child_completed = false;
                for child in nodes {
                    if child.parent_id != node.id {
                        continue;
                    }
                    any_child = true;
                    let res = step_level_play_node(
                        ctx,
                        lp_id,
                        nodes,
                        child,
                        defer_bt_notifies,
                        push_updates,
                        player_pos_opt,
                        newly_activated,
                        newly_actions,
                        deferred_bt_vars,
                        dirty_pc,
                        steps,
                    )?;
                    if res == LpStepResult::TreeEnded {
                        return Ok(LpStepResult::TreeEnded);
                    }
                    if res == LpStepResult::Completed {
                        child_completed = true;
                        break;
                    }
                    return Ok(LpStepResult::Running);
                }
                if !any_child {
                    finish_lp_non_child_node(
                        ctx,
                        lp_id,
                        node,
                        push_updates,
                        dirty_pc,
                        deferred_bt_vars.as_deref_mut(),
                    );
                    return Ok(LpStepResult::Completed);
                }
                if !child_completed {
                    return Ok(LpStepResult::Running);
                }
                reset_lp_subtree_finished_nodes(ctx, lp_id, nodes, node.id, dirty_pc);
                let wait_ms = subtree_wait_delay_ms(nodes, node.id);
                if wait_ms > 0 {
                    ctx.scene
                        .lp_repeater_cooldowns
                        .insert(cd_key, now_millis() + wait_ms);
                    return Ok(LpStepResult::Running);
                }
            }
            Ok(LpStepResult::Running)
        }
        "ChildQuest" => {
            let was_active = is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id);
            if !was_active {
                if let Some(ea) = node.data.get("EnterActions") {
                    run_level_play_actions(
                        ctx,
                        lp_id,
                        node.id,
                        Some(ea),
                        deferred_bt_vars.as_deref_mut(),
                    );
                }
                if let Some(cond) = quest_engine::get_child_condition_pub(&node.data)
                    && let Some(delay_ms) = quest_engine::parse_timer_delay_ms_pub(cond)
                    && delay_ms > 0
                    && !ctx.scene.lp_timers.contains(lp_id, node.id)
                {
                    ctx.scene.lp_timers.upsert(QuestTimerEntry {
                        quest_id: lp_id,
                        node_id: node.id,
                        due_ms: now_millis() + delay_ms,
                    });
                }
            }

            if is_lp_child_quest_satisfied_now(ctx, lp_id, node, player_pos_opt) {
                if !was_active && !defer_bt_notifies && push_updates {
                    let mut qctx = ctx.as_quest_ctx();
                    let _ = quest_engine::push_bt_child_node_status(
                        &mut qctx,
                        BtType::BtTypeLevelPlay,
                        lp_id,
                        node,
                        NodeStatus::Activate,
                        ChildQuestNodeStatus::Progress,
                    );
                }
                complete_lp_child_quest(
                    ctx,
                    lp_id,
                    node,
                    push_updates,
                    dirty_pc,
                    deferred_bt_vars.as_deref_mut(),
                )?;
                return Ok(LpStepResult::Completed);
            }

            if !was_active {
                set_lp_node_active(ctx, lp_id, node.id, true, dirty_pc);
                if defer_bt_notifies {
                    newly_activated.push(node.clone());
                } else if push_updates {
                    let mut qctx = ctx.as_quest_ctx();
                    let _ = quest_engine::push_bt_child_node_status(
                        &mut qctx,
                        BtType::BtTypeLevelPlay,
                        lp_id,
                        node,
                        NodeStatus::Activate,
                        ChildQuestNodeStatus::Progress,
                    );
                    let _ = quest_engine::push_do_interact_options_for_bt_node(
                        qctx.scene.as_deref_mut().expect("scene present"),
                        qctx.sink,
                        BtType::BtTypeLevelPlay,
                        lp_id,
                        node,
                        None,
                    );
                }
            }
            Ok(LpStepResult::Running)
        }
        _ => {
            finish_lp_non_child_node(
                ctx,
                lp_id,
                node,
                push_updates,
                dirty_pc,
                deferred_bt_vars.as_deref_mut(),
            );
            Ok(LpStepResult::Completed)
        }
    }
}

fn step_level_play_tree<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    nodes: &[NodeEntry],
    push_updates: bool,
    player_pos_opt: Option<[f32; 3]>,
) -> io::Result<()> {
    if nodes.is_empty() || ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(());
    }
    if push_updates && !ctx.scene.sent_lp_bt_trees.contains(&lp_id) {
        ctx.sink.push(
            LevelPlayStateNotify::MSG_ID,
            &LevelPlayStateNotify {
                Id: lp_id,
                State: 2,
                CompleteNumber: 0,
            },
        );
        ctx.sink.push(
            EnterLevelPlayNotify::MSG_ID,
            &EnterLevelPlayNotify {
                Id: lp_id,
                State: 2,
                CanGetReward: true,
            },
        );
        let _ = push_level_play_behavior_tree_info(ctx, lp_id, nodes);
    }

    if LP_STEP_DEPTH.with(|d| d.get()) > 0 {
        LP_RECHECK_PENDING.with(|p| p.set(true));
        return Ok(());
    }

    let _guard = StepDepthGuard::enter();
    LP_RECHECK_PENDING.with(|p| p.set(false));

    let Some(root) = find_lp_root_node(nodes).or_else(|| nodes.first()) else {
        return Ok(());
    };
    let mut pass = 0usize;
    while pass < 4 {
        pass += 1;
        LP_RECHECK_PENDING.with(|p| p.set(false));
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            break;
        }
        let mut dirty_pc = false;
        let mut newly_activated = Vec::new();
        let mut newly_actions = Vec::new();
        let mut steps = 0usize;
        let res = step_level_play_node(
            ctx,
            lp_id,
            nodes,
            root,
            false,
            push_updates,
            player_pos_opt,
            &mut newly_activated,
            &mut newly_actions,
            &mut None,
            &mut dirty_pc,
            &mut steps,
        )?;
        if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }
        if res == LpStepResult::Completed
            && !ctx.quest_comp.info.completed_level_plays.contains(&lp_id)
        {
            complete_level_play(ctx, lp_id)?;
            break;
        }
        if !LP_RECHECK_PENDING.with(|p| p.get()) || res == LpStepResult::TreeEnded {
            break;
        }
    }
    Ok(())
}

fn enter_and_start_level_play<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp: &LevelPlayData,
    player_pos_opt: Option<[f32; 3]>,
    dirty_entered: &mut bool,
) -> io::Result<()> {
    let lp_id = lp.LevelPlayId;
    if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(());
    }
    if ctx.quest_comp.info.progress.entered_level_plays.contains(&lp_id) {
        return Ok(());
    }

    if append_unique_int(&mut ctx.quest_comp.info.progress.entered_level_plays, lp_id) {
        *dirty_entered = true;
    }
    ctx.sink.push(
        LevelPlayStateNotify::MSG_ID,
        &LevelPlayStateNotify {
            State: 2,
            CompleteNumber: 0,
            Id: lp_id,
        },
    );
    ctx.sink.push(
        EnterLevelPlayNotify::MSG_ID,
        &EnterLevelPlayNotify {
            CanGetReward: true,
            State: 2,
            Id: lp_id,
        },
    );

    let nodes = ctx
        .assets
        .level_play_nodes
        .get_owner_nodes(lp_id)
        .unwrap_or_default();
    if nodes.is_empty() {
        return Ok(());
    }

    let root = find_lp_root_node(&nodes).unwrap_or(&nodes[0]);
    let mut dirty_pc = false;

    if !is_lp_bt_booted(ctx.progress_comp.as_deref(), lp_id) {
        mark_lp_bt_booted(ctx, lp_id, &mut dirty_pc);
        let mut newly_activated = Vec::new();
        let mut newly_actions = Vec::new();
        let mut deferred_vars = Vec::new();
        let mut steps = 0usize;

        let res = {
            let _guard = StepDepthGuard::enter();
            let mut deferred_opt = Some(&mut deferred_vars);
            step_level_play_node(
                ctx,
                lp_id,
                &nodes,
                root,
                true,
                false,
                player_pos_opt,
                &mut newly_activated,
                &mut newly_actions,
                &mut deferred_opt,
                &mut dirty_pc,
                &mut steps,
            )?
        };
        if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }

        let _ = push_level_play_behavior_tree_info(ctx, lp_id, &nodes);

        for dv in deferred_vars {
            ctx.sink.push(
                BtVarUpdateNotify::MSG_ID,
                &BtVarUpdateNotify {
                    TreeIncId: i64::from(dv.owner_id),
                    VarName: dv.var_name.into_bytes(),
                    VarDefine: Some(dv.var_define),
                    TreeOwnerId: dv.owner_id,
                },
            );
        }

        for cnode in &newly_activated {
            let mut qctx = ctx.as_quest_ctx();
            let enter_acts = quest_engine::node_action_items_pub(Some(cnode), "EnterActions");
            let _ = quest_engine::push_bt_level_events_for_type(
                &mut qctx,
                BtType::BtTypeLevelPlay,
                lp_id,
                cnode.id,
                GameCtxType::ChildQuestNodeEnterAction,
                enter_acts,
            );
            let _ = quest_engine::push_do_interact_options_for_bt_node(
                qctx.scene.as_deref_mut().expect("scene present"),
                qctx.sink,
                BtType::BtTypeLevelPlay,
                lp_id,
                cnode,
                None,
            );
        }

        for act_node in &newly_actions {
            let acts = match &act_node.data {
                Value::Object(obj) => match obj.get("Actions") {
                    Some(Value::Array(arr)) => arr.as_slice(),
                    _ => match obj.get("Action") {
                        Some(single @ Value::Object(_)) => std::slice::from_ref(single),
                        _ => &[],
                    },
                },
                _ => &[],
            };
            if !acts.is_empty() {
                let mut qctx = ctx.as_quest_ctx();
                let _ = quest_engine::push_bt_level_events_for_type(
                    &mut qctx,
                    BtType::BtTypeLevelPlay,
                    lp_id,
                    act_node.id,
                    GameCtxType::ActionGroupNodeAction,
                    acts,
                );
            }
        }

        if res == LpStepResult::TreeEnded {
            complete_level_play(ctx, lp_id)?;
        } else if (res == LpStepResult::Completed || LP_RECHECK_PENDING.with(|p| p.get()))
            && !ctx.quest_comp.info.completed_level_plays.contains(&lp_id)
        {
            LP_RECHECK_PENDING.with(|p| p.set(false));
            step_level_play_tree(ctx, lp_id, &nodes, true, player_pos_opt)?;
        }
    } else {
        let _ = push_level_play_behavior_tree_info(ctx, lp_id, &nodes);
        for node in nodes.iter() {
            if is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                let mut qctx = ctx.as_quest_ctx();
                let _ = quest_engine::push_do_interact_options_for_bt_node(
                    qctx.scene.as_deref_mut().expect("scene present"),
                    qctx.sink,
                    BtType::BtTypeLevelPlay,
                    lp_id,
                    node,
                    None,
                );
            }
        }
        step_level_play_tree(ctx, lp_id, &nodes, true, player_pos_opt)?;
    }
    Ok(())
}

fn run_level_play_actions<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    owner_id: i32,
    node_id: i32,
    actions_opt: Option<&Value>,
    deferred_bt_vars: Option<&mut Vec<DeferredBtVar>>,
) {
    let Some(Value::Array(items)) = actions_opt else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let mut cbs = quest_engine::action_callbacks::<S>();
    cbs.bt_type = BtType::BtTypeLevelPlay;
    cbs.deferred_bt_vars = deferred_bt_vars;
    let mut qctx = ctx.as_quest_ctx();
    let _ = quest_actions::execute_action_list(&mut qctx, &mut cbs, owner_id, node_id, items);
}

pub fn complete_level_play<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    level_play_id: i32,
) -> io::Result<()> {
    if level_play_id <= 0 {
        return Ok(());
    }
    ctx.scene.lp_timers.cancel_quest(level_play_id);
    ctx.scene.fail_timers.cancel_quest(level_play_id);
    ctx.scene.lp_fail_timers.cancel_quest(level_play_id);
    ctx.scene
        .release_occupations_for_tree(level_play_id, i64::from(level_play_id));
    ctx.scene.clear_save_nodes_for_owner(level_play_id);
    if ctx.quest_comp.info.completed_level_plays.contains(&level_play_id) {
        return Ok(());
    }

    let booted = is_lp_bt_booted(ctx.progress_comp.as_deref(), level_play_id);
    if let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(level_play_id) {
        for node in nodes.iter() {
            if is_lp_node_active(ctx.progress_comp.as_deref(), level_play_id, node.id) {
                let mut dirty = false;
                set_lp_node_active(ctx, level_play_id, node.id, false, &mut dirty);
                if dirty && let Some(pc) = ctx.progress_comp.as_deref_mut() {
                    let _ = pc.save(ctx.fs);
                }
            }
            {
                let mut qctx = ctx.as_quest_ctx();
                quest_engine::remove_do_interact_options_for_bt_node(
                    &mut qctx,
                    BtType::BtTypeLevelPlay,
                    level_play_id,
                    node.id,
                );
            }
            if node.node_type != "QuestSucceed" {
                continue;
            }
            if booted {
                ctx.sink.push(
                    UpdateNodeStatusNotify::MSG_ID,
                    &UpdateNodeStatusNotify {
                        TreeOwnerId: level_play_id,
                        TreeIncId: i64::from(level_play_id),
                        NodeId: node.id,
                        Status: Some(NodeStatus::CompletedSuccess),
                    },
                );
            }
            if let Some(acts) = node.data.get("FinishActions").and_then(|v| v.as_array())
                && !acts.is_empty()
            {
                if booted {
                    let mut qctx = ctx.as_quest_ctx();
                    let _ = quest_engine::push_bt_level_events_for_type(
                        &mut qctx,
                        BtType::BtTypeLevelPlay,
                        level_play_id,
                        node.id,
                        GameCtxType::SuccessNodeAction,
                        acts,
                    );
                }
                let fa_val = Value::Array(acts.clone());
                run_level_play_actions(ctx, level_play_id, node.id, Some(&fa_val), None);
            }
        }
    }

    let was_entered = remove_int_value(
        &mut ctx.quest_comp.info.progress.entered_level_plays,
        level_play_id,
    );
    ctx.quest_comp.info.mark_level_play_completed(level_play_id);
    ctx.quest_comp.save(ctx.fs)?;

    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.info.set_key_int(&format!("lp_state:{level_play_id}"), 3);
        pc.save(ctx.fs)?;
    }

    if was_entered {
        ctx.sink.push(
            LevelPlayFirstNotify::MSG_ID,
            &LevelPlayFirstNotify { Id: level_play_id },
        );
    }

    ctx.sink.push(
        LevelPlayStateNotify::MSG_ID,
        &LevelPlayStateNotify {
            State: 3,
            CompleteNumber: 1,
            Id: level_play_id,
        },
    );

    if let Some(lp) = ctx
        .assets
        .tables
        .level_play_data
        .get_data_by_id(&level_play_id)
        && let Some(rc) = &lp.Data.LevelPlayRewardConfig
        && rc.RewardEntityId > 0
    {
        ensure_entity_spawned(ctx, rc.RewardEntityId);
    }

    if was_entered {
        ctx.scene.sent_lp_bt_trees.remove(&level_play_id);
        ctx.sink.push(
            LeaveLevelPlayNotify::MSG_ID,
            &LeaveLevelPlayNotify { Id: level_play_id },
        );
    }

    ctx.sink.push(
        LevelPlayInfoNotify::MSG_ID,
        &LevelPlayInfoNotify {
            LevelPlayInfo: vec![LevelPlayInfo {
                Id: level_play_id,
                IsFirst: true,
                State: 3,
                UpdateTime: 0,
                GetRewardCount: 0,
            }],
        },
    );

    let inst_id = ctx.scene.instance_id;
    let mut qctx = ctx.as_quest_ctx();
    quest_engine::events::on_level_play_completed(&mut qctx, level_play_id)?;
    if inst_id != 8 {
        quest_engine::events::on_dungeon_event(&mut qctx, inst_id, true)?;
    }
    Ok(())
}

fn remove_entity_from_scene_and_notify<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    net_id: i64,
) {
    let _ = ctx.scene.remove(ctx.fs, net_id);
    ctx.sink.push(
        EntityRemoveNotify::MSG_ID,
        &EntityRemoveNotify {
            RemoveInfos: vec![EntityRemoveInfo {
                EntityId: net_id,
                Type: None,
            }],
            IsRemove: true,
        },
    );
}

fn check_level_play_kill_progress<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    killed_cfg_id: i32,
    killed_net_id: i64,
) -> io::Result<()> {
    if killed_cfg_id <= 0 {
        return Ok(());
    }
    let ref_indices: Vec<u32> = ctx
        .assets
        .indexes
        .level_play_indices_for_ref_entity(killed_cfg_id)
        .to_vec();
    if ref_indices.is_empty() {
        return Ok(());
    }

    let mut dirty_pc = false;
    let mut completed_ids = Vec::new();
    let mut step_ids = Vec::new();

    for lp_idx in ref_indices {
        let lp = &ctx.assets.tables.level_play_data.items[lp_idx as usize];
        let lp_id = lp.LevelPlayId;
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        if !matches_scene_instance(ctx.assets, lp, ctx.scene.instance_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };

        let mut all_targets_cleared = true;
        let mut has_matching_kill_node = false;
        let mut matched_kill_nodes = Vec::new();

        for node in nodes.iter() {
            if node.node_type != "ChildQuest" {
                continue;
            }
            let Some(Value::Object(cond)) = node.data.get("Condition") else {
                continue;
            };
            let Some(ctype) = json_str(cond.get("Type")) else {
                continue;
            };
            if ctype != "Kill" && ctype != "MonsterCreator" {
                continue;
            }

            let in_exist = json_array_contains_int(cond.get("ExistTargets"), killed_cfg_id);
            let in_awake = json_array_contains_int(cond.get("TargetsToAwake"), killed_cfg_id);
            let in_mc = json_array_contains_int(cond.get("MonsterCreatorEntityIds"), killed_cfg_id);
            if !in_exist && !in_awake && !in_mc {
                continue;
            }
            has_matching_kill_node = true;

            if let Some(pc) = ctx.progress_comp.as_deref_mut()
                && pc
                    .info
                    .mark_claim(&lp_key("lp_kill", lp_id, killed_cfg_id))
            {
                dirty_pc = true;
            }

            let mut targets_done = true;
            for field in ["ExistTargets", "TargetsToAwake", "MonsterCreatorEntityIds"] {
                if let Some(Value::Array(arr)) = cond.get(field) {
                    for v in arr {
                        let Some(tid) = json_int(Some(v)) else {
                            continue;
                        };
                        let killed_rec = ctx
                            .progress_comp
                            .as_deref()
                            .is_some_and(|pc| pc.info.has_claim(&lp_key("lp_kill", lp_id, tid)));
                        if !killed_rec || is_target_still_alive_in_scene(ctx.scene, tid, killed_net_id)
                        {
                            targets_done = false;
                            break;
                        }
                    }
                }
                if !targets_done {
                    break;
                }
            }
            if targets_done {
                matched_kill_nodes.push(node.clone());
            } else {
                all_targets_cleared = false;
            }
        }

        if has_matching_kill_node {
            for mnode in &matched_kill_nodes {
                complete_lp_child_quest(ctx, lp_id, mnode, true, &mut dirty_pc, None)?;
            }
            if all_targets_cleared && !matched_kill_nodes.is_empty() {
                step_ids.push(lp_id);
            } else if !ctx
                .quest_comp
                .info
                .progress
                .entered_level_plays
                .contains(&lp_id)
            {
                let mut all_ref_dead = true;
                for r in &lp.Data.Reference {
                    if let Ok(rcfg) = r.trim().parse::<i32>()
                        && is_target_still_alive_in_scene(ctx.scene, rcfg, killed_net_id)
                    {
                        all_ref_dead = false;
                        break;
                    }
                }
                if all_ref_dead {
                    completed_ids.push(lp_id);
                }
            }
        }
    }

    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }

    for lp_id in step_ids {
        if let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) {
            step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
        }
    }
    for lp_id in completed_ids {
        complete_level_play(ctx, lp_id)?;
    }
    Ok(())
}

fn record_world_respawn_cooldown<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    cfg_id: i32,
    duration_ms: i64,
) -> io::Result<()> {
    if cfg_id <= 0 || duration_ms <= 0 || ctx.scene.instance_id != 8 {
        return Ok(());
    }
    let now_ms = now_millis();
    ctx.scene
        .instance
        .set_respawn_cooldown(cfg_id, now_ms, now_ms + duration_ms);
    ctx.scene.save_instance(ctx.fs)
}

fn resolve_valid_vision_capture_id(assets: &Assets, resolved: &ResolvedEntityConfig) -> i32 {
    if resolved
        .components
        .VisionCaptureComponent
        .as_ref()
        .is_some_and(|vc| vc.Disabled.unwrap_or(false))
    {
        return 0;
    }
    let has_calabash_entry =
        |id: i32| id > 0 && assets.tables.calabash_develop_reward.get_data_by_id(&id).is_some();

    if let Some(id) = resolved
        .components
        .VisionCaptureComponent
        .as_ref()
        .and_then(|vc| vc.VisionCaptureId)
        .filter(|&id| has_calabash_entry(id))
    {
        return id;
    }

    if let Some(tpl) = assets.find_template_config(&resolved.blueprint_type)
        && let Some(id) = tpl
            .ComponentsData
            .VisionCaptureComponent
            .as_ref()
            .filter(|vc| !vc.Disabled.unwrap_or(false))
            .and_then(|vc| vc.VisionCaptureId)
            .filter(|&id| has_calabash_entry(id))
    {
        return id;
    }

    0
}

pub fn spawn_vision_capture_phantom<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    killed_net_id: i64,
    killed_cfg_id: i32,
    blueprint_type: &str,
    vision_id: i32,
) {
    if vision_id <= 0 {
        return;
    }
    let Some(tpl) = ctx.assets.find_template_config(blueprint_type) else {
        return;
    };
    if tpl.Id <= 0 {
        return;
    }
    let tpl_id = tpl.Id;

    let mut pos = ctx
        .scene
        .instance
        .players
        .first()
        .map_or([0.0, 0.0, 0.0], |p| p.location);
    let mut rot = [0.0f32; 3];
    if let Some(s) = ctx.scene.entity(killed_net_id)
        && let Some(p) = s.position
    {
        pos = p.location;
        rot = p.rotation;
    } else if let Some(lcfg) = ctx
        .assets
        .find_level_entity(ctx.scene.instance_id, i64::from(killed_cfg_id))
    {
        if let Some(wpos) = entity_world_pos(&lcfg) {
            pos = wpos;
        }
        rot = entity_world_rot(&lcfg);
    }

    let normal_state_tag_id = gameplay_tags::id_from_name(
        &ctx.assets.tables.gameplay_tag_remap,
        "关卡.Common.状态.常态",
    )
    .unwrap_or(-1152559349);

    let mut storage = EntityComponentStorage::new(
        0,
        ConfigComponent {
            camp: 2,
            config_id: 14000056,
            config_type: ConfigType::Template,
            entity_type: EntityType::SceneItem,
            state: EntityState::Default,
        },
    );
    storage.actor_visible = true;
    storage.visible = true;
    storage.state_tag = Some(StateTagComponent {
        state_tag_id: normal_state_tag_id,
    });
    storage.tag = Some(TagComponent::default());
    storage.position = Some(PositionComponent {
        location: pos,
        rotation: rot,
    });
    storage.world_item.monster_capture = Some(MonsterCaptureComponent {
        template_id: tpl_id,
        entity_id: killed_cfg_id,
        monster_id: vision_id,
    });

    let entity = ctx.scene.spawn(storage);
    let _ = ctx.scene.save_entity(ctx.fs, entity);

    if let Some(s) = ctx.scene.entity(entity.net_id) {
        let proto_pb = s.to_proto(ctx.assets, now_millis());
        ctx.sink.push(
            EntityAddNotify::MSG_ID,
            &EntityAddNotify {
                EntityPbs: vec![proto_pb],
                RemoveTagIds: false,
            },
        );
    }
}

pub fn on_entity_killed<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    target_net_id: i64,
    cfg_id_opt: Option<i32>,
) -> io::Result<()> {
    if !ctx.scene.rewarded_kills.insert(target_net_id) {
        return Ok(());
    }

    let mut cfg_id = cfg_id_opt.unwrap_or(0);
    if cfg_id <= 0
        && let Some(s) = ctx.scene.entity(target_net_id)
    {
        cfg_id = s.config.config_id;
    }

    if cfg_id > 0 {
        if let Some(resolved) =
            resolve_entity_config(ctx.assets, ctx.scene.instance_id, i64::from(cfg_id))
        {
            let reward_id = resolved
                .components
                .RewardComponent
                .as_ref()
                .and_then(|rc| rc.RewardId)
                .unwrap_or(0);
            let vision_id = resolve_valid_vision_capture_id(ctx.assets, &resolved);
            let is_monster = resolved.blueprint_type.contains("Monster");
            let is_boss = resolved.blueprint_type.contains("Boss")
                || resolved.blueprint_type.contains("Elite");

            if is_monster || reward_id > 0 || vision_id > 0 {
                let calabash_lvl = condition::current_calabash_level(
                    ctx.assets,
                    ctx.basic_comp,
                    ctx.echo_comp,
                );
                let _ = reward::grant_monster_drop_with_echo_option(
                    &mut ctx.as_reward_ctx(),
                    reward_id,
                    vision_id,
                    is_boss,
                    calabash_lvl,
                    false,
                )?;
                if vision_id > 0 {
                    spawn_vision_capture_phantom(
                        ctx,
                        target_net_id,
                        cfg_id,
                        &resolved.blueprint_type,
                        vision_id,
                    );
                }
            }
            record_world_respawn_cooldown(
                ctx,
                cfg_id,
                if is_boss {
                    BOSS_MONSTER_RESPAWN_MS
                } else {
                    NORMAL_MONSTER_RESPAWN_MS
                },
            )?;
            check_level_play_kill_progress(ctx, cfg_id, target_net_id)?;
            if (is_monster || reward_id > 0 || vision_id > 0)
                && is_boss
                && ctx.scene.instance_id != 8
            {
                let inst_id = ctx.scene.instance_id;
                let mut qctx = ctx.as_quest_ctx();
                quest_engine::on_dungeon_event(&mut qctx, inst_id, true)?;
            }
        } else {
            check_level_play_kill_progress(ctx, cfg_id, target_net_id)?;
        }
    }
    Ok(())
}

pub fn on_get_reward_treasure_box<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    net_id: i64,
) -> io::Result<()> {
    let Some(s) = ctx.scene.entity(net_id) else {
        return Ok(());
    };
    let cfg_id = s.config.config_id;
    if cfg_id > 0 && ctx.quest_comp.info.has_opened_chest(cfg_id) {
        return Ok(());
    }

    let mut chest_type_id = 1;
    let mut reward_id = 0;

    if let Some(resolved) =
        resolve_entity_config(ctx.assets, ctx.scene.instance_id, i64::from(cfg_id))
    {
        if let Some(tb) = &resolved.components.TreasureBoxComponent
            && let Some(tid) = tb.TypeId
        {
            chest_type_id = tid;
        }
        if let Some(rc) = &resolved.components.RewardComponent
            && let Some(rid) = rc.RewardId
        {
            reward_id = rid;
        }
    }

    let mut completed_lp_id = None;
    if cfg_id > 0 {
        for &lp_idx in ctx
            .assets
            .indexes
            .level_play_indices_for_reward_entity(cfg_id)
        {
            let lp = &ctx.assets.tables.level_play_data.items[lp_idx as usize];
            if !matches_scene_instance(ctx.assets, lp, ctx.scene.instance_id) {
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

    let _ = reward::grant_treasure_box_reward(
        &mut ctx.as_reward_ctx(),
        chest_type_id,
        reward_id,
        7000,
    )?;

    if cfg_id > 0 {
        ctx.quest_comp.info.mark_chest_opened(cfg_id);
        ctx.quest_comp
            .info
            .set_entity_state(i64::from(cfg_id), "关卡.Common.状态.激活");
        ctx.quest_comp.save(ctx.fs)?;
    }

    if let Some(upd) = ctx.scene.update_entity_state_tag(
        &ctx.assets.tables.gameplay_tag_remap,
        net_id,
        "关卡.Common.状态.激活",
    ) {
        quest_actions::emit_entity_state_tag_update_checked(
            ctx.scene,
            Some(ctx.assets),
            ctx.sink,
            net_id,
            upd,
        );
    }

    if let Some(lp_id) = completed_lp_id {
        complete_level_play(ctx, lp_id)?;
    }
    Ok(())
}

pub fn on_entity_interact_fallback<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    net_id: i64,
) -> io::Result<bool> {
    let Some(s) = ctx.scene.entity(net_id) else {
        return Ok(false);
    };
    let cfg_id = s.config.config_id;
    if cfg_id <= 0 {
        return Ok(false);
    }

    let Some(resolved) =
        resolve_entity_config(ctx.assets, ctx.scene.instance_id, i64::from(cfg_id))
    else {
        return Ok(false);
    };

    if resolved.components.TreasureBoxComponent.is_some()
        || resolved.blueprint_type.contains("TreasureBox")
    {
        on_get_reward_treasure_box(ctx, net_id)?;
        return Ok(true);
    }

    let is_collectible = resolved.components.RewardComponent.is_some()
        && (resolved.blueprint_type.contains("Collect")
            || resolved.blueprint_type.contains("Pick")
            || resolved.components.CollectComponent.is_some());

    if is_collectible {
        let now_ms = now_millis();
        if !ctx.quest_comp.info.has_opened_chest(cfg_id)
            && !ctx.scene.instance.is_on_respawn_cooldown(cfg_id, now_ms)
        {
            let reward_id = resolved
                .components
                .RewardComponent
                .as_ref()
                .and_then(|rc| rc.RewardId)
                .unwrap_or(0);
            if reward_id > 0 {
                let res = reward::grant_drop_package(&mut ctx.as_reward_ctx(), reward_id, 1)?;
                if res.granted_count == 0 {
                    let items = [RewardItem {
                        item_id: 2,
                        count: 200,
                    }];
                    let _ = reward::grant_items(&mut ctx.as_reward_ctx(), 0, false, &items, 1)?;
                }
            }
            record_world_respawn_cooldown(ctx, cfg_id, COLLECTIBLE_RESPAWN_MS)?;
        }
        remove_entity_from_scene_and_notify(ctx, net_id);
        return Ok(true);
    }

    let ref_indices: Vec<u32> = ctx
        .assets
        .indexes
        .level_play_indices_for_ref_entity(cfg_id)
        .to_vec();
    for lp_idx in ref_indices {
        let lp = &ctx.assets.tables.level_play_data.items[lp_idx as usize];
        if !can_enter_level_play(ctx, lp) {
            continue;
        }
        let lp_id = lp.LevelPlayId;
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        let has_tree_active = nodes
            .iter()
            .any(|n| is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, n.id));
        let mut matched_node: Option<NodeEntry> = None;
        for node in nodes.iter() {
            if node.node_type != "ChildQuest" {
                continue;
            }
            if has_tree_active && !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                continue;
            }
            let Some(Value::Object(cond)) = node.data.get("Condition") else {
                continue;
            };
            if json_str(cond.get("Type")) != Some("DoInteract") {
                continue;
            }
            if json_int(cond.get("EntityId")).unwrap_or(0) == cfg_id {
                matched_node = Some(node.clone());
                break;
            }
            if let Some(Value::Array(opts)) = cond.get("AddOptions") {
                for opt_v in opts {
                    if let Value::Object(obj) = opt_v
                        && json_int(obj.get("EntityId")).unwrap_or(0) == cfg_id
                    {
                        matched_node = Some(node.clone());
                        break;
                    }
                }
                if matched_node.is_some() {
                    break;
                }
            }
        }
        if let Some(mnode) = matched_node {
            if is_lp_bt_booted(ctx.progress_comp.as_deref(), lp_id) || has_tree_active {
                let mut dirty_pc = false;
                complete_lp_child_quest(ctx, lp_id, &mnode, true, &mut dirty_pc, None)?;
                if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
                    pc.save(ctx.fs)?;
                }
                step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
            } else {
                run_level_play_actions(ctx, lp_id, mnode.id, mnode.data.get("FinishActions"), None);
                complete_level_play(ctx, lp_id)?;
            }
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn on_animal_drop<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    net_id: i64,
) -> io::Result<()> {
    let Some(s) = ctx.scene.entity(net_id) else {
        return Ok(());
    };
    let cfg_id = s.config.config_id;
    let reward_id = resolve_entity_config(ctx.assets, ctx.scene.instance_id, i64::from(cfg_id))
        .and_then(|r| r.components.RewardComponent)
        .and_then(|rc| rc.RewardId)
        .unwrap_or(0);

    let res = if reward_id > 0 {
        reward::grant_drop_package(&mut ctx.as_reward_ctx(), reward_id, 1)?
    } else {
        Default::default()
    };
    if res.granted_count == 0 {
        let items = [
            RewardItem {
                item_id: 42300010,
                count: 1,
            },
            RewardItem {
                item_id: 2,
                count: 100,
            },
        ];
        let _ = reward::grant_items(&mut ctx.as_reward_ctx(), 0, false, &items, 1)?;
    }
    record_world_respawn_cooldown(ctx, cfg_id, ANIMAL_RESPAWN_MS)?;
    remove_entity_from_scene_and_notify(ctx, net_id);
    Ok(())
}

pub fn on_animal_destroy<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    net_id: i64,
) -> io::Result<()> {
    let Some(s) = ctx.scene.entity(net_id) else {
        return Ok(());
    };
    let cfg_id = s.config.config_id;
    record_world_respawn_cooldown(ctx, cfg_id, ANIMAL_RESPAWN_MS)?;
    remove_entity_from_scene_and_notify(ctx, net_id);
    Ok(())
}

pub fn ensure_active_timers_scheduled<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
) -> io::Result<()> {
    let now_ms = now_millis();
    for &lp_id in &ctx.quest_comp.info.progress.entered_level_plays {
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        for node in nodes.iter() {
            if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                continue;
            }
            if ctx.scene.lp_timers.contains(lp_id, node.id) {
                continue;
            }
            if let Some(cond) = quest_engine::get_child_condition_pub(&node.data)
                && let Some(delay_ms) = quest_engine::parse_timer_delay_ms_pub(cond)
                && delay_ms > 0
            {
                ctx.scene.lp_timers.upsert(QuestTimerEntry {
                    quest_id: lp_id,
                    node_id: node.id,
                    due_ms: now_ms + delay_ms,
                });
            }
        }
    }
    Ok(())
}

pub fn on_timer_due<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
) -> io::Result<bool> {
    if ctx.quest_comp.info.completed_level_plays.contains(&lp_id)
        || !ctx
            .quest_comp
            .info
            .progress
            .entered_level_plays
            .contains(&lp_id)
    {
        return Ok(false);
    }
    if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node_id) {
        return Ok(false);
    }
    let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
        return Ok(false);
    };
    let Some(node) = find_lp_node_by_id(&nodes, node_id).cloned() else {
        return Ok(false);
    };
    let mut dirty_pc = false;
    complete_lp_child_quest(ctx, lp_id, &node, true, &mut dirty_pc, None)?;
    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    let player_pos = current_player_pos(ctx);
    step_level_play_tree(ctx, lp_id, &nodes, true, player_pos)?;
    Ok(true)
}

pub fn on_fail_timer_due<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
) -> io::Result<bool> {
    if lp_id <= 0
        || node_id <= 0
        || ctx.quest_comp.info.completed_level_plays.contains(&lp_id)
        || !ctx
            .quest_comp
            .info
            .progress
            .entered_level_plays
            .contains(&lp_id)
    {
        return Ok(false);
    }
    let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
        return Ok(false);
    };
    let node = find_lp_node_by_id(&nodes, node_id);
    if node.is_some_and(|n| {
        !is_lp_failed_monitor_active(ctx.progress_comp.as_deref(), lp_id, &nodes, n)
    }) {
        return Ok(false);
    }
    if let Some(node) = node
        && let Some(fa) = node.data.get("FinishActions")
    {
        run_level_play_actions(ctx, lp_id, node.id, Some(fa), None);
    }
    on_level_play_enabled(ctx, lp_id, false)?;
    Ok(true)
}

pub fn collect_active_target_entity_ids(
    assets: &Assets,
    quest_comp: &PlayerQuestComponent,
    progress_comp: Option<&PlayerProgressComponent>,
    out: &mut HashSet<i32>,
) {
    for &lp_id in &quest_comp.info.progress.entered_level_plays {
        if quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        for node in nodes.iter() {
            if !is_lp_node_active(progress_comp, lp_id, node.id) {
                continue;
            }
            for eid in quest_engine::extract_node_target_entity_ids(node) {
                out.insert(eid);
            }
        }
    }
}

fn parse_ref_entity_id(raw: &str) -> Option<i32> {
    let trimmed = raw.trim();
    if let Ok(eid) = trimmed.parse::<i32>() {
        return (eid > 0).then_some(eid);
    }
    let (_, tail) = trimmed.rsplit_once('_')?;
    let eid = tail.parse::<i32>().ok()?;
    (eid > 0).then_some(eid)
}

fn dungeon_level_play_matches_instance(
    assets: &Assets,
    lp: &LevelPlayData,
    inst_id: i32,
    map_id: i32,
) -> bool {
    if lp.Data.Type != "Dungeon" || (lp.Data.Id <= 0 && lp.LevelPlayId <= 0) {
        return false;
    }
    if lp.Data.InstanceId == inst_id {
        return true;
    }
    if lp.Data.InstanceId != 0 || lp.Data.LevelId != map_id {
        return false;
    }
    lp.Data.Reference.iter().any(|r| {
        parse_ref_entity_id(r)
            .and_then(|eid| assets.find_level_entity(map_id, i64::from(eid)))
            .and_then(|cfg| cfg.ComponentsData.BaseInfoComponent.as_ref().and_then(|bi| bi.PackId))
            == Some(inst_id)
    })
}

pub fn sync_dungeon_level_plays<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
) -> io::Result<()> {
    let inst_id = ctx.scene.instance_id;
    if inst_id <= 0 || inst_id == 8 {
        return Ok(());
    }
    let map_id = ctx.assets.resolve_map_config_id(inst_id);
    let candidates: Vec<LevelPlayData> = ctx
        .assets
        .tables
        .level_play_data
        .items
        .iter()
        .filter(|lp| {
            let lp_id = lp.LevelPlayId;
            lp_id > 0
                && lp.Data.LevelPlayEntityId == 0
                && !ctx.quest_comp.info.completed_level_plays.contains(&lp_id)
                && !ctx
                    .quest_comp
                    .info
                    .progress
                    .entered_level_plays
                    .contains(&lp_id)
                && dungeon_level_play_matches_instance(ctx.assets, lp, inst_id, map_id)
        })
        .cloned()
        .collect();
    let player_pos = current_player_pos(ctx);
    let mut dirty_entered = false;
    for lp in candidates {
        enter_and_start_level_play(ctx, &lp, player_pos, &mut dirty_entered)?;
    }
    if dirty_entered {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn sync_on_login<S: PushSink + ?Sized>(ctx: &mut LevelPlayContext<'_, S>) -> io::Result<()> {
    ensure_active_timers_scheduled(ctx)?;
    let pos = current_player_pos(ctx);
    let entered_snap = ctx.quest_comp.info.progress.entered_level_plays.clone();
    for lp_id in entered_snap {
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Some(lp) = ctx.assets.tables.level_play_data.get_data_by_id(&lp_id) else {
            continue;
        };
        if !matches_scene_instance(ctx.assets, &lp, ctx.scene.instance_id) {
            continue;
        }
        ctx.sink.push(
            LevelPlayStateNotify::MSG_ID,
            &LevelPlayStateNotify {
                State: 2,
                CompleteNumber: 0,
                Id: lp_id,
            },
        );
        ctx.sink.push(
            EnterLevelPlayNotify::MSG_ID,
            &EnterLevelPlayNotify {
                CanGetReward: true,
                State: 2,
                Id: lp_id,
            },
        );
        if let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id)
            && !nodes.is_empty()
        {
            let _ = push_level_play_behavior_tree_info(ctx, lp_id, &nodes);
            for node in nodes.iter() {
                if is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                    let _ = quest_engine::push_do_interact_options_for_bt_node(
                        ctx.scene,
                        ctx.sink,
                        BtType::BtTypeLevelPlay,
                        lp_id,
                        node,
                        None,
                    );
                }
            }
            step_level_play_tree(ctx, lp_id, &nodes, true, pos)?;
        }
    }
    sync_dungeon_level_plays(ctx)?;
    Ok(())
}

pub fn on_player_move<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    x: f32,
    y: f32,
    z: f32,
) -> io::Result<()> {
    let player_pos = [x, y, z];
    let mut dirty_entered = false;

    let mut remaining_entered = Vec::new();
    let entered_snap = ctx.quest_comp.info.progress.entered_level_plays.clone();
    for lp_id in entered_snap {
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            dirty_entered = true;
            continue;
        }
        let Some(lp) = ctx.assets.tables.level_play_data.get_data_by_id(&lp_id) else {
            remaining_entered.push(lp_id);
            continue;
        };
        if !matches_scene_instance(ctx.assets, &lp, ctx.scene.instance_id) {
            remaining_entered.push(lp_id);
            continue;
        }
        let enter_r = if lp.Data.EnterRadius > 0 {
            lp.Data.EnterRadius as f32
        } else {
            3000.0
        };
        let leave_r = if lp.Data.LeaveRadius > 0 {
            lp.Data.LeaveRadius as f32
        } else {
            enter_r + 1500.0
        };
        if lp.Data.Type != "Challenge"
            && lp.Data.Type != "Dungeon"
            && let Some(anchor) = resolve_level_play_anchor_pos(ctx, &lp)
        {
            let dx = x - anchor[0];
            let dy = y - anchor[1];
            let dz = z - anchor[2];
            if (dx * dx + dy * dy + dz * dz) > (leave_r * leave_r) {
                ctx.scene.sent_lp_bt_trees.remove(&lp_id);
                ctx.scene.lp_timers.cancel_quest(lp_id);
                ctx.scene.lp_fail_timers.cancel_quest(lp_id);
                ctx.scene
                    .release_occupations_for_tree(lp_id, i64::from(lp_id));
                ctx.scene.remove_bt_timers_for_owner(lp_id);
                ctx.sink
                    .push(LeaveLevelPlayNotify::MSG_ID, &LeaveLevelPlayNotify { Id: lp_id });
                dirty_entered = true;
                continue;
            }
        }
        remaining_entered.push(lp_id);
    }
    if remaining_entered.len() != ctx.quest_comp.info.progress.entered_level_plays.len() {
        ctx.quest_comp.info.progress.entered_level_plays = remaining_entered;
        dirty_entered = true;
    }

    // Collect nearby LevelPlay anchors (spawned scene entities) + enabled + quest-referenced LevelPlays.
    let mut candidate_lps = Vec::new();
    for s in &ctx.scene.entities {
        if s.config.config_id <= 0 {
            continue;
        }
        for &lp_idx in ctx
            .assets
            .indexes
            .level_play_indices_for_entity(s.config.config_id)
        {
            let lp_id = ctx.assets.tables.level_play_data.items[lp_idx as usize].LevelPlayId;
            if lp_id > 0 {
                let _ = append_unique_int(&mut candidate_lps, lp_id);
            }
        }
    }
    if let Some(pc) = ctx.progress_comp.as_deref() {
        for entry in &pc.info.dynamic.ints {
            let Some(rest) = entry.key.strip_prefix("lp_state:") else {
                continue;
            };
            if entry.val != 2 {
                continue;
            }
            if let Ok(lp_id) = rest.parse::<i32>()
                && lp_id > 0
            {
                let _ = append_unique_int(&mut candidate_lps, lp_id);
            }
        }
    }
    for aq in &ctx.quest_comp.info.active_quests {
        if aq.active_nodes.is_empty() {
            continue;
        }
        let Ok(qnodes) = ctx.assets.quest_nodes.get_owner_nodes(aq.quest_id) else {
            continue;
        };
        for &nid in &aq.active_nodes {
            let Some(qnode) = find_lp_node_by_id(&qnodes, nid) else {
                continue;
            };
            if qnode.node_type != "ChildQuest" {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&qnode.data) else {
                continue;
            };
            if json_str(cond.get("Type")) != Some("CheckLevelPlay") {
                continue;
            }
            append_condition_level_play_ids(&mut candidate_lps, cond);
        }
    }

    for &lp_id in &candidate_lps {
        let Some(lp) = ctx
            .assets
            .tables
            .level_play_data
            .get_data_by_id(&lp_id)
            .map(|c| c.into_owned())
        else {
            continue;
        };
        if !can_enter_level_play(ctx, &lp) {
            continue;
        }
        if ctx
            .quest_comp
            .info
            .progress
            .entered_level_plays
            .contains(&lp_id)
        {
            continue;
        }
        if lp.Data.EnterRadius > 0
            && let Some(anchor) = resolve_level_play_anchor_pos(ctx, &lp)
        {
            let enter_r = lp.Data.EnterRadius as f32;
            let dx = x - anchor[0];
            let dy = y - anchor[1];
            let dz = z - anchor[2];
            if (dx * dx + dy * dy + dz * dz) > (enter_r * enter_r) {
                continue;
            }
        }
        enter_and_start_level_play(ctx, &lp, Some(player_pos), &mut dirty_entered)?;
    }

    // Also evaluate non-spawnable map LevelPlay anchors (`LevelPlayEntityId`) within 3D EnterRadius.
    let non_spawnable_indices: Vec<usize> = ctx
        .assets
        .tables
        .level_play_data
        .items
        .iter()
        .enumerate()
        .filter_map(|(idx, lp)| {
            (lp.Data.EnterRadius > 0
                && lp.Data.LevelPlayEntityId > 0
                && lp.Data.Type != "Quest"
                && lp.Data.Type != "Dungeon"
                && lp.Data.Type != "LordGym"
                && !candidate_lps.contains(&lp.LevelPlayId)
                && matches_scene_instance(ctx.assets, lp, ctx.scene.instance_id))
            .then_some(idx)
        })
        .collect();
    for lp_idx in non_spawnable_indices {
        let lp = ctx.assets.tables.level_play_data.items[lp_idx].clone();
        if !can_enter_level_play(ctx, &lp) {
            continue;
        }
        if ctx
            .quest_comp
            .info
            .progress
            .entered_level_plays
            .contains(&lp.LevelPlayId)
        {
            continue;
        }
        let Some(lcfg) = ctx
            .assets
            .find_level_entity(ctx.scene.instance_id, i64::from(lp.Data.LevelPlayEntityId))
        else {
            continue;
        };
        let Some(anchor) = entity_world_pos(&lcfg) else {
            continue;
        };
        let enter_r = lp.Data.EnterRadius as f32;
        let dx = x - anchor[0];
        let dy = y - anchor[1];
        let dz = z - anchor[2];
        if (dx * dx + dy * dy + dz * dz) <= (enter_r * enter_r) {
            enter_and_start_level_play(ctx, &lp, Some(player_pos), &mut dirty_entered)?;
        }
    }

    let active_lp_snap = ctx.quest_comp.info.progress.entered_level_plays.clone();
    for lp_id in active_lp_snap {
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        let mut reach_done = false;
        let mut dirty_pc = false;
        for node in nodes.iter() {
            if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                continue;
            }
            if node.node_type != "ChildQuest" {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
                continue;
            };
            if json_str(cond.get("Type")) == Some("ReachArea")
                && is_player_within_reach_area(ctx, &node.data, cond, player_pos)
            {
                complete_lp_child_quest(ctx, lp_id, node, true, &mut dirty_pc, None)?;
                reach_done = true;
            }
        }
        if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }
        if reach_done {
            step_level_play_tree(ctx, lp_id, &nodes, true, Some(player_pos))?;
        } else {
            for node in nodes.iter() {
                if node.node_type != "QuestFailed" {
                    continue;
                }
                if !is_lp_failed_monitor_active(ctx.progress_comp.as_deref(), lp_id, &nodes, node) {
                    continue;
                }
                let fail_reason = {
                    let mut lp_fail_timers = std::mem::take(&mut ctx.scene.lp_fail_timers);
                    let res = {
                        let mut qctx = ctx.as_quest_ctx();
                        quest_engine::eval_quest_failed_condition_pub(
                            &mut qctx,
                            lp_id,
                            node,
                            Some(player_pos),
                            &mut lp_fail_timers,
                        )
                    };
                    ctx.scene.lp_fail_timers = lp_fail_timers;
                    res?
                };
                if let Some(reason) = fail_reason {
                    if let Some(fa) = node.data.get("FinishActions") {
                        run_level_play_actions(ctx, lp_id, node.id, Some(fa), None);
                    }
                    on_bt_rollback(ctx, lp_id, node.id, reason)?;
                    break;
                }
            }
        }
    }

    if dirty_entered {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_level_play_enabled<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    level_play_id: i32,
    enabled: bool,
) -> io::Result<()> {
    if level_play_id <= 0 {
        return Ok(());
    }
    let lp_state_key = format!("lp_state:{level_play_id}");
    if !enabled {
        let mut pc_dirty = false;
        if let Some(pc) = ctx.progress_comp.as_deref_mut()
            && pc.info.get_key_int(&lp_state_key) != 0
        {
            pc.info.set_key_int(&lp_state_key, 0);
            pc_dirty = true;
        }
        ctx.scene.lp_timers.cancel_quest(level_play_id);
        ctx.scene.fail_timers.cancel_quest(level_play_id);
        ctx.scene.lp_fail_timers.cancel_quest(level_play_id);
        ctx.scene
            .release_occupations_for_tree(level_play_id, i64::from(level_play_id));
        ctx.scene.clear_save_nodes_for_owner(level_play_id);
        let mut dirty = false;
        if remove_int_value(
            &mut ctx.quest_comp.info.progress.entered_level_plays,
            level_play_id,
        ) {
            dirty = true;
            ctx.scene.sent_lp_bt_trees.remove(&level_play_id);
            ctx.sink.push(
                LevelPlayStateNotify::MSG_ID,
                &LevelPlayStateNotify {
                    State: 0,
                    CompleteNumber: 0,
                    Id: level_play_id,
                },
            );
            ctx.sink.push(
                LeaveLevelPlayNotify::MSG_ID,
                &LeaveLevelPlayNotify { Id: level_play_id },
            );
        }
        if pc_dirty && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }
        if dirty {
            ctx.quest_comp.save(ctx.fs)?;
        }
        return Ok(());
    }

    let mut pc_dirty = false;
    if let Some(pc) = ctx.progress_comp.as_deref_mut()
        && pc.info.get_key_int(&lp_state_key) != 2
    {
        pc.info.set_key_int(&lp_state_key, 2);
        pc_dirty = true;
    }
    let mut dirty = false;
    if let Some(lp) = ctx
        .assets
        .tables
        .level_play_data
        .get_data_by_id(&level_play_id)
        .map(|c| c.into_owned())
        && matches_scene_instance(ctx.assets, &lp, ctx.scene.instance_id)
        && !ctx
            .quest_comp
            .info
            .completed_level_plays
            .contains(&level_play_id)
    {
        enter_and_start_level_play(ctx, &lp, current_player_pos(ctx), &mut dirty)?;
    }
    if pc_dirty && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    if dirty {
        ctx.quest_comp.save(ctx.fs)?;
    }
    Ok(())
}

pub fn on_dynamic_interact<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
    entity_id: i64,
) -> io::Result<bool> {
    if lp_id <= 0 || ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(false);
    }
    let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
        return Ok(false);
    };
    let Some(node) = find_lp_node_by_id(&nodes, node_id).cloned() else {
        return Ok(false);
    };
    let cfg_id = ctx
        .scene
        .entity(entity_id)
        .map(|s| s.config.config_id)
        .unwrap_or_else(|| i32::try_from(entity_id).unwrap_or(0));
    let mut dirty_pc = false;

    if let Some(cond) = quest_engine::get_child_condition_pub(&node.data) {
        let mut all_done = true;
        if cfg_id > 0
            && let Some(pc) = ctx.progress_comp.as_deref_mut()
            && pc
                .info
                .mark_claim(&format!("lp_interact:{lp_id}:{node_id}:{cfg_id}"))
        {
            dirty_pc = true;
        }
        if let Some(Value::Array(opts)) = cond.get("AddOptions")
            && opts.len() > 1
            && let Some(pc) = ctx.progress_comp.as_deref()
        {
            for o in opts {
                if let Value::Object(obj) = o
                    && let Some(eid) = json_int(obj.get("EntityId"))
                    && !pc
                        .info
                        .has_claim(&format!("lp_interact:{lp_id}:{node_id}:{eid}"))
                {
                    all_done = false;
                    break;
                }
            }
        }
        if !all_done {
            if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
                pc.save(ctx.fs)?;
            }
            return Ok(true);
        }
    }

    complete_lp_child_quest(ctx, lp_id, &node, true, &mut dirty_pc, None)?;
    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
    Ok(true)
}

pub fn on_var_changed<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    owner_id: i32,
    var_name: &str,
) -> io::Result<()> {
    let entered = ctx.quest_comp.info.progress.entered_level_plays.clone();
    for lp_id in entered {
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        if owner_id > 0 && owner_id != lp_id {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        let mut dirty_pc = false;
        let mut changed = false;
        for node in nodes.iter() {
            if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
                continue;
            };
            if json_str(cond.get("Type")) != Some("ListenToEvent") {
                continue;
            }
            let Some(Value::Object(et)) = cond.get("EventType") else {
                continue;
            };
            if json_str(et.get("Type")) != Some("VarValueChange") {
                continue;
            }
            let want_name = json_str(et.get("VarName"))
                .or_else(|| json_str(et.get("Name")))
                .or_else(|| {
                    et.get("Var")
                        .and_then(Value::as_object)
                        .and_then(crate::helpers::quest_types::resolve_var_name)
                })
                .unwrap_or("");
            if !var_name.is_empty() && !want_name.is_empty() && want_name != var_name {
                continue;
            }
            complete_lp_child_quest(ctx, lp_id, node, true, &mut dirty_pc, None)?;
            changed = true;
        }
        if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }
        if changed {
            step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
        }
    }
    evaluate_entered_level_play_trees(ctx, owner_id)
}

pub fn on_entity_state_changed<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    entity_config_id: i32,
    state_name: &str,
) -> io::Result<()> {
    let entered = ctx.quest_comp.info.progress.entered_level_plays.clone();
    for lp_id in entered {
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        let mut dirty_pc = false;
        let mut changed = false;
        for node in nodes.iter() {
            if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
                continue;
            };
            if json_str(cond.get("Type")) != Some("ListenToEvent") {
                continue;
            }
            let Some(Value::Object(et)) = cond.get("EventType") else {
                continue;
            };
            if json_str(et.get("Type")) != Some("EntityStateChange") {
                continue;
            }
            let eid = json_int(et.get("EntityId")).unwrap_or(0);
            let want_state = json_str(et.get("State")).unwrap_or("");
            if (eid == 0 || eid == entity_config_id)
                && (want_state.is_empty() || want_state == state_name)
            {
                complete_lp_child_quest(ctx, lp_id, node, true, &mut dirty_pc, None)?;
                changed = true;
            }
        }
        if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }
        if changed {
            step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
        }
    }
    evaluate_entered_level_play_trees(ctx, 0)
}

pub fn on_condition_state_changed<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
) -> io::Result<()> {
    evaluate_entered_level_play_trees(ctx, 0)
}

pub fn step_entered_tree_by_id<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
) -> io::Result<()> {
    if lp_id <= 0 || ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(());
    }
    if !ctx.quest_comp.info.progress.entered_level_plays.contains(&lp_id) {
        return Ok(());
    }
    if let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) {
        step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
    }
    Ok(())
}

pub fn on_bt_timer_end<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
) -> io::Result<bool> {
    if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(false);
    }
    let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
        return Ok(false);
    };
    let mut dirty_pc = false;
    let mut progressed = false;
    for node in nodes.iter() {
        if node.node_type != "ChildQuest" {
            continue;
        }
        if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
            continue;
        }
        let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
            continue;
        };
        let ctype = json_str(cond.get("Type")).unwrap_or("");
        if ctype == "Timer" || ctype == "WaitTime" {
            complete_lp_child_quest(ctx, lp_id, node, true, &mut dirty_pc, None)?;
            progressed = true;
            break;
        }
    }
    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    if progressed {
        step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
    }
    Ok(progressed)
}

pub fn on_level_sequence_mark_reached<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    entity_config_id: i32,
    mark: &str,
) -> io::Result<()> {
    let entered_snap = ctx.quest_comp.info.progress.entered_level_plays.clone();
    for lp_id in entered_snap {
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        let mut dirty_pc = false;
        let mut changed = false;
        for node in nodes.iter() {
            if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id) {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
                continue;
            };
            if json_str(cond.get("Type")) != Some("WaitUntilLevelSequenceReachMark") {
                continue;
            }
            let eid = json_int(cond.get("EntityId")).unwrap_or(0);
            let want_mark = json_str(cond.get("Mark")).unwrap_or("");
            if (eid == 0 || eid == entity_config_id)
                && (want_mark.is_empty() || want_mark == mark)
            {
                complete_lp_child_quest(ctx, lp_id, node, true, &mut dirty_pc, None)?;
                changed = true;
            }
        }
        if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }
        if changed {
            step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
        }
    }
    Ok(())
}

pub fn on_bt_rollback<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    fail_node_id: i32,
    reason: FailReason,
) -> io::Result<()> {
    on_bt_rollback_with_select(ctx, lp_id, fail_node_id, reason, 2)
}

pub fn on_bt_rollback_with_select<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    fail_node_id: i32,
    reason: FailReason,
    select: i32,
) -> io::Result<()> {
    if lp_id <= 0 || ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(());
    }
    ctx.scene.lp_timers.cancel_quest(lp_id);
    ctx.scene.lp_fail_timers.cancel_quest(lp_id);
    ctx.scene
        .release_occupations_for_tree(lp_id, i64::from(lp_id));

    let nodes = ctx
        .assets
        .level_play_nodes
        .get_owner_nodes(lp_id)
        .unwrap_or_default();
    let mut latest_ckpt_id = 0i32;
    for node in nodes.iter() {
        if !is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, node.id) {
            continue;
        }
        if node.data.get("SaveConfig").is_some_and(|v| !v.is_null()) && node.id > latest_ckpt_id {
            latest_ckpt_id = node.id;
        }
    }
    if latest_ckpt_id == 0 && reason != FailReason::DefaultFail {
        on_level_play_enabled(ctx, lp_id, false)?;
        return Ok(());
    }

    let mut dirty_pc = false;
    for node in nodes.iter() {
        let was_active = is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id);
        if was_active {
            let mut qctx = ctx.as_quest_ctx();
            quest_engine::remove_do_interact_options_for_bt_node(
                &mut qctx,
                BtType::BtTypeLevelPlay,
                lp_id,
                node.id,
            );
            set_lp_node_active(ctx, lp_id, node.id, false, &mut dirty_pc);
        }
        if node.id >= latest_ckpt_id {
            clear_lp_node_finished(ctx, lp_id, node.id, &mut dirty_pc);
        }
    }
    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }

    ctx.sink.push(
        BtRollbackStartNotify::MSG_ID,
        &BtRollbackStartNotify {
            TreeOwnerId: lp_id,
            TreeIncId: i64::from(lp_id),
            IsRollbackSubLevel: false,
            IsRollbackPos: select == 1,
        },
    );
    ctx.sink.push(
        BtRollbackNotify::MSG_ID,
        &BtRollbackNotify {
            BtRollbackInfo: None,
            FailReason: Some(reason),
            TreeIncId: i64::from(lp_id),
            TreeOwnerId: lp_id,
            FailNodeId: fail_node_id,
        },
    );

    if !nodes.is_empty() {
        step_level_play_tree(ctx, lp_id, &nodes, true, current_player_pos(ctx))?;
    }
    Ok(())
}

pub fn on_flow_end<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
) -> io::Result<()> {
    if lp_id <= 0 || ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
        return Ok(());
    }
    let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
        return Ok(());
    };
    let Some(node) = find_lp_node_by_id(&nodes, node_id).cloned() else {
        return Ok(());
    };
    let mut dirty_pc = false;
    if node.node_type == "ChildQuest" {
        complete_lp_child_quest(ctx, lp_id, &node, true, &mut dirty_pc, None)?;
    } else if node.node_type == "Action" || node.node_type == "ActionWithResult" {
        finish_lp_non_child_node(ctx, lp_id, &node, true, &mut dirty_pc, None);
    } else {
        return Ok(());
    }
    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }
    step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
    Ok(())
}

fn evaluate_entered_level_play_trees<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    preferred_lp_id: i32,
) -> io::Result<()> {
    if preferred_lp_id > 0
        && ctx.quest_comp.info.progress.entered_level_plays.contains(&preferred_lp_id,)
        && !ctx.quest_comp.info.completed_level_plays.contains(&preferred_lp_id,)
        && let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(preferred_lp_id)
    {
        step_level_play_tree(ctx, preferred_lp_id, &nodes, true, None)?;
    }

    let inst_id = ctx.scene.instance_id;
    let map_id = ctx.assets.resolve_map_config_id(inst_id);
    let entered_snap = ctx.quest_comp.info.progress.entered_level_plays.clone();
    for lp_id in entered_snap {
        if lp_id == preferred_lp_id || ctx.quest_comp.info.completed_level_plays.contains(&lp_id)
        {
            continue;
        }
        if let Some(lp) = ctx.assets.tables.level_play_data.get_data_by_id(&lp_id) {
            if lp.Data.Type == "Dungeon" {
                if !dungeon_level_play_matches_instance(ctx.assets, &lp, inst_id, map_id) {
                    continue;
                }
            } else if (lp.Data.InstanceId > 0 && lp.Data.InstanceId != inst_id)
                || (lp.Data.LevelId > 0 && lp.Data.LevelId != map_id)
            {
                continue;
            }
        }
        if let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) {
            step_level_play_tree(ctx, lp_id, &nodes, true, None)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelPlayEventKind {
    FinishFlipper,
    FinishKurotato,
    FinishRoleTower,
    FinishRollBlock,
    FinishSurvivorsRouge,
    FinishTetris,
    FinishTrapDefense,
    CrossSceneSyncKill,
    FinishKuroSimpleCombatKill,
    CompareActorVar,
    NewHandInItems,
    ParallaxAlign,
    PlaybackShowTimeline,
    TakePicturesWithTimeScale,
    WalkingPattern,
}

impl LevelPlayEventKind {
    pub fn condition_type(self) -> &'static str {
        match self {
            Self::FinishFlipper => "FinishFlipper",
            Self::FinishKurotato => "FinishKurotato",
            Self::FinishRoleTower => "FinishRoleTower",
            Self::FinishRollBlock => "FinishRollBlock",
            Self::FinishSurvivorsRouge => "FinishSurvivorsRouge",
            Self::FinishTetris => "FinishTetris",
            Self::FinishTrapDefense => "FinishTrapDefense",
            Self::CrossSceneSyncKill => "CrossSceneSyncKillMonster",
            Self::FinishKuroSimpleCombatKill => "FinishKuroSimpleCombatKillMonster",
            Self::CompareActorVar => "CompareActorVar",
            Self::NewHandInItems => "NewHandInItems",
            Self::ParallaxAlign => "ParallaxAlign",
            Self::PlaybackShowTimeline => "PlaybackShowTimeline",
            Self::TakePicturesWithTimeScale => "TakePicturesWithTimeScale",
            Self::WalkingPattern => "WalkingPattern",
        }
    }

    #[allow(dead_code)]
    pub fn progress_prefix(self) -> &'static str {
        match self {
            Self::FinishFlipper => "lp_flipper",
            Self::FinishKurotato => "lp_kurotato",
            Self::FinishRoleTower => "lp_roletower",
            Self::FinishRollBlock => "lp_rollblock",
            Self::FinishSurvivorsRouge => "lp_survivors",
            Self::FinishTetris => "lp_tetris",
            Self::FinishTrapDefense => "lp_trapdef",
            Self::CrossSceneSyncKill => "lp_xkill",
            Self::FinishKuroSimpleCombatKill => "lp_ksc_kill",
            Self::CompareActorVar => "lp_actor_var",
            Self::NewHandInItems => "lp_handin",
            Self::ParallaxAlign => "lp_parallax",
            Self::PlaybackShowTimeline => "lp_timeline",
            Self::TakePicturesWithTimeScale => "lp_photo_ts",
            Self::WalkingPattern => "lp_walkpat",
        }
    }
}

pub fn allows_level_play_client_submit(ctype: &str) -> bool {
    matches!(
        ctype,
        "CompareActorVar"
            | "FinishFlipper"
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
            | "TakePhoto"
            | "TakePhoto2"
            | "WalkingPattern"
            | "ReachArea"
            | "CheckUiGame"
            | "CheckEntityState"
            | "CompareVar"
            | "UseSkill"
            | "UseItem"
            | "Deliver"
            | "Communicate"
            | "Parkour"
            | "RhythmGame"
            | "SlidingBlocks"
            | "CrossSceneSyncKillMonster"
            | "FinishKuroSimpleCombatKillMonster"
            | "ShowUi"
            | "AwakeAndLoadEntity"
            | "GetItem"
            | "CheckPlayerInput"
            | "ReadPhoneMessage"
            | "FinishBvbChallenge"
            | "FinishArcadeGameplay"
    )
}

fn json_int_val(v_opt: Option<&Value>) -> i64 {
    match v_opt {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn has_pending_sibling_child_quests(
    progress_comp: Option<&PlayerProgressComponent>,
    lp_id: i32,
    nodes: &[NodeEntry],
    completed_node_id: i32,
) -> bool {
    let mut completed_parent = 0;
    for n in nodes {
        if n.id == completed_node_id {
            completed_parent = n.parent_id;
            break;
        }
    }
    let parent_type = if completed_parent > 0 {
        nodes
            .iter()
            .find(|n| n.id == completed_parent)
            .map(|n| n.node_type.as_str())
    } else {
        None
    };

    if matches!(parent_type, Some("Parallel" | "ParallelSelect")) {
        return false;
    }

    nodes.iter().any(|n| {
        n.node_type == "ChildQuest"
            && n.id != completed_node_id
            && !is_level_play_node_marked(progress_comp, lp_id, n.id)
    })
}

fn finish_level_play_node_and_maybe_complete<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    lp_id: i32,
    node_id: i32,
    cond_type: &str,
) -> io::Result<bool> {
    let nodes = ctx
        .assets
        .level_play_nodes
        .get_owner_nodes(lp_id)
        .unwrap_or_default();

    let mut target_node = None;
    if node_id > 0 {
        target_node = find_lp_node_by_id(&nodes, node_id).cloned();
    }
    if target_node.is_none() && !cond_type.is_empty() {
        for n in nodes.iter() {
            if n.node_type != "ChildQuest" {
                continue;
            }
            if is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, n.id) {
                continue;
            }
            if let Some(cond) = quest_engine::get_child_condition_pub(&n.data)
                && json_str(cond.get("Type")) == Some(cond_type)
            {
                target_node = Some(n.clone());
                break;
            }
        }
    }

    let Some(mnode) = target_node else {
        return Ok(false);
    };
    if is_lp_bt_booted(ctx.progress_comp.as_deref(), lp_id)
        || nodes
            .iter()
            .any(|n| is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, n.id))
    {
        let mut dirty_pc = false;
        complete_lp_child_quest(ctx, lp_id, &mnode, true, &mut dirty_pc, None)?;
        if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
            pc.save(ctx.fs)?;
        }
        let player_pos = current_player_pos(ctx);
        step_level_play_tree(ctx, lp_id, &nodes, true, player_pos)?;
        return Ok(true);
    }

    let mut dirty_pc = false;
    mark_lp_node_finished(ctx, lp_id, mnode.id, &mut dirty_pc);
    if dirty_pc && let Some(pc) = ctx.progress_comp.as_deref_mut() {
        pc.save(ctx.fs)?;
    }

    if let Some(fa) = mnode.data.get("FinishActions") {
        run_level_play_actions(ctx, lp_id, mnode.id, Some(fa), None);
    }

    if !has_pending_sibling_child_quests(ctx.progress_comp.as_deref(), lp_id, &nodes, mnode.id) {
        complete_level_play(ctx, lp_id)?;
    } else {
        ctx.sink.push(
            LevelPlayStateNotify::MSG_ID,
            &LevelPlayStateNotify {
                State: 2,
                CompleteNumber: 0,
                Id: lp_id,
            },
        );
    }
    Ok(true)
}

fn find_active_level_play_id<S: PushSink + ?Sized>(
    ctx: &LevelPlayContext<'_, S>,
    owner_id: i32,
    tree_inc_id: i64,
) -> Option<i32> {
    let candidate = if owner_id > 0 {
        owner_id
    } else if tree_inc_id > 0 {
        i32::try_from(tree_inc_id).ok()?
    } else {
        return None;
    };
    if ctx.quest_comp.info.completed_level_plays.contains(&candidate) {
        return None;
    }
    let lp = ctx
        .assets
        .tables
        .level_play_data
        .get_data_by_id(&candidate)?;
    if !matches_scene_instance(ctx.assets, &lp, ctx.scene.instance_id) {
        return None;
    }
    Some(candidate)
}

pub fn on_submit_level_play_node<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    owner_id: i32,
    tree_inc_id: i64,
    node_id: i32,
) -> io::Result<bool> {
    let Some(lp_id) = find_active_level_play_id(ctx, owner_id, tree_inc_id) else {
        return Ok(false);
    };
    let nodes = ctx
        .assets
        .level_play_nodes
        .get_owner_nodes(lp_id)
        .unwrap_or_default();
    let Some(node) = find_lp_node_by_id(&nodes, node_id) else {
        return Ok(false);
    };
    if node.node_type != "ChildQuest" {
        return Ok(false);
    }
    let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
        return Ok(false);
    };
    let ctype = json_str(cond.get("Type")).unwrap_or("");
    if !allows_level_play_client_submit(ctype) {
        return Ok(false);
    }
    if ctype == "CompareActorVar"
        && let Some(pc) = ctx.progress_comp.as_deref()
        && let Some(vname) = json_str(cond.get("VarName"))
    {
        let expected = json_int_val(cond.get("Value"));
        let cmp_op = json_str(cond.get("Compare")).unwrap_or("Eq");
        let cur = pc.info.get_key_int(&format!("actor_var:{lp_id}:{vname}"));
        let ok = match cmp_op {
            "Gt" | ">" => cur > expected,
            "Ge" | ">=" => cur >= expected,
            "Lt" | "<" => cur < expected,
            "Le" | "<=" => cur <= expected,
            "Ne" | "!=" => cur != expected,
            _ => cur == expected,
        };
        if !ok {
            return Ok(true);
        }
    }

    finish_level_play_node_and_maybe_complete(ctx, lp_id, node_id, ctype)
}

pub fn on_submit_photo_level_play_node<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    owner_id: i32,
    tree_inc_id: i64,
    node_id: i32,
) -> io::Result<bool> {
    if let Some(pc) = ctx.progress_comp.as_deref_mut() {
        let _ = pc.info.add_key_int("explore:photo_cnt", 1);
    }
    if (owner_id > 0 || tree_inc_id > 0)
        && node_id > 0
        && on_submit_level_play_node(ctx, owner_id, tree_inc_id, node_id)?
    {
        return Ok(true);
    }
    on_condition_event(ctx, LevelPlayEventKind::TakePicturesWithTimeScale, 0, 0)?;
    Ok(true)
}

fn condition_matches_target_entity(
    kind: LevelPlayEventKind,
    cond: &Map<String, Value>,
    entity_cfg_id: i32,
    extra_id: i32,
) -> bool {
    let entity_cfg_i64 = i64::from(entity_cfg_id);
    match kind {
        LevelPlayEventKind::NewHandInItems => {
            let Some(Value::Object(hi)) = cond.get("HandIn") else {
                return entity_cfg_id == 0;
            };
            let eid = json_int_val(hi.get("EntityId"));
            let iid = json_int_val(hi.get("ItemId")) as i32;
            if entity_cfg_id > 0 && eid > 0 && eid != entity_cfg_i64 {
                return false;
            }
            if extra_id > 0 && iid > 0 && iid != extra_id {
                return false;
            }
            true
        }
        LevelPlayEventKind::WalkingPattern => {
            if entity_cfg_id <= 0 {
                return true;
            }
            let spline_id = json_int_val(cond.get("SplineEntityId"));
            let end_id = json_int_val(cond.get("EndEntityId"));
            spline_id == entity_cfg_i64
                || end_id == entity_cfg_i64
                || (spline_id == 0 && end_id == 0)
        }
        LevelPlayEventKind::FinishRollBlock => {
            if entity_cfg_id <= 0 {
                return true;
            }
            let center_id = json_int_val(cond.get("CenterEntityId"));
            let interact_id = json_int_val(cond.get("InteractEntityId"));
            let grp_id = json_int_val(cond.get("RollBlockGroupId")) as i32;
            center_id == entity_cfg_i64
                || interact_id == entity_cfg_i64
                || (extra_id > 0 && grp_id == extra_id)
        }
        LevelPlayEventKind::FinishTetris => {
            if entity_cfg_id <= 0 {
                return true;
            }
            let origin_id = json_int_val(cond.get("OriginPosEntityId"));
            origin_id == 0 || origin_id == entity_cfg_i64
        }
        _ => true,
    }
}

pub fn on_condition_event<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    kind: LevelPlayEventKind,
    target_id: i32,
    value: i32,
) -> io::Result<()> {
    let cond_type = kind.condition_type();
    if kind == LevelPlayEventKind::CompareActorVar {
        let entered = ctx.quest_comp.info.progress.entered_level_plays.clone();
        if let Some(pc) = ctx.progress_comp.as_deref_mut() {
            for id in &entered {
                pc.info
                    .set_key_int(&format!("actor_var:{id}:{target_id}"), i64::from(value));
            }
            pc.save(ctx.fs)?;
        }
        return Ok(());
    }

    if let Some(stat_key) = match kind {
        LevelPlayEventKind::ParallaxAlign => Some("parallax:done"),
        LevelPlayEventKind::PlaybackShowTimeline => Some("timeline:done"),
        LevelPlayEventKind::FinishFlipper => Some("flipper:fin"),
        LevelPlayEventKind::FinishRollBlock => Some("rb_blk:fin"),
        LevelPlayEventKind::WalkingPattern => Some("walk_pat:done"),
        _ => None,
    } && let Some(pc) = ctx.progress_comp.as_deref_mut()
    {
        let _ = pc.info.add_key_int(stat_key, 1);
        let _ = pc.save(ctx.fs);
    }

    let mut candidate_ids = Vec::new();
    for &lp_id in &ctx.quest_comp.info.progress.entered_level_plays {
        let _ = append_unique_int(&mut candidate_ids, lp_id);
    }
    if target_id > 0 {
        for &lp_idx in ctx.assets.indexes.level_play_indices_for_ref_entity(target_id) {
            if let Some(lp) = ctx.assets.tables.level_play_data.items.get(lp_idx as usize) {
                let _ = append_unique_int(&mut candidate_ids, lp.LevelPlayId);
            }
        }
        for &lp_idx in ctx.assets.indexes.level_play_indices_for_entity(target_id) {
            if let Some(lp) = ctx.assets.tables.level_play_data.items.get(lp_idx as usize) {
                let _ = append_unique_int(&mut candidate_ids, lp.LevelPlayId);
            }
        }
    }

    for lp_id in candidate_ids {
        let Some(lp) = ctx.assets.tables.level_play_data.get_data_by_id(&lp_id) else {
            continue;
        };
        if !matches_scene_instance(ctx.assets, &lp, ctx.scene.instance_id) {
            continue;
        }
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        let mut matched_node_id = 0;
        for n in nodes.iter() {
            if n.node_type != "ChildQuest" {
                continue;
            }
            if is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, n.id) {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&n.data) else {
                continue;
            };
            let Some(actual_ctype) = json_str(cond.get("Type")) else {
                continue;
            };
            let matches_type = actual_ctype == cond_type
                || (kind == LevelPlayEventKind::TakePicturesWithTimeScale
                    && (actual_ctype == "TakePhoto" || actual_ctype == "TakePhoto2"));
            if !matches_type {
                continue;
            }
            if !condition_matches_target_entity(kind, cond, target_id, value) {
                continue;
            }
            matched_node_id = n.id;
            break;
        }
        if matched_node_id > 0 {
            let _ =
                finish_level_play_node_and_maybe_complete(ctx, lp_id, matched_node_id, cond_type)?;
        }
    }
    Ok(())
}

pub fn on_entity_killed_with_conditions<S: PushSink + ?Sized>(
    ctx: &mut LevelPlayContext<'_, S>,
    killed_net_id: i64,
    cfg_id_opt: Option<i32>,
) -> io::Result<()> {
    let resolved_cfg_id = cfg_id_opt.unwrap_or_else(|| {
        ctx.scene
            .entity(killed_net_id)
            .map_or(0, |s| s.config.config_id)
    });

    on_entity_killed(ctx, killed_net_id, cfg_id_opt)?;

    if resolved_cfg_id <= 0 {
        return Ok(());
    }

    let mut candidate_ids = Vec::new();
    for &lp_id in &ctx.quest_comp.info.progress.entered_level_plays {
        let _ = append_unique_int(&mut candidate_ids, lp_id);
    }
    for &lp_idx in ctx
        .assets
        .indexes
        .level_play_indices_for_ref_entity(resolved_cfg_id)
    {
        if let Some(lp) = ctx.assets.tables.level_play_data.items.get(lp_idx as usize) {
            let _ = append_unique_int(&mut candidate_ids, lp.LevelPlayId);
        }
    }

    let mut finish_targets: Vec<(i32, i32, &'static str)> = Vec::new();

    for lp_id in candidate_ids {
        let Some(lp) = ctx.assets.tables.level_play_data.get_data_by_id(&lp_id) else {
            continue;
        };
        if !matches_scene_instance(ctx.assets, &lp, ctx.scene.instance_id) {
            continue;
        }
        if ctx.quest_comp.info.completed_level_plays.contains(&lp_id) {
            continue;
        }
        let Ok(nodes) = ctx.assets.level_play_nodes.get_owner_nodes(lp_id) else {
            continue;
        };
        for node in nodes.iter() {
            if node.node_type != "ChildQuest" {
                continue;
            }
            if is_level_play_node_marked(ctx.progress_comp.as_deref(), lp_id, node.id) {
                continue;
            }
            if !is_lp_node_active(ctx.progress_comp.as_deref(), lp_id, node.id)
                && !ctx
                    .quest_comp
                    .info
                    .progress
                    .entered_level_plays
                    .contains(&lp_id)
            {
                continue;
            }
            let Some(cond) = quest_engine::get_child_condition_pub(&node.data) else {
                continue;
            };
            let Some(ctype) = json_str(cond.get("Type")) else {
                continue;
            };
            let cond_kind: &'static str = if ctype == "CrossSceneSyncKillMonster" {
                "CrossSceneSyncKillMonster"
            } else if ctype == "FinishKuroSimpleCombatKillMonster" {
                "FinishKuroSimpleCombatKillMonster"
            } else {
                continue;
            };
            let mut has_filter = false;
            let mut matches_filter = false;
            if let Some(eid) = json_int(cond.get("EntityId"))
                && eid > 0
            {
                has_filter = true;
                if eid == resolved_cfg_id {
                    matches_filter = true;
                }
            }
            for field in [
                "EntityIds",
                "ExistTargets",
                "TargetsToAwake",
                "MonsterCreatorEntityIds",
            ] {
                if let Some(Value::Array(arr)) = cond.get(field)
                    && !arr.is_empty()
                {
                    has_filter = true;
                    if json_array_contains_int(cond.get(field), resolved_cfg_id) {
                        matches_filter = true;
                    }
                }
            }
            if has_filter && !matches_filter {
                continue;
            }

            finish_targets.push((lp_id, node.id, cond_kind));
            break;
        }
    }

    for (lp_id, node_id, cond_kind) in finish_targets {
        let _ = finish_level_play_node_and_maybe_complete(ctx, lp_id, node_id, cond_kind)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonoro_gamesv_data::tables::instance_dungeon::InstanceDungeon;
    use sonoro_gamesv_data::tables::level_play_data::{DataEntry, LevelPlayData};

    fn make_test_instance_dungeon(id: i32, map_config_id: i32) -> InstanceDungeon {
        InstanceDungeon {
            Id: id,
            MapConfigId: map_config_id,
            ..Default::default()
        }
    }

    #[test]
    fn matches_scene_instance_respects_instance_id_override_and_rejects_cross_map_level_id() {
        let mut assets = Assets::default();
        assets.tables.instance_dungeon = sonoro_gamesv_data::data_tables::Table::from_items(
            vec![
                make_test_instance_dungeon(8, 8),
                make_test_instance_dungeon(906, 906),
            ],
            |d| d.Id,
        );

        let overworld_lp = LevelPlayData {
            LevelPlayId: 1001,
            Data: DataEntry {
                Id: 1001,
                LevelId: 8,
                Type: "Quest".to_string(),
                ..Default::default()
            },
        };
        assert!(matches_scene_instance(&assets, &overworld_lp, 8));
        assert!(!matches_scene_instance(&assets, &overworld_lp, 906));

        let lahai_roi_lp = LevelPlayData {
            LevelPlayId: 2002,
            Data: DataEntry {
                Id: 2002,
                LevelId: 8,
                Type: "Quest".to_string(),
                InstanceId: 906,
                ..Default::default()
            },
        };
        assert!(matches_scene_instance(&assets, &lahai_roi_lp, 906));
        assert!(!matches_scene_instance(&assets, &lahai_roi_lp, 8));
    }

    #[test]
    fn all_15_level_play_node_data_condition_types_map_cleanly_and_allow_valid_submit_or_event_triggers() {
        let all_15 = [
            ("FinishFlipper", LevelPlayEventKind::FinishFlipper),
            ("FinishKurotato", LevelPlayEventKind::FinishKurotato),
            ("FinishRoleTower", LevelPlayEventKind::FinishRoleTower),
            ("FinishRollBlock", LevelPlayEventKind::FinishRollBlock),
            ("FinishSurvivorsRouge", LevelPlayEventKind::FinishSurvivorsRouge),
            ("FinishTetris", LevelPlayEventKind::FinishTetris),
            ("FinishTrapDefense", LevelPlayEventKind::FinishTrapDefense),
            ("CrossSceneSyncKillMonster", LevelPlayEventKind::CrossSceneSyncKill),
            ("FinishKuroSimpleCombatKillMonster", LevelPlayEventKind::FinishKuroSimpleCombatKill),
            ("CompareActorVar", LevelPlayEventKind::CompareActorVar),
            ("NewHandInItems", LevelPlayEventKind::NewHandInItems),
            ("ParallaxAlign", LevelPlayEventKind::ParallaxAlign),
            ("PlaybackShowTimeline", LevelPlayEventKind::PlaybackShowTimeline),
            ("TakePicturesWithTimeScale", LevelPlayEventKind::TakePicturesWithTimeScale),
            ("WalkingPattern", LevelPlayEventKind::WalkingPattern),
        ];
        assert_eq!(all_15.len(), 15);
        for (ctype, kind) in all_15 {
            assert_eq!(kind.condition_type(), ctype);
            assert!(allows_level_play_client_submit(ctype));
            assert!(!kind.progress_prefix().is_empty());
        }
        assert!(!allows_level_play_client_submit("DoInteract"));
        assert!(!allows_level_play_client_submit("PlayFlow"));
        assert!(allows_level_play_client_submit("FinishArcadeGameplay"));
    }
}
