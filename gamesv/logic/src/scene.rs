use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::data_tables::{Table, create_buff_information};
use sonoro_gamesv_data::tables::gameplay_tag_remap::GameplayTagRemap;
use sonoro_gamesv_fs::explore_tools_info::ExploreToolsInfo;
use sonoro_gamesv_fs::file_util::load_or_create_zon;
use sonoro_gamesv_fs::formation_info::FormationInfo;
use sonoro_gamesv_fs::last_scene_info::LastSceneInfo;
use sonoro_gamesv_fs::scene_instance::SceneInstance;
use sonoro_proto::enums::{BtType, ERemoveEntityType, MotionType, TimerSetType};
use sonoro_proto::pb::{
    AiHateEntity, CombatCommon, CombatNotifyData, CombatNotifyDataMessageOneof,
    CombatReceiveData, CombatReceiveDataMessageOneof, EntityIsVisibleNotify,
    EntityRemoveInfo, FightBuffInformation, PlayerBattleStateChangeNotify,
    RemoveCombineRelationNotify, Rotator, TimerInfoPb, Vector, ActorVisibleNotify,
};

use crate::component::comp_util;
use crate::component::entity::fsm::FsmComponent;
use crate::component::entity::small::{EntityType, StateTagComponent};
use crate::component::entity::{EntityComponentStorage, TagComponent};
use crate::component::player::PlayerSceneComponent;
use crate::events::{BuffAdditionEntry, EntityHandle};
use crate::fsm::types::wake_reason;
use crate::helpers::gameplay_tags;
use crate::scene_time::TimeInfo;
use crate::schedulers::fsm_wake_scheduler::{FsmWakeScheduler, WakeEntry, WakeKind};
use crate::schedulers::quest_timer_queue::QuestTimerQueue;

pub fn now_millis() -> i64 {
    static EPOCH_ANCHOR: OnceLock<(i64, Instant)> = OnceLock::new();
    let &(base_unix_ms, base_instant) = EPOCH_ANCHOR.get_or_init(|| {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        (unix_ms, Instant::now())
    });
    let elapsed_ms = i64::try_from(base_instant.elapsed().as_millis()).unwrap_or(i64::MAX);
    base_unix_ms.saturating_add(elapsed_ms)
}

#[derive(Debug, Clone, Default)]
pub struct DeferredJoinFlow {
    pub inc_id: i32,
    pub namespace: String,
    pub id: i32,
    pub state: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KilledRemovalEntry {
    pub entity_id: i64,
    pub due_ms: i64,
    pub remove_type: ERemoveEntityType,
}

#[derive(Debug, Clone)]
pub struct AsyncFlowEntry {
    pub inc_id: i64,
    pub quest_id: i32,
    pub node_id: i32,
    pub bt_type: BtType,
    pub flow_id: i32,
    pub state_id: i32,
    pub owner_entity_id: i64,
    pub flow_list_name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RangeTriggerState {
    pub in_range: bool,
    pub enter_fired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DynamicInteractKind {
    #[default]
    None,
    Flow,
    Actions,
}

#[derive(Debug, Clone)]
pub struct DynamicInteractOption {
    pub option_guid: String,
    pub entity_net_id: i64,
    pub entity_config_id: i32,
    pub bt_type: BtType,
    pub quest_id: i32,
    pub node_id: i32,
    pub kind: DynamicInteractKind,
    pub flow_list_name: String,
    pub flow_id: i32,
    pub state_id: i32,
    pub actions_json: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct PlayBubbleRecord {
    pub entity_config_id: i32,
    pub action_guid: String,
}

#[derive(Debug, Clone)]
pub struct ActiveBtTimer {
    pub owner_id: i32,
    pub inc_id: i64,
    pub node_id: i32,
    pub timer_type: String,
    pub end_time_ms: i64,
    pub pause_time_ms: i64,
    pub paused: bool,
}

impl ActiveBtTimer {
    pub fn to_proto(&self) -> TimerInfoPb {
        TimerInfoPb {
            TimerType: self.timer_type.as_bytes().to_vec(),
            NodeId: self.node_id,
            EndTime: self.end_time_ms,
            PauseTime: self.pause_time_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateTagUpdate {
    pub old_tag_id: i32,
    pub new_tag_id: i32,
}

#[derive(Debug, Clone)]
pub struct ScenePendingFlow {
    pub quest_id: i32,
    pub node_id: i32,
    pub flow_list_name: String,
    pub flow_id: i32,
    pub state_id: i32,
}

#[derive(Debug, Clone)]
pub struct BtOccupation {
    pub bt_type: BtType,
    pub owner_id: i32,
    pub inc_id: i64,
    pub node_id: i32,
    pub resource_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugSpawnRoute {
    pub source_map_id: i32,
    pub repeat_boss: bool,
    pub owner_count: usize,
}

impl DebugSpawnRoute {
    pub fn matches(self, source_map_id: i32, repeat_boss: bool) -> bool {
        self.source_map_id == source_map_id && self.repeat_boss == repeat_boss
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CombineRelation {
    pub target_entity_id: i64,
    pub part_index: i32,
    pub position: Vector,
    pub rotation: Rotator,
}

impl CombineRelation {
    pub fn matches(&self, other: &Self) -> bool {
        self.target_entity_id == other.target_entity_id
            && self.part_index == other.part_index
            && self.position == other.position
            && self.rotation == other.rotation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CombineDetach {
    pub combine_entity_id: i64,
    pub target_entity_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombineRegisterResult {
    Added,
    Existing,
}

#[derive(Debug)]
pub struct Scene {
    pub player_id: i32,
    pub instance_id: i32,
    pub last_passed_portal_id: i32,
    pub instance: SceneInstance,
    pub formation_info: FormationInfo,
    pub explore_tools_info: ExploreToolsInfo,
    pub entities: Vec<EntityComponentStorage>,
    pub net_id_map: HashMap<i64, usize>,
    pub scene_time: TimeInfo,
    pub active_battle_entities: HashSet<i64>,
    pub rewarded_kills: HashSet<i64>,
    pub fsm_hate_states: HashMap<i64, bool>,
    pub combine_relations: HashMap<i64, CombineRelation>,
    pub pending_combine_detaches: Vec<CombineDetach>,
    pub debug_spawn_routes: HashMap<i64, DebugSpawnRoute>,
    pub debug_spawn_route_owners: HashMap<i64, i64>,
    pub fsm_wakes: FsmWakeScheduler,
    pub player_in_battle: bool,
    pub battle_state_notified: bool,
    pub battle_state_dirty: bool,
    pub last_spatial_check_loc: [f32; 3],
    pub last_quest_move_loc: [f32; 3],
    pub has_quest_move_loc: bool,
    pub player_movement_mode: i32,
    pub has_player_movement_mode: bool,
    pub player_motion_type: MotionType,
    pub has_player_motion: bool,
    pub active_sneak_id: i32,
    pub failed_dungeons: Vec<i32>,
    pub current_quest_node: i32,
    pub current_quest_id: i32,
    pub prologue_flow_started: bool,
    pub active_flow_inc_id: i64,
    pub active_flow_quest_id: i32,
    pub active_flow_node_id: i32,
    pub active_flow_bt_type: BtType,
    pub active_flow_list_name: String,
    pub active_flow_id: i32,
    pub active_flow_state_id: i32,
    pub active_flow_owner_entity_id: i64,
    pub flow_next_state_id: i32,
    pub executed_flow_action_ids: Vec<i32>,
    pub pending_transition_instance_id: Option<i32>,
    pub next_action_inc_id: i32,
    pub client_black_screen_open: bool,
    pub join_scene_pushed: bool,
    pub scene_loading_finished: bool,
    pub deferred_join_sync_pending: bool,
    pub login_world_synced: bool,
    pub deferred_spatial_spawns_at_ms: Option<i64>,
    pub deferred_join_flow: Option<DeferredJoinFlow>,
    pub next_entity_id: i64,
    pub plot_added_rover_slot: Option<usize>,
    pub plot_saved_cur_role: i32,
    pub plot_saved_slot_role_id: i32,
    pub plot_saved_slot_entity_id: i64,
    /// Role entities of the Echo (Phantom) group while ChangePhantomFormation is in effect.
    pub phantom_role_entity_ids: Vec<i64>,
    /// Role of the Echo group the client switched to (SwitchRoleRequest); 0 = the battle role.
    pub phantom_cur_role: i32,
    pub pending_flows: Vec<ScenePendingFlow>,
    pub quest_timers: QuestTimerQueue,
    pub lp_timers: QuestTimerQueue,
    pub fail_timers: QuestTimerQueue,
    pub lp_fail_timers: QuestTimerQueue,
    pub sent_lp_bt_trees: HashSet<i32>,
    pub lp_repeater_cooldowns: HashMap<i64, i64>,
    pub bt_timers: Vec<ActiveBtTimer>,
    pub bt_occupations: Vec<BtOccupation>,
    pub save_node_keys: Vec<u64>,
    pub trial_role_ids: Vec<i32>,
    pub pending_killed_removals: Vec<KilledRemovalEntry>,
    pub dynamic_interact_options: Vec<DynamicInteractOption>,
    pub play_bubbles: Vec<PlayBubbleRecord>,
    pub player_range_states: HashMap<i64, RangeTriggerState>,
    pub fired_condition_listeners: HashSet<u64>,
    pub auto_interact_states: HashMap<i64, bool>,
    pub async_flows: Vec<AsyncFlowEntry>,
    pub action_node_next_idx: HashMap<u64, usize>,
    pub last_server_rollback_quest_id: i32,
    pub last_server_rollback_ms: i64,
    pub summoned_entities: HashMap<String, Vec<i64>>,
    pub prefab_entities: HashMap<String, Vec<i64>>,
}

impl Scene {
    pub fn from_parts(
        player_id: i32,
        instance_id: i32,
        instance: SceneInstance,
        formation_info: FormationInfo,
        explore_tools_info: ExploreToolsInfo,
    ) -> Self {
        Self {
            player_id,
            instance_id,
            last_passed_portal_id: 0,
            instance,
            formation_info,
            explore_tools_info,
            entities: Vec::new(),
            net_id_map: HashMap::new(),
            scene_time: TimeInfo::default(),
            active_battle_entities: HashSet::new(),
            rewarded_kills: HashSet::new(),
            fsm_hate_states: HashMap::new(),
            combine_relations: HashMap::new(),
            pending_combine_detaches: Vec::new(),
            debug_spawn_routes: HashMap::new(),
            debug_spawn_route_owners: HashMap::new(),
            fsm_wakes: FsmWakeScheduler::new(),
            player_in_battle: false,
            battle_state_notified: false,
            battle_state_dirty: false,
            last_spatial_check_loc: [0.0; 3],
            last_quest_move_loc: [0.0; 3],
            has_quest_move_loc: false,
            player_movement_mode: 1,
            has_player_movement_mode: false,
            player_motion_type: MotionType::Spurt,
            has_player_motion: false,
            active_sneak_id: 0,
            failed_dungeons: Vec::new(),
            current_quest_node: 19,
            current_quest_id: 139_000_025,
            prologue_flow_started: false,
            active_flow_inc_id: 0,
            active_flow_quest_id: 0,
            active_flow_node_id: 0,
            active_flow_bt_type: BtType::BtTypeQuest,
            active_flow_list_name: String::new(),
            active_flow_id: 0,
            active_flow_state_id: 0,
            active_flow_owner_entity_id: 0,
            flow_next_state_id: 0,
            executed_flow_action_ids: Vec::new(),
            pending_transition_instance_id: None,
            next_action_inc_id: 1,
            client_black_screen_open: false,
            join_scene_pushed: false,
            scene_loading_finished: false,
            deferred_join_sync_pending: false,
            login_world_synced: false,
            deferred_spatial_spawns_at_ms: None,
            deferred_join_flow: None,
            next_entity_id: 1,
            plot_added_rover_slot: None,
            plot_saved_cur_role: 0,
            plot_saved_slot_role_id: 0,
            plot_saved_slot_entity_id: -1,
            phantom_role_entity_ids: Vec::new(),
            phantom_cur_role: 0,
            pending_flows: Vec::new(),
            quest_timers: QuestTimerQueue::new(),
            lp_timers: QuestTimerQueue::new(),
            fail_timers: QuestTimerQueue::new(),
            lp_fail_timers: QuestTimerQueue::new(),
            sent_lp_bt_trees: HashSet::new(),
            lp_repeater_cooldowns: HashMap::new(),
            bt_timers: Vec::new(),
            bt_occupations: Vec::new(),
            save_node_keys: Vec::new(),
            trial_role_ids: Vec::new(),
            pending_killed_removals: Vec::new(),
            dynamic_interact_options: Vec::new(),
            play_bubbles: Vec::new(),
            player_range_states: HashMap::new(),
            fired_condition_listeners: HashSet::new(),
            auto_interact_states: HashMap::new(),
            async_flows: Vec::new(),
            action_node_next_idx: HashMap::new(),
            last_server_rollback_quest_id: 0,
            last_server_rollback_ms: 0,
            summoned_entities: HashMap::new(),
            prefab_entities: HashMap::new(),
        }
    }

    pub fn init(fs: &FileSystem, player_id: i32, instance_id: i32) -> io::Result<Self> {
        let mut instance = load_or_create_zon::<SceneInstance>(
            fs,
            &format!("player/{player_id}/scene/{instance_id}/instance"),
        )?;
        if instance.owner_id == 0 {
            instance.owner_id = player_id;
        }
        instance.buff_handle = 0;

        let formation_info =
            load_or_create_zon::<FormationInfo>(fs, &format!("player/{player_id}/formation_info"))?;
        let explore_tools_info = load_or_create_zon::<ExploreToolsInfo>(
            fs,
            &format!("player/{player_id}/explore_tools_info"),
        )?;

        let scene = Self::from_parts(
            player_id,
            instance_id,
            instance,
            formation_info,
            explore_tools_info,
        );
        scene.clear_entities(fs)?;
        Ok(scene)
    }

    pub fn clear_entities(&self, fs: &FileSystem) -> io::Result<()> {
        fs.delete_tree(&format!(
            "player/{}/scene/{}/entity",
            self.player_id, self.instance_id
        ))
    }

    pub fn entity_handle(&self, net_id: i64) -> Option<EntityHandle> {
        let &index = self.net_id_map.get(&net_id)?;
        let storage = self.entities.get(index)?;
        if storage.net_id != net_id {
            return None;
        }
        Some(EntityHandle { index, net_id })
    }

    pub fn entity_mut(&mut self, net_id: i64) -> Option<&mut EntityComponentStorage> {
        let &index = self.net_id_map.get(&net_id)?;
        let storage = self.entities.get_mut(index)?;
        (storage.net_id == net_id).then_some(storage)
    }

    pub fn entity(&self, net_id: i64) -> Option<&EntityComponentStorage> {
        let &index = self.net_id_map.get(&net_id)?;
        let storage = self.entities.get(index)?;
        (storage.net_id == net_id).then_some(storage)
    }

    pub fn set_active_flow(
        &mut self,
        inc_id: i64,
        quest_id: i32,
        node_id: i32,
        flow_list_name: &str,
        flow_id: i32,
        state_id: i32,
    ) {
        let name = flow_list_name.to_string();
        self.clear_active_flow();
        self.active_flow_inc_id = inc_id;
        self.active_flow_quest_id = quest_id;
        self.active_flow_node_id = node_id;
        self.active_flow_list_name = name;
        self.active_flow_id = flow_id;
        self.active_flow_state_id = state_id;
    }

    pub fn clear_active_flow(&mut self) {
        self.active_flow_list_name.clear();
        self.active_flow_inc_id = 0;
        self.active_flow_quest_id = 0;
        self.active_flow_node_id = 0;
        self.active_flow_bt_type = BtType::BtTypeQuest;
        self.active_flow_id = 0;
        self.active_flow_state_id = 0;
        self.active_flow_owner_entity_id = 0;
        self.flow_next_state_id = 0;
        self.executed_flow_action_ids.clear();
    }

    pub fn mark_flow_action_executed(&mut self, action_id: i32) {
        if action_id > 0 && !self.has_flow_action_executed(action_id) {
            self.executed_flow_action_ids.push(action_id);
        }
    }

    pub fn has_flow_action_executed(&self, action_id: i32) -> bool {
        action_id > 0 && self.executed_flow_action_ids.contains(&action_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_async_flow(
        &mut self,
        inc_id: i64,
        quest_id: i32,
        node_id: i32,
        bt_type: BtType,
        flow_list_name: &str,
        flow_id: i32,
        state_id: i32,
        owner_entity_id: i64,
    ) {
        if inc_id == 0 {
            return;
        }
        while self.async_flows.len() >= 32 {
            self.async_flows.remove(0);
        }
        self.async_flows.push(AsyncFlowEntry {
            inc_id,
            quest_id,
            node_id,
            bt_type,
            flow_id,
            state_id,
            owner_entity_id,
            flow_list_name: flow_list_name.to_string(),
        });
    }

    pub fn find_async_flow(&mut self, inc_id: i64) -> Option<&mut AsyncFlowEntry> {
        if inc_id == 0 {
            return None;
        }
        self.async_flows.iter_mut().find(|af| af.inc_id == inc_id)
    }

    pub fn take_async_flow(&mut self, inc_id: i64) -> Option<AsyncFlowEntry> {
        if inc_id == 0 {
            return None;
        }
        let pos = self.async_flows.iter().position(|af| af.inc_id == inc_id)?;
        Some(self.async_flows.swap_remove(pos))
    }

    pub fn clear_action_node_progress_for_owner(&mut self, owner_id: i32) {
        let prefix = ((owner_id as u32) as u64) << 32;
        let mask = 0xFFFF_FFFF_0000_0000u64;
        self.action_node_next_idx
            .retain(|&k, _| (k & mask) != prefix);
    }

    pub fn has_pending_flow_for_node(&self, quest_id: i32, node_id: i32) -> bool {
        self.pending_flows
            .iter()
            .any(|q| q.quest_id == quest_id && q.node_id == node_id)
    }

    pub fn enqueue_pending_flow(
        &mut self,
        quest_id: i32,
        node_id: i32,
        flow_list_name: &str,
        flow_id: i32,
        state_id: i32,
    ) {
        if self.active_flow_inc_id != 0
            && self.active_flow_quest_id == quest_id
            && self.active_flow_node_id == node_id
            && self.active_flow_id == flow_id
            && self.active_flow_state_id == state_id
            && self.active_flow_list_name == flow_list_name
        {
            return;
        }
        if self.pending_flows.iter().any(|q| {
            q.quest_id == quest_id
                && q.node_id == node_id
                && q.flow_id == flow_id
                && q.state_id == state_id
                && q.flow_list_name == flow_list_name
        }) {
            return;
        }
        self.pending_flows.push(ScenePendingFlow {
            quest_id,
            node_id,
            flow_list_name: flow_list_name.to_string(),
            flow_id,
            state_id,
        });
    }

    pub fn pop_pending_flow(&mut self) -> Option<ScenePendingFlow> {
        if self.pending_flows.is_empty() {
            None
        } else {
            Some(self.pending_flows.remove(0))
        }
    }

    pub fn clear_pending_flows(&mut self) {
        self.pending_flows.clear();
    }

    pub fn set_deferred_join_flow(
        &mut self,
        inc_id: i32,
        namespace: &str,
        id: i32,
        state: i32,
    ) {
        self.deferred_join_flow = Some(DeferredJoinFlow {
            inc_id,
            namespace: namespace.to_string(),
            id,
            state,
        });
    }

    pub fn clear_deferred_join_flow(&mut self) {
        self.deferred_join_flow = None;
    }

    pub fn spawn(&mut self, storage: EntityComponentStorage) -> EntityHandle {
        self.spawn_with_optional_net_id(storage, None)
            .expect("auto net_id allocation cannot conflict")
    }

    pub fn spawn_with_net_id(
        &mut self,
        net_id: i64,
        storage: EntityComponentStorage,
    ) -> Result<EntityHandle, &'static str> {
        if net_id <= 0 {
            return Err("InvalidEntityId");
        }
        if self.net_id_map.contains_key(&net_id) {
            return Err("EntityIdAlreadyExists");
        }
        self.spawn_with_optional_net_id(storage, Some(net_id))
    }

    fn spawn_with_optional_net_id(
        &mut self,
        mut storage: EntityComponentStorage,
        requested_net_id: Option<i64>,
    ) -> Result<EntityHandle, &'static str> {
        let id = match requested_net_id {
            Some(nid) => {
                self.ensure_next_entity_id_at_least(nid + 1);
                nid
            }
            None => self.alloc_next_entity_id(),
        };

        storage.net_id = id;
        if let Some(ctx) = &mut storage.motor_da_ctx
            && ctx.context_id == 0
        {
            ctx.context_id = id;
        }
        if let Some(fsm) = &mut storage.fsm {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            fsm.init_runtime(now_ms)?;
        }
        let has_fsm = storage.fsm.is_some();
        let index = self.entities.len();
        self.net_id_map.insert(id, index);
        self.entities.push(storage);
        let _ = self.refresh_fsm_hate_at_index(index, false);
        self.refresh_summoned_fsm_hate(id);
        if has_fsm {
            self.fsm_wakes.mark_dirty(id);
        }
        Ok(EntityHandle { index, net_id: id })
    }

    pub fn alloc_next_entity_id(&mut self) -> i64 {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        id
    }

    pub fn ensure_next_entity_id_at_least(&mut self, next_id: i64) {
        if next_id > self.next_entity_id {
            self.next_entity_id = next_id;
        }
    }

    pub fn set_fsm_encounter_target(
        &mut self,
        fsm_entity_id: i64,
        target_entity_id: Option<i64>,
    ) -> Result<bool, &'static str> {
        let index = *self
            .net_id_map
            .get(&fsm_entity_id)
            .ok_or("EntityNotFound")?;
        if self.entities[index].fsm.is_none() {
            return Err("EntityFsmNotFound");
        }
        if let Some(target_id) = target_entity_id {
            if !self.net_id_map.contains_key(&target_id) {
                return Err("TargetEntityNotFound");
            }
            if i32::try_from(target_id).is_err() {
                return Err("FsmBlackboardTargetOutOfRange");
            }
        }
        let fsm = self.entities[index]
            .fsm
            .as_mut()
            .ok_or("EntityFsmNotFound")?;

        let target = target_entity_id.map(|id| id as i32);
        let changed = fsm.set_encounter_target(target);
        if changed {
            fsm.mark_dirty(wake_reason::BLACKBOARD);
            self.fsm_wakes.mark_dirty(fsm_entity_id);
        }
        Ok(changed)
    }

    pub fn mark_fsm_dirty(&mut self, entity_id: i64, reason: u16) {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return;
        };
        let Some(fsm) = self.entities[index].fsm.as_mut() else {
            return;
        };
        if fsm.mark_dirty(reason) {
            self.fsm_wakes.mark_dirty(entity_id);
        }
    }

    pub fn update_fsm_hate_target(&mut self, entity_id: i64, has_target: bool) -> bool {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return false;
        };
        let previous = self.fsm_hate_states.insert(entity_id, has_target);
        if previous == Some(has_target) {
            return false;
        }
        let changed = self.refresh_fsm_hate_at_index(index, false);
        self.refresh_summoned_fsm_hate(entity_id);
        changed
    }

    fn refresh_fsm_hate_at_index(&mut self, index: usize, enqueue: bool) -> bool {
        let entity_id = self.entities[index].net_id;
        let mut in_hate = self.fsm_hate_states.get(&entity_id).copied().unwrap_or(false);
        if in_hate
            && let Some(summoner) = self.entities[index].summoner
            && self.net_id_map.contains_key(&summoner.summoner_id)
        {
            in_hate = self
                .fsm_hate_states
                .get(&summoner.summoner_id)
                .copied()
                .unwrap_or(false);
        }
        let Some(fsm) = self.entities[index].fsm.as_mut() else {
            return false;
        };
        if !fsm.set_in_hate(in_hate) {
            return false;
        }
        let should_wake = fsm.mark_dirty(wake_reason::HATE);
        if enqueue && should_wake {
            self.fsm_wakes.mark_dirty(entity_id);
        }
        true
    }

    fn refresh_summoned_fsm_hate(&mut self, summoner_id: i64) {
        let indices: Vec<usize> = self
            .entities
            .iter()
            .enumerate()
            .filter_map(|(idx, e)| {
                e.summoner
                    .filter(|s| s.summoner_id == summoner_id)
                    .map(|_| idx)
            })
            .collect();
        for idx in indices {
            let _ = self.refresh_fsm_hate_at_index(idx, true);
        }
    }

    pub fn mark_fsm_root_dirty(&mut self, entity_id: i64, fsm_id: i32, reason: u16) {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return;
        };
        let Some(fsm) = self.entities[index].fsm.as_mut() else {
            return;
        };
        if crate::fsm::transition_engine::mark_root_dirty(fsm, fsm_id, reason) {
            self.fsm_wakes.mark_dirty(entity_id);
        }
    }

    pub fn queue_fsm_sync(&mut self, entity_id: i64) {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return;
        };
        if self.entities[index].fsm.is_some() {
            self.fsm_wakes.mark_dirty(entity_id);
        }
    }

    pub fn init_fsm_runtimes(&mut self, now_ms: i64) -> Result<(), &'static str> {
        let mut wake_ids = Vec::new();
        for entity in &mut self.entities {
            if let Some(fsm) = entity.fsm.as_mut() {
                fsm.init_runtime(now_ms)?;
                wake_ids.push(entity.net_id);
            }
        }
        for net_id in wake_ids {
            self.fsm_wakes.mark_dirty(net_id);
        }
        Ok(())
    }

    pub fn sync_fsm_deadlines_for_entity(&mut self, entity_id: i64) {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return;
        };
        let Some(fsm) = self.entities[index].fsm.as_ref() else {
            return;
        };
        sync_fsm_deadlines_with(&mut self.fsm_wakes, entity_id, fsm);
    }

    pub fn hate_targets_formation(&self, hate_list: &[AiHateEntity]) -> bool {
        let Ok(formation_index) = usize::try_from(self.formation_info.cur_formation) else {
            return false;
        };
        let Some(formation) = self.formation_info.formations.get(formation_index) else {
            return false;
        };
        for hate in hate_list {
            if hate.HatredValue < 1 {
                continue;
            }
            for maybe_role in &formation.roles {
                let Some(role) = maybe_role else {
                    continue;
                };
                if role.entity_id == hate.EntityId {
                    return true;
                }
            }
        }
        false
    }

    pub fn set_battle_entity_active(&mut self, entity_id: i64, active: bool) {
        if active {
            self.active_battle_entities.insert(entity_id);
        } else {
            self.active_battle_entities.remove(&entity_id);
        }
        self.update_battle_state();
    }

    pub fn update_battle_entity_from_hate(&mut self, entity_id: i64, hate_list: &[AiHateEntity]) {
        let active = self.hate_targets_formation(hate_list);
        self.set_battle_entity_active(entity_id, active);
    }

    pub fn force_battle_state(&mut self, mode: bool) {
        self.active_battle_entities.clear();
        self.player_in_battle = mode;
        self.battle_state_dirty = self.player_in_battle != self.battle_state_notified;
    }

    pub fn combine_relation(&self, combine_entity_id: i64) -> Option<CombineRelation> {
        self.combine_relations.get(&combine_entity_id).cloned()
    }

    pub fn register_combine_relation(
        &mut self,
        combine_entity_id: i64,
        relation: CombineRelation,
    ) -> Result<CombineRegisterResult, &'static str> {
        if let Some(existing) = self.combine_relations.get(&combine_entity_id) {
            return if existing.matches(&relation) {
                Ok(CombineRegisterResult::Existing)
            } else {
                Err("CombineRelationConflict")
            };
        }
        self.combine_relations.insert(combine_entity_id, relation);
        Ok(CombineRegisterResult::Added)
    }

    pub fn detach_combine_relation(
        &mut self,
        combine_entity_id: i64,
        target_entity_id: i64,
    ) -> Option<CombineRelation> {
        let relation = self.combine_relations.get(&combine_entity_id).cloned()?;
        if relation.target_entity_id != target_entity_id {
            return None;
        }
        self.combine_relations.remove(&combine_entity_id);
        Some(relation)
    }

    pub fn remove_combine_relations_for_entity(&mut self, entity_id: i64) {
        let affected: Vec<i64> = self
            .combine_relations
            .iter()
            .filter_map(|(&k, rel)| {
                (k == entity_id || rel.target_entity_id == entity_id).then_some(k)
            })
            .collect();
        for combine_entity_id in affected {
            if let Some(relation) = self.combine_relations.remove(&combine_entity_id)
                && relation.target_entity_id == entity_id
                && combine_entity_id != entity_id
            {
                self.pending_combine_detaches.push(CombineDetach {
                    combine_entity_id,
                    target_entity_id: relation.target_entity_id,
                });
            }
        }
    }

    pub fn clear_pending_combine_detaches(&mut self) {
        self.pending_combine_detaches.clear();
    }

    pub fn queue_entity_removal_at(
        &mut self,
        eid: i64,
        due_ms: i64,
        remove_type: ERemoveEntityType,
    ) {
        if !self.net_id_map.contains_key(&eid) {
            return;
        }
        if let Some(existing) = self
            .pending_killed_removals
            .iter_mut()
            .find(|e| e.entity_id == eid)
        {
            if due_ms <= existing.due_ms {
                existing.due_ms = due_ms;
                existing.remove_type = remove_type;
            }
            return;
        }
        self.pending_killed_removals.push(KilledRemovalEntry {
            entity_id: eid,
            due_ms,
            remove_type,
        });
    }

    pub fn queue_killed_monster_removal_at(&mut self, eid: i64, due_ms: i64) {
        let Some(&idx) = self.net_id_map.get(&eid) else {
            return;
        };
        if self.entities[idx].config.entity_type != EntityType::Monster {
            return;
        }
        self.queue_entity_removal_at(eid, due_ms, ERemoveEntityType::HpIsZero);
    }

    pub fn next_killed_removal_wake_delay_ms(&self, now_ms: i64) -> Option<i64> {
        self.pending_killed_removals
            .iter()
            .map(|e| if e.due_ms <= now_ms { 0 } else { e.due_ms - now_ms })
            .min()
    }

    pub fn flush_due_killed_removals(
        &mut self,
        fs: &FileSystem,
        now_ms: i64,
        remove_infos: &mut Vec<EntityRemoveInfo>,
    ) {
        if self.pending_killed_removals.is_empty() {
            return;
        }
        let entries = std::mem::take(&mut self.pending_killed_removals);
        for entry in entries {
            if entry.due_ms > now_ms {
                self.pending_killed_removals.push(entry);
                continue;
            }
            if !self.net_id_map.contains_key(&entry.entity_id) {
                continue;
            }
            if self.remove(fs, entry.entity_id).is_err() {
                self.pending_killed_removals.push(entry);
                continue;
            }
            remove_infos.push(EntityRemoveInfo {
                EntityId: entry.entity_id,
                Type: Some(entry.remove_type),
            });
        }
    }

    pub fn schedule_deferred_spatial_spawns(&mut self, due_ms: i64) {
        self.deferred_spatial_spawns_at_ms = Some(match self.deferred_spatial_spawns_at_ms {
            Some(cur) => cur.min(due_ms),
            None => due_ms,
        });
    }

    pub fn next_deferred_spatial_spawns_delay_ms(&self, now_ms: i64) -> Option<i64> {
        self.deferred_spatial_spawns_at_ms.map(|due| due - now_ms)
    }

    pub fn take_due_deferred_spatial_spawns(&mut self, now_ms: i64) -> bool {
        if self
            .deferred_spatial_spawns_at_ms
            .is_some_and(|due| due <= now_ms)
        {
            self.deferred_spatial_spawns_at_ms = None;
            true
        } else {
            false
        }
    }

    pub fn pending_update_wake_delay_ms(&self, now_ms: i64) -> Option<i64> {
        pending_update_wake_delay(
            self.battle_state_dirty,
            self.pending_combine_detaches.len(),
            self.next_killed_removal_wake_delay_ms(now_ms),
        )
    }

    pub fn signal_fsm_dissolve_combine(&mut self, entity_id: i64) {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return;
        };
        if let Some(fsm) = self.entities[index].fsm.as_mut() {
            fsm.signal_dissolve_combine();
            fsm.mark_dirty(wake_reason::DISSOLVE_COMBINE);
            self.fsm_wakes.mark_dirty(entity_id);
        }
    }

    pub fn note_fsm_skill_start(&mut self, entity_id: i64, skill_id: i32, now_ms: i64) {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return;
        };
        if let Some(fsm) = self.entities[index].fsm.as_mut() {
            let _ = fsm.init_runtime(now_ms);
            fsm.note_skill_start(skill_id);
        }
    }

    pub fn signal_fsm_skill_end(&mut self, entity_id: i64, skill_id: i32, now_ms: i64) {
        let Some(&index) = self.net_id_map.get(&entity_id) else {
            return;
        };
        if let Some(fsm) = self.entities[index].fsm.as_mut() {
            let _ = fsm.init_runtime(now_ms);
            if fsm.signal_skill_end(skill_id) {
                self.fsm_wakes.mark_dirty(entity_id);
            }
        }
    }

    pub fn remove_combine_notify(detach: CombineDetach) -> CombatReceiveData {
        CombatReceiveData {
            Message: Some(CombatReceiveDataMessageOneof::CombatNotifyData(
                CombatNotifyData {
                    CombatCommon: Some(CombatCommon {
                        EntityId: detach.combine_entity_id,
                        ..Default::default()
                    }),
                    Message: Some(CombatNotifyDataMessageOneof::RemoveCombineRelationNotify(
                        RemoveCombineRelationNotify {
                            CombineEntity: detach.combine_entity_id,
                            TargetEntity: detach.target_entity_id,
                        },
                    )),
                },
            )),
        }
    }

    pub fn find_entity_net_id_by_config_id(&self, config_id: i32) -> Option<i64> {
        for e in &self.entities {
            if e.config.config_id == config_id && e.config.entity_type != EntityType::Player {
                return Some(e.net_id);
            }
        }
        for e in &self.entities {
            if e.config.config_id == config_id {
                return Some(e.net_id);
            }
        }
        None
    }

    pub fn find_config_id_by_net_id(&self, net_id: i64) -> Option<i32> {
        let &idx = self.net_id_map.get(&net_id)?;
        Some(self.entities[idx].config.config_id)
    }

    pub fn collect_entities_in_radius(
        &self,
        center: [f32; 3],
        radius: f32,
        only_monsters_and_chests: bool,
        out_net_ids: &mut Vec<i64>,
    ) {
        let r2 = radius * radius;
        for e in &self.entities {
            if e.config.entity_type == EntityType::Player {
                continue;
            }
            if only_monsters_and_chests
                && e.config.entity_type != EntityType::Monster
                && e.config.entity_type != EntityType::SceneItem
            {
                continue;
            }
            let Some(pos) = e.position else {
                continue;
            };
            let dx = pos.location[0] - center[0];
            let dy = pos.location[1] - center[1];
            let dz = pos.location[2] - center[2];
            if dx * dx + dy * dy + dz * dz <= r2 {
                out_net_ids.push(e.net_id);
            }
        }
    }

    pub fn alloc_action_inc_id(&mut self) -> i32 {
        let id = self.next_action_inc_id;
        self.next_action_inc_id = if id == i32::MAX { 1 } else { id + 1 };
        id
    }

    pub fn set_entity_visibility(
        &mut self,
        net_id: i64,
        visible: bool,
        combat_pack: &mut Vec<CombatReceiveData>,
    ) -> bool {
        let Some(&idx) = self.net_id_map.get(&net_id) else {
            return false;
        };
        self.entities[idx].visible = visible;
        self.entities[idx].actor_visible = visible;
        combat_pack.push(CombatReceiveData {
            Message: Some(CombatReceiveDataMessageOneof::CombatNotifyData(
                CombatNotifyData {
                    CombatCommon: Some(CombatCommon {
                        EntityId: net_id,
                        MessageId: -1,
                        PreMessageId: -1,
                        ..Default::default()
                    }),
                    Message: Some(CombatNotifyDataMessageOneof::EntityIsVisibleNotify(
                        EntityIsVisibleNotify {
                            Id: net_id,
                            IsVisible: visible,
                            CombatCommon: None,
                        },
                    )),
                },
            )),
        });
        combat_pack.push(CombatReceiveData {
            Message: Some(CombatReceiveDataMessageOneof::CombatNotifyData(
                CombatNotifyData {
                    CombatCommon: Some(CombatCommon {
                        EntityId: net_id,
                        MessageId: -1,
                        PreMessageId: -1,
                        ..Default::default()
                    }),
                    Message: Some(CombatNotifyDataMessageOneof::ActorVisibleNotify(
                        ActorVisibleNotify {
                            Id: net_id,
                            IsActorVisible: visible,
                        },
                    )),
                },
            )),
        });
        true
    }

    pub fn update_entity_state_tag(
        &mut self,
        remaps: &Table<GameplayTagRemap, i32>,
        net_id: i64,
        state_name: &str,
    ) -> Option<StateTagUpdate> {
        let new_tag_id = if state_name.is_empty() {
            0
        } else {
            gameplay_tags::id_from_name(remaps, state_name).unwrap_or(0)
        };
        self.update_entity_state_tag_id(net_id, new_tag_id)
    }

    pub fn update_entity_state_tag_id(
        &mut self,
        net_id: i64,
        new_tag_id: i32,
    ) -> Option<StateTagUpdate> {
        let &idx = self.net_id_map.get(&net_id)?;
        let storage = &mut self.entities[idx];
        let old_tag_id = storage.state_tag.map_or(0, |st| st.state_tag_id);
        storage.state_tag = if new_tag_id != 0 {
            Some(StateTagComponent {
                state_tag_id: new_tag_id,
            })
        } else {
            None
        };

        if old_tag_id != 0 && old_tag_id != new_tag_id && let Some(tags) = storage.tag.as_mut() {
            tags.entity_common_tags.retain(|&t| t != old_tag_id);
            tags.remove_tag(old_tag_id);
        }
        if new_tag_id != 0 {
            let tags = storage.tag.get_or_insert_with(TagComponent::default);
            if !tags.entity_common_tags.contains(&new_tag_id) {
                tags.entity_common_tags.push(new_tag_id);
            }
            tags.set_tag_count(new_tag_id, 1);
        }
        Some(StateTagUpdate {
            old_tag_id,
            new_tag_id,
        })
    }

    pub fn set_trial_roles(&mut self, role_ids: &[i32]) {
        self.trial_role_ids.clear();
        self.trial_role_ids.extend_from_slice(role_ids);
    }

    pub fn upsert_bt_timer(
        &mut self,
        owner_id: i32,
        inc_id: i64,
        node_id: i32,
        timer_type: &str,
        end_time_ms: i64,
    ) -> ActiveBtTimer {
        if let Some(t) = self
            .bt_timers
            .iter_mut()
            .find(|t| t.owner_id == owner_id && t.inc_id == inc_id && t.timer_type == timer_type)
        {
            t.node_id = node_id;
            t.end_time_ms = end_time_ms;
            t.paused = false;
            t.pause_time_ms = 0;
            return t.clone();
        }
        let timer = ActiveBtTimer {
            owner_id,
            inc_id,
            node_id,
            timer_type: timer_type.to_string(),
            end_time_ms,
            pause_time_ms: 0,
            paused: false,
        };
        self.bt_timers.push(timer.clone());
        timer
    }

    #[allow(clippy::too_many_arguments)]
    pub fn adjust_bt_timer(
        &mut self,
        owner_id: i32,
        inc_id: i64,
        node_id: i32,
        timer_type: &str,
        now_ms: i64,
        seconds: i32,
        mode: TimerSetType,
    ) -> ActiveBtTimer {
        let delta_ms = i64::from(seconds) * 1000;
        if let Some(t) = self
            .bt_timers
            .iter_mut()
            .find(|t| t.owner_id == owner_id && t.inc_id == inc_id && t.timer_type == timer_type)
        {
            t.node_id = node_id;
            t.end_time_ms = match mode {
                TimerSetType::Add => t.end_time_ms + delta_ms,
                TimerSetType::Sub => now_ms.max(t.end_time_ms - delta_ms),
                TimerSetType::Set => now_ms + delta_ms,
            };
            return t.clone();
        }
        self.upsert_bt_timer(owner_id, inc_id, node_id, timer_type, now_ms + delta_ms.max(0))
    }

    pub fn remove_bt_timer(&mut self, owner_id: i32, inc_id: i64, timer_type: &str) -> bool {
        let Some(pos) = self.bt_timers.iter().position(|t| {
            t.owner_id == owner_id
                && t.inc_id == inc_id
                && (timer_type.is_empty() || t.timer_type == timer_type)
        }) else {
            return false;
        };
        self.bt_timers.swap_remove(pos);
        true
    }

    pub fn clear_bt_timers(&mut self) {
        self.bt_timers.clear();
    }

    pub fn remove_bt_timers_for_owner(&mut self, owner_id: i32) {
        self.bt_timers.retain(|t| t.owner_id != owner_id);
    }

    pub fn next_quest_wake_delay_ms(&self, now_ms: i64) -> Option<i64> {
        [
            self.quest_timers.next_wake_delay_ms(now_ms),
            self.lp_timers.next_wake_delay_ms(now_ms),
            self.fail_timers.next_wake_delay_ms(now_ms),
            self.lp_fail_timers.next_wake_delay_ms(now_ms),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn clear_bt_occupations(&mut self) {
        self.bt_occupations.clear();
    }

    pub fn find_occupation_conflict(
        &self,
        resource_name: &str,
        owner_id: i32,
        inc_id: i64,
    ) -> Option<&BtOccupation> {
        self.bt_occupations.iter().find(|occ| {
            occ.resource_name == resource_name
                && !(occ.owner_id == owner_id && occ.inc_id == inc_id)
        })
    }

    pub fn upsert_occupation(
        &mut self,
        bt_type: BtType,
        owner_id: i32,
        inc_id: i64,
        node_id: i32,
        resource_name: &str,
    ) {
        if let Some(occ) = self
            .bt_occupations
            .iter_mut()
            .find(|o| o.resource_name == resource_name)
        {
            occ.bt_type = bt_type;
            occ.owner_id = owner_id;
            occ.inc_id = inc_id;
            occ.node_id = node_id;
            return;
        }
        self.bt_occupations.push(BtOccupation {
            bt_type,
            owner_id,
            inc_id,
            node_id,
            resource_name: resource_name.to_string(),
        });
    }

    pub fn release_occupations_for_tree(&mut self, owner_id: i32, inc_id: i64) {
        self.bt_occupations
            .retain(|o| !(o.owner_id == owner_id && o.inc_id == inc_id));
    }

    pub fn release_occupations_for_node(&mut self, owner_id: i32, node_id: i32) {
        self.bt_occupations
            .retain(|o| !(o.owner_id == owner_id && o.node_id == node_id));
    }

    pub fn has_save_node_entered(&self, owner_id: i32, node_id: i32) -> bool {
        let k = save_node_key(owner_id, node_id);
        self.save_node_keys.contains(&k)
    }

    pub fn mark_save_node_entered(&mut self, owner_id: i32, node_id: i32) -> bool {
        if self.has_save_node_entered(owner_id, node_id) {
            return false;
        }
        self.save_node_keys.push(save_node_key(owner_id, node_id));
        true
    }

    pub fn clear_save_nodes_for_owner(&mut self, owner_id: i32) {
        let prefix = ((owner_id as u32) as u64) << 32;
        let mask = 0xFFFF_FFFF_0000_0000u64;
        self.save_node_keys.retain(|&k| (k & mask) != prefix);
    }

    pub fn set_entity_position(
        &mut self,
        net_id: i64,
        pos: [f32; 3],
        yaw_opt: Option<f32>,
    ) -> bool {
        let Some(&idx) = self.net_id_map.get(&net_id) else {
            return false;
        };
        let slot = &mut self.entities[idx].position;
        if let Some(p) = slot {
            p.location = pos;
            if let Some(yaw) = yaw_opt {
                p.rotation[2] = yaw;
            }
        } else {
            *slot = Some(crate::component::entity::PositionComponent {
                location: pos,
                rotation: [0.0, 0.0, yaw_opt.unwrap_or(0.0)],
            });
        }
        true
    }

    pub fn set_current_role_gravity(&mut self, gravity_dir: [f32; 3]) {
        let role_eid = self.current_role_entity_id();
        if role_eid <= 0 {
            return;
        }
        let Some(&idx) = self.net_id_map.get(&role_eid) else {
            return;
        };
        self.entities[idx].world_item.gravity =
            Some(crate::component::entity::GravityComponent {
                location: gravity_dir,
            });
    }

    pub fn has_failed_dungeon(&self, inst_id: i32) -> bool {
        self.failed_dungeons.contains(&inst_id)
    }

    pub fn record_failed_dungeon(&mut self, inst_id: i32) {
        if inst_id > 0 && !self.has_failed_dungeon(inst_id) {
            self.failed_dungeons.push(inst_id);
        }
    }

    pub fn clear_failed_dungeon(&mut self, inst_id: i32) {
        self.failed_dungeons.retain(|&id| id != inst_id);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_dynamic_interact_option(
        &mut self,
        guid: &str,
        entity_net_id: i64,
        entity_config_id: i32,
        quest_id: i32,
        node_id: i32,
        kind: DynamicInteractKind,
        flow_list_name: &str,
        flow_id: i32,
        state_id: i32,
        actions_json: &str,
        text: &str,
    ) {
        self.upsert_dynamic_interact_option_with_bt(
            guid,
            entity_net_id,
            entity_config_id,
            BtType::BtTypeQuest,
            quest_id,
            node_id,
            kind,
            flow_list_name,
            flow_id,
            state_id,
            actions_json,
            text,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_dynamic_interact_option_with_bt(
        &mut self,
        guid: &str,
        entity_net_id: i64,
        entity_config_id: i32,
        bt_type: BtType,
        quest_id: i32,
        node_id: i32,
        kind: DynamicInteractKind,
        flow_list_name: &str,
        flow_id: i32,
        state_id: i32,
        actions_json: &str,
        text: &str,
    ) {
        if let Some(pos) = self
            .dynamic_interact_options
            .iter()
            .position(|o| o.option_guid == guid)
        {
            self.dynamic_interact_options.swap_remove(pos);
        }
        self.dynamic_interact_options.push(DynamicInteractOption {
            option_guid: guid.to_string(),
            entity_net_id,
            entity_config_id,
            bt_type,
            quest_id,
            node_id,
            kind,
            flow_list_name: flow_list_name.to_string(),
            flow_id,
            state_id,
            actions_json: actions_json.to_string(),
            text: text.to_string(),
        });
    }

    pub fn find_dynamic_interact_option(&self, guid: &str) -> Option<&DynamicInteractOption> {
        self.dynamic_interact_options
            .iter()
            .find(|o| o.option_guid == guid)
    }

    pub fn take_matching_dynamic_interact_options(
        &mut self,
        entity_config_id: i32,
        flow_list_name_opt: Option<&str>,
        flow_id_opt: Option<i32>,
    ) -> Vec<DynamicInteractOption> {
        let mut removed = Vec::new();
        let mut i = self.dynamic_interact_options.len();
        while i > 0 {
            i -= 1;
            let matches = {
                let opt = &self.dynamic_interact_options[i];
                (entity_config_id <= 0 || opt.entity_config_id == entity_config_id)
                    && flow_list_name_opt.is_none_or(|fname| opt.flow_list_name == fname)
                    && flow_id_opt.is_none_or(|fid| fid <= 0 || opt.flow_id == fid)
            };
            if matches {
                removed.push(self.dynamic_interact_options.swap_remove(i));
            }
        }
        removed
    }

    pub fn take_dynamic_interact_options_for_quest_node(
        &mut self,
        quest_id: i32,
        node_id: i32,
    ) -> Vec<DynamicInteractOption> {
        self.take_dynamic_interact_options_for_bt_node(BtType::BtTypeQuest, quest_id, node_id)
    }

    pub fn take_dynamic_interact_options_for_bt_node(
        &mut self,
        bt_type: BtType,
        owner_id: i32,
        node_id: i32,
    ) -> Vec<DynamicInteractOption> {
        let mut removed = Vec::new();
        let mut i = self.dynamic_interact_options.len();
        while i > 0 {
            i -= 1;
            let opt = &self.dynamic_interact_options[i];
            if opt.bt_type == bt_type && opt.quest_id == owner_id && opt.node_id == node_id {
                removed.push(self.dynamic_interact_options.swap_remove(i));
            }
        }
        removed
    }

    pub fn add_play_bubble(&mut self, entity_config_id: i32, action_guid: &str) {
        if entity_config_id <= 0 || action_guid.is_empty() {
            return;
        }
        if self
            .play_bubbles
            .iter()
            .any(|r| r.entity_config_id == entity_config_id && r.action_guid == action_guid)
        {
            return;
        }
        self.play_bubbles.push(PlayBubbleRecord {
            entity_config_id,
            action_guid: action_guid.to_string(),
        });
    }

    pub fn take_play_bubbles_for_entity(
        &mut self,
        entity_config_id: i32,
    ) -> Vec<PlayBubbleRecord> {
        let mut removed = Vec::new();
        let mut i = self.play_bubbles.len();
        while i > 0 {
            i -= 1;
            if self.play_bubbles[i].entity_config_id == entity_config_id {
                removed.push(self.play_bubbles.swap_remove(i));
            }
        }
        removed
    }

    pub fn debug_spawn_route(&self, config_id: i64) -> Option<DebugSpawnRoute> {
        self.debug_spawn_routes.get(&config_id).copied()
    }

    pub fn register_debug_spawn_route(
        &mut self,
        net_id: i64,
        config_id: i64,
        source_map_id: i32,
        repeat_boss: bool,
    ) -> Result<(), &'static str> {
        if self.debug_spawn_route_owners.contains_key(&net_id) {
            return Err("DebugSpawnRouteOwnerExists");
        }
        if let Some(route) = self.debug_spawn_routes.get_mut(&config_id) {
            if !route.matches(source_map_id, repeat_boss) {
                return Err("DebugSpawnRouteConflict");
            }
            self.debug_spawn_route_owners.insert(net_id, config_id);
            route.owner_count += 1;
            return Ok(());
        }
        self.debug_spawn_routes.insert(
            config_id,
            DebugSpawnRoute {
                source_map_id,
                repeat_boss,
                owner_count: 1,
            },
        );
        self.debug_spawn_route_owners.insert(net_id, config_id);
        Ok(())
    }

    pub fn unregister_debug_spawn_route(&mut self, net_id: i64) {
        let Some(config_id) = self.debug_spawn_route_owners.remove(&net_id) else {
            return;
        };
        if let Some(route) = self.debug_spawn_routes.get_mut(&config_id) {
            if route.owner_count > 1 {
                route.owner_count -= 1;
            } else {
                self.debug_spawn_routes.remove(&config_id);
            }
        }
    }

    pub fn append_battle_state_notify(&mut self, data: &mut Vec<CombatReceiveData>) -> bool {
        if !self.battle_state_dirty {
            return false;
        }
        let entity_id = self.current_role_entity_id();
        data.push(CombatReceiveData {
            Message: Some(CombatReceiveDataMessageOneof::CombatNotifyData(
                CombatNotifyData {
                    CombatCommon: Some(CombatCommon {
                        EntityId: entity_id,
                        ..Default::default()
                    }),
                    Message: Some(CombatNotifyDataMessageOneof::PlayerBattleStateChangeNotify(
                        PlayerBattleStateChangeNotify {
                            PlayerId: self.player_id,
                            InBattle: self.player_in_battle,
                        },
                    )),
                },
            )),
        });
        self.battle_state_notified = self.player_in_battle;
        self.battle_state_dirty = false;
        true
    }

    /// Echo (Phantom) group role entity of `role_id`, while ChangePhantomFormation is in effect.
    pub fn phantom_role_entity_id(&self, role_id: i32) -> Option<i64> {
        self.phantom_role_entity_ids
            .iter()
            .copied()
            .find(|&eid| self.entity(eid).is_some_and(|s| s.config.config_id == role_id))
    }

    pub fn current_role_entity_id(&self) -> i64 {
        if self.phantom_cur_role > 0
            && let Some(eid) = self.phantom_role_entity_id(self.phantom_cur_role)
        {
            return eid;
        }
        let Ok(idx) = usize::try_from(self.formation_info.cur_formation) else {
            return -1;
        };
        let Some(formation) = self.formation_info.formations.get(idx) else {
            return -1;
        };
        for maybe_role in &formation.roles {
            let Some(role) = maybe_role else {
                continue;
            };
            if role.role_id == formation.cur_role {
                return role.entity_id;
            }
        }
        -1
    }

    fn update_battle_state(&mut self) {
        self.player_in_battle = !self.active_battle_entities.is_empty();
        self.battle_state_dirty = self.player_in_battle != self.battle_state_notified;
    }

    pub fn delete_entity_files(
        fs: &FileSystem,
        player_id: i32,
        scene_id: i32,
        net_id: i64,
    ) -> io::Result<()> {
        fs.delete_tree(&format!("player/{player_id}/scene/{scene_id}/entity/{net_id}"))
    }

    pub fn remove(&mut self, fs: &FileSystem, net_id: i64) -> io::Result<()> {
        let Some(&index) = self.net_id_map.get(&net_id) else {
            return Ok(());
        };
        Self::delete_entity_files(fs, self.player_id, self.instance_id, net_id)?;
        self.remove_entity_state(index, net_id);
        Ok(())
    }

    pub fn rollback_spawn(&mut self, net_id: i64) {
        let Some(&index) = self.net_id_map.get(&net_id) else {
            return;
        };
        self.remove_entity_state(index, net_id);
    }

    fn remove_entity_state(&mut self, index: usize, net_id: i64) {
        self.clear_fsm_encounter_target_references(net_id);
        self.remove_combine_relations_for_entity(net_id);
        self.dynamic_interact_options
            .retain(|o| o.entity_net_id != net_id);
        self.fsm_wakes.cancel_entity(net_id);
        self.unregister_debug_spawn_route(net_id);
        self.active_battle_entities.remove(&net_id);
        self.rewarded_kills.remove(&net_id);
        self.update_battle_state();
        self.entities.swap_remove(index);
        self.net_id_map.remove(&net_id);
        self.fsm_hate_states.remove(&net_id);
        if index < self.entities.len() {
            let swapped_id = self.entities[index].net_id;
            self.net_id_map.insert(swapped_id, index);
        }
        self.refresh_summoned_fsm_hate(net_id);
    }

    fn clear_fsm_encounter_target_references(&mut self, target_entity_id: i64) {
        let Ok(target_id) = i32::try_from(target_entity_id) else {
            return;
        };
        let mut dirty_ids = Vec::new();
        for entity in &mut self.entities {
            if let Some(fsm) = entity.fsm.as_mut()
                && fsm.clear_encounter_target_reference(target_id)
            {
                fsm.mark_dirty(wake_reason::BLACKBOARD);
                dirty_ids.push(entity.net_id);
            }
        }
        for id in dirty_ids {
            self.fsm_wakes.mark_dirty(id);
        }
    }

    pub fn save_persistent(&self, fs: &FileSystem) -> io::Result<()> {
        self.save_instance(fs)?;
        comp_util::save_struct(
            fs,
            &self.formation_info,
            &format!("player/{}/formation_info", self.player_id),
        )?;
        comp_util::save_struct(
            fs,
            &self.explore_tools_info,
            &format!("player/{}/explore_tools_info", self.player_id),
        )
    }

    pub fn save(&self, fs: &FileSystem) -> io::Result<()> {
        self.save_persistent(fs)?;
        for entity in &self.entities {
            entity.save(fs, self.player_id, self.instance_id)?;
        }
        Ok(())
    }

    pub fn save_last_scene(
        &self,
        scene_comp: &PlayerSceneComponent,
        fs: &FileSystem,
    ) -> io::Result<()> {
        comp_util::save_struct::<LastSceneInfo>(
            fs,
            &scene_comp.last_scene_info,
            &format!("player/{}/last_scene", self.player_id),
        )
    }

    pub fn save_entity(&self, fs: &FileSystem, entity: EntityHandle) -> io::Result<()> {
        if let Some(storage) = self.entities.get(entity.index) {
            storage.save(fs, self.player_id, self.instance_id)?;
        }
        Ok(())
    }

    pub fn save_entity_by_net_id(&self, fs: &FileSystem, net_id: i64) -> io::Result<()> {
        if let Some(storage) = self.entity(net_id) {
            storage.save(fs, self.player_id, self.instance_id)?;
        }
        Ok(())
    }

    pub fn save_instance(&self, fs: &FileSystem) -> io::Result<()> {
        comp_util::save_struct(
            fs,
            &self.instance,
            &format!("player/{}/scene/{}/instance", self.player_id, self.instance_id),
        )
    }

    pub fn build_fight_buff_infos(
        &mut self,
        role_buffs: &[BuffAdditionEntry],
        net_id: i64,
    ) -> Vec<FightBuffInformation> {
        let mut infos = Vec::with_capacity(role_buffs.len());
        for entry in role_buffs {
            self.instance.buff_handle += 1;
            infos.push(create_buff_information(
                self.instance.buff_handle,
                entry.id,
                net_id,
                net_id,
                entry.is_active,
            ));
        }
        infos
    }

    pub fn next_wake_delay_ms(&self, now_ms: i64) -> Option<i64> {
        let fsm = self.fsm_wakes.next_wake_delay_ms(now_ms);
        let quest = self.next_quest_wake_delay_ms(now_ms);
        let pending = self.pending_update_wake_delay_ms(now_ms);
        let spatial = self.next_deferred_spatial_spawns_delay_ms(now_ms);
        [fsm, quest, pending, spatial].into_iter().flatten().min()
    }

    pub fn entity_attribute_and_tags(&self, net_id: i64) -> Option<(Option<&crate::component::entity::AttributeComponent>, Option<&TagComponent>)> {
        let storage = self.entity(net_id)?;
        Some((storage.attribute.as_ref(), storage.tag.as_ref()))
    }
}

pub fn sync_fsm_deadlines_with(wakes: &mut FsmWakeScheduler, entity_id: i64, fsm: &FsmComponent) {
    for runtime in &fsm.runtime_nodes {
        if let Some(due_ms) = runtime.next_timer_due_ms {
            wakes.upsert(WakeEntry {
                entity_id,
                fsm_id: runtime.fsm_id,
                kind: WakeKind::RootTimer,
                due_ms,
            });
        } else {
            wakes.cancel(entity_id, runtime.fsm_id, WakeKind::RootTimer);
        }

        if let Some(due_ms) = runtime.delayed_suicide_due_ms {
            wakes.upsert(WakeEntry {
                entity_id,
                fsm_id: runtime.fsm_id,
                kind: WakeKind::DelayedSuicide,
                due_ms,
            });
        } else {
            wakes.cancel(entity_id, runtime.fsm_id, WakeKind::DelayedSuicide);
        }

        if let Some(due_ms) = runtime.delayed_destroy_due_ms {
            wakes.upsert(WakeEntry {
                entity_id,
                fsm_id: runtime.fsm_id,
                kind: WakeKind::DelayedDestroy,
                due_ms,
            });
        } else {
            wakes.cancel(entity_id, runtime.fsm_id, WakeKind::DelayedDestroy);
        }
    }

    if let Some(due_ms) = fsm.pending_deadline() {
        wakes.upsert(WakeEntry {
            entity_id,
            fsm_id: 0,
            kind: WakeKind::PendingTimeout,
            due_ms,
        });
    } else {
        wakes.cancel(entity_id, 0, WakeKind::PendingTimeout);
    }

    if fsm.paralysis_active
        && let Some(due_ms) = fsm.paralysis_next_ms
    {
        wakes.upsert(WakeEntry {
            entity_id,
            fsm_id: 0,
            kind: WakeKind::Paralysis,
            due_ms,
        });
    } else {
        wakes.cancel(entity_id, 0, WakeKind::Paralysis);
    }
}

pub fn save_struct_generic<T: serde::Serialize>(
    fs: &FileSystem,
    data: &T,
    path: &str,
) -> io::Result<()> {
    comp_util::save_struct(fs, data, path)
}

pub fn asset_tables_have_role(assets: &Assets, role_id: i32) -> bool {
    assets.tables.resolve_base_role_info(role_id).is_some()
}

fn save_node_key(owner_id: i32, node_id: i32) -> u64 {
    (((owner_id as u32) as u64) << 32) | ((node_id as u32) as u64)
}

fn pending_update_wake_delay(
    battle_state_dirty: bool,
    pending_detach_count: usize,
    killed_removal_delay_ms: Option<i64>,
) -> Option<i64> {
    if battle_state_dirty || pending_detach_count != 0 {
        return Some(0);
    }
    killed_removal_delay_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_pending_updates_wake_only_for_pending_work() {
        assert_eq!(pending_update_wake_delay(false, 0, None), None);
        assert_eq!(pending_update_wake_delay(true, 0, None), Some(0));
        assert_eq!(pending_update_wake_delay(false, 1, None), Some(0));
        assert_eq!(pending_update_wake_delay(true, 1, Some(250)), Some(0));
        assert_eq!(pending_update_wake_delay(false, 0, Some(250)), Some(250));
    }

    #[test]
    fn scene_active_flow_replaces_and_clears_owned_flow_name() {
        let mut scene = Scene::from_parts(
            1,
            8,
            SceneInstance::default(),
            FormationInfo::default(),
            ExploreToolsInfo::default(),
        );
        scene.set_active_flow(10, 139_000_025, 21, "剧情_新主线_CE版_序章", 1, 12);
        assert_eq!(scene.active_flow_inc_id, 10);
        assert_eq!(scene.active_flow_id, 1);
        assert_eq!(scene.active_flow_state_id, 12);
        assert_eq!(scene.active_flow_list_name, "剧情_新主线_CE版_序章");

        scene.set_active_flow(11, 139_000_025, 22, "next_flow", 2, 14);
        assert_eq!(scene.active_flow_inc_id, 11);
        assert_eq!(scene.active_flow_list_name, "next_flow");

        scene.clear_active_flow();
        assert_eq!(scene.active_flow_inc_id, 0);
        assert!(scene.active_flow_list_name.is_empty());
    }

    #[test]
    fn now_millis_is_monotonic() {
        let t1 = now_millis();
        let t2 = now_millis();
        assert!(t1 > 0);
        assert!(t2 >= t1);
    }

    #[test]
    fn queue_entity_removal_at_preserves_capture_type_and_waits_until_due() {
        let dir = std::env::temp_dir().join(format!("sonoro_scene_capture_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fs = FileSystem::open(&dir).unwrap();

        let mut scene = Scene::from_parts(
            1,
            8,
            SceneInstance::default(),
            FormationInfo::default(),
            ExploreToolsInfo::default(),
        );
        let handle = scene.spawn(EntityComponentStorage::new(
            0,
            crate::component::entity::small::ConfigComponent {
                camp: 0,
                config_id: 1001,
                config_type: crate::component::entity::small::ConfigType::Level,
                entity_type: EntityType::SceneItem,
                state: crate::component::entity::small::EntityState::Default,
            },
        ));
        let now = 10_000;
        scene.queue_entity_removal_at(handle.net_id, now + 3500, ERemoveEntityType::RemoveTypeCapture);
        assert_eq!(scene.next_killed_removal_wake_delay_ms(now + 1000), Some(2500));

        let mut removed = Vec::new();
        scene.flush_due_killed_removals(&fs, now + 1000, &mut removed);
        assert!(removed.is_empty());
        assert!(scene.entity_handle(handle.net_id).is_some());

        scene.flush_due_killed_removals(&fs, now + 3500, &mut removed);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].EntityId, handle.net_id);
        assert_eq!(removed[0].Type, Some(ERemoveEntityType::RemoveTypeCapture));
        assert!(scene.entity_handle(handle.net_id).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_killed_monster_removal_only_queues_monsters_with_hp_is_zero() {
        let dir = std::env::temp_dir().join(format!("sonoro_scene_monster_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fs = FileSystem::open(&dir).unwrap();

        let mut scene = Scene::from_parts(
            1,
            8,
            SceneInstance::default(),
            FormationInfo::default(),
            ExploreToolsInfo::default(),
        );
        let npc = scene.spawn(EntityComponentStorage::new(
            0,
            crate::component::entity::small::ConfigComponent {
                camp: 0,
                config_id: 2001,
                config_type: crate::component::entity::small::ConfigType::Level,
                entity_type: EntityType::Npc,
                state: crate::component::entity::small::EntityState::Default,
            },
        ));
        let monster = scene.spawn(EntityComponentStorage::new(
            0,
            crate::component::entity::small::ConfigComponent {
                camp: 1,
                config_id: 2002,
                config_type: crate::component::entity::small::ConfigType::Level,
                entity_type: EntityType::Monster,
                state: crate::component::entity::small::EntityState::Default,
            },
        ));

        let now = 20_000;
        scene.queue_killed_monster_removal_at(npc.net_id, now + 1800);
        scene.queue_killed_monster_removal_at(monster.net_id, now + 1800);
        assert_eq!(scene.next_killed_removal_wake_delay_ms(now), Some(1800));

        let mut removed = Vec::new();
        scene.flush_due_killed_removals(&fs, now + 1800, &mut removed);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].EntityId, monster.net_id);
        assert_eq!(removed[0].Type, Some(ERemoveEntityType::HpIsZero));
        assert!(scene.entity_handle(npc.net_id).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
