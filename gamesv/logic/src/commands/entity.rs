use std::borrow::Cow;

use serde_json::Value;
use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::asset_indexes;
use sonoro_gamesv_data::data_tables::Table;
use sonoro_gamesv_data::tables::ai_state_machine_config::StateMachineTransition;
use sonoro_gamesv_data::tables::blueprint_config::{BlueprintConfig, EntityLogic};
use sonoro_gamesv_data::tables::buff::buff_list_from_ids;
use sonoro_gamesv_data::tables::entity_components::base_info::BaseInfoComponent;
use sonoro_gamesv_data::tables::entity_components::components::{
    Components, gravity_flip_from_default,
};
use sonoro_gamesv_data::tables::entity_components::extra::{ExtraEntry, ExtraKind};
use sonoro_gamesv_data::tables::gameplay_tag_remap::GameplayTagRemap;
use sonoro_gamesv_data::tables::level_entity_config::LevelEntityConfig;
use sonoro_gamesv_data::tables::template_config::TemplateConfig;
use sonoro_gamesv_fs::progress_info::ProgressInfo;
use sonoro_proto::enums::{EAttributeType, GameCtxType, VehicleSource};
use sonoro_proto::pb::{
    BehaviorTreeCtxPb, DynamicInteractInfo, EntityAddNotify, EntityComponentPb,
    EntityComponentPbComponentPbOneof, EntityCtxPb, EntityPb, GameCtxPb, GameCtxPbCtxInfoOneof,
    InteractComponentPb, JSPatchNotify,
};

use crate::commands::respond;
use crate::component::entity::fsm::FsmComponent;
use crate::component::entity::interact_state::InteractStateComponent;
use crate::component::entity::small::{
    CharacterAttachComponent, ConfigComponent, ConfigType, EntityState, EntityType, LiftComponent,
    MonsterAiComponent, PassiveGaSkillComponent, PositionComponent, SceneItemComponent,
    StateTagComponent, VehicleComponent,
};
use crate::component::entity::{
    EntityComponentStorage, FightBuffComponent, LogicStateComponent, PartComponent, TagComponent,
};
use crate::component::player::{
    PlayerBasicComponent, PlayerEchoComponent, PlayerRoleComponent, PlayerWeaponComponent,
};
use crate::event_queue::EventQueue;
use crate::events::EntityHandle;
use crate::helpers::{entity_attributes, gameplay_tags, role as role_helper};
use crate::scene::{Scene, now_millis};
use crate::schedulers::BuffTimerScheduler;
use crate::sink::PushSink;

pub const ALIAS: &str = "s";
pub const DESCRIPTION: &str =
    "spawns an entity.\nusage: spawn [entity_id] [freeze?] [tune_break?]";

#[derive(Debug, Clone, Copy)]
struct EntitySpawnBase {
    config_type: ConfigType,
    entity_type: EntityType,
    entity_state: EntityState,
}

const FROZEN_BUFFS: &[i64] = &[10010001];
const TUNE_BROKEN_BUFFS: &[i64] = &[3161, 3162, 3163, 3164, 9000000000];

/// Compute the world position of an elevator cabin on `floor` (1-indexed) from
/// its base `InitLocation` (`base_pos`) and `LiftComponent.StayPositions`, matching
/// `ElevatorNormalMoveStrategy.InitFromConfig` (`HoverPoints[i] = init + StayPositions[i]`).
pub fn resolve_lift_floor_pos(base_pos: [f32; 3], stay_positions: &Value, floor: i32) -> [f32; 3] {
    if floor < 1 {
        return base_pos;
    }
    let Some(entry) = stay_positions
        .as_array()
        .and_then(|arr| arr.get((floor - 1) as usize))
        .and_then(|v| v.as_object())
    else {
        return base_pos;
    };
    let dx = entry.get("X").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let dy = entry.get("Y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let dz = entry.get("Z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    [base_pos[0] + dx, base_pos[1] + dy, base_pos[2] + dz]
}

/// Resolve `StayPositions` with a template fallback because generated `MergeFields`
/// for `LiftComponent` omits `StayPositions`.
pub fn resolve_lift_stay_positions<'a>(
    assets: &Assets,
    blueprint_type: &str,
    lc: &'a sonoro_gamesv_data::tables::entity_components::lift::LiftComponent,
) -> Cow<'a, Value> {
    if lc
        .StayPositions
        .as_array()
        .is_some_and(|arr| !arr.is_empty())
    {
        return Cow::Borrowed(&lc.StayPositions);
    }
    match assets
        .find_template_config(blueprint_type)
        .and_then(|tpl| {
            tpl.ComponentsData
                .LiftComponent
                .as_ref()
                .map(|tlc| tlc.StayPositions.clone())
        }) {
        Some(v) => Cow::Owned(v),
        None => Cow::Borrowed(&lc.StayPositions),
    }
}

/// Compute the world position of lift `cfg_id` on `floor` given its base `InitPos`.
pub fn lift_floor_pos_for_entity(
    assets: &Assets,
    map_id: i32,
    cfg_id: i32,
    base_pos: [f32; 3],
    floor: i32,
) -> [f32; 3] {
    let Some(lcfg) = assets.find_level_entity(map_id, i64::from(cfg_id)) else {
        return base_pos;
    };
    if let Some(lc) = &lcfg.ComponentsData.LiftComponent {
        let stay = resolve_lift_stay_positions(assets, &lcfg.BlueprintType, lc);
        return resolve_lift_floor_pos(base_pos, stay.as_ref(), floor);
    }
    if let Some(tpl) = assets.find_template_config(&lcfg.BlueprintType)
        && let Some(lc) = &tpl.ComponentsData.LiftComponent
    {
        return resolve_lift_floor_pos(base_pos, &lc.StayPositions, floor);
    }
    base_pos
}
const REPEAT_BOSS_TAG_ID: i32 = 1639442014;
const STORY_BOSS_TAG_ID: i32 = -1276461747;

struct SpawnConfig {
    entity_config: LevelEntityConfig,
    blueprint_config: BlueprintConfig,
    template_config: TemplateConfig,
    score: i32,
}

fn spawn_base_from_logic(entity_logic: EntityLogic) -> Option<EntitySpawnBase> {
    match entity_logic {
        EntityLogic::Item => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::SceneItem,
            entity_state: EntityState::Default,
        }),
        EntityLogic::Animal => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::Animal,
            entity_state: EntityState::Default,
        }),
        EntityLogic::Monster => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::Monster,
            entity_state: EntityState::Born,
        }),
        EntityLogic::Vehicle => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::Vehicle,
            entity_state: EntityState::Default,
        }),
        EntityLogic::Npc => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::Npc,
            entity_state: EntityState::Default,
        }),
        EntityLogic::Vision => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::Vision,
            entity_state: EntityState::Default,
        }),
        EntityLogic::Custom => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::Custom,
            entity_state: EntityState::Default,
        }),
        EntityLogic::SimpleCombat => Some(EntitySpawnBase {
            config_type: ConfigType::Level,
            entity_type: EntityType::Monster,
            entity_state: EntityState::Default,
        }),
        EntityLogic::ClientOnly | EntityLogic::ServerOnly | EntityLogic::Unknown => None,
    }
}

fn build_monster_passive_ga_skill(assets: &Assets, model_id: i32) -> Option<PassiveGaSkillComponent> {
    if model_id <= 0 {
        return None;
    }
    let model = assets.tables.model_config_preload.get_data_by_id(&model_id)?;
    if model.ActorClassPath.is_empty() {
        return None;
    }

    let mut comp = PassiveGaSkillComponent::default();
    if let Some(db) = assets.tables.assets_db() {
        if let Ok(skill_ids) = db.query_skill_ids_by_actor_blueprint(&model.ActorClassPath) {
            for sid in skill_ids {
                comp.add_skill(sid);
            }
        }
    } else {
        for entry in &assets.tables.entity_skill_preload.items {
            if entry.ActorBlueprint == model.ActorClassPath {
                comp.add_skill(entry.SkillId);
            }
        }
    }
    if comp.skill_component_pb.is_empty() {
        None
    } else {
        Some(comp)
    }
}

fn use_ai_runtime(entity_type: EntityType) -> bool {
    matches!(
        entity_type,
        EntityType::Animal | EntityType::Monster | EntityType::Npc | EntityType::Vision
    )
}

fn has_usable_fsm(assets: &Assets, components: &Components) -> bool {
    components
        .AiComponent
        .as_ref()
        .and_then(|ai| ai.AiId)
        .is_some_and(|aid| crate::fsm::config_loader::has_usable_ai_base_id(assets, aid))
}

fn transition_has_tag(transition: &StateMachineTransition, tag_id: i32) -> bool {
    transition
        .Conditions
        .iter()
        .any(|c| c.CondTag.as_ref().is_some_and(|t| t.TagId == Some(tag_id)))
}

fn use_repeat_boss_startup(ai_id: Option<i32>, assets: &Assets) -> bool {
    let Some(id) = ai_id else {
        return false;
    };
    let Some(ai_base) = assets.tables.ai_base.get_data_by_id(&id) else {
        return false;
    };
    let Some(config) = assets
        .tables
        .ai_state_machine_config
        .get_data_by_id(&ai_base.StateMachine)
    else {
        return false;
    };

    for node in &config.StateMachineJson.Nodes {
        for repeat_transition in &node.Transitions {
            if !transition_has_tag(repeat_transition, REPEAT_BOSS_TAG_ID) {
                continue;
            }
            for story_transition in &node.Transitions {
                if story_transition.From == repeat_transition.From
                    && transition_has_tag(story_transition, STORY_BOSS_TAG_ID)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn has_combat_attributes(components: &Components) -> bool {
    components
        .AttributeComponent
        .as_ref()
        .is_some_and(|attr| !attr.Disabled.unwrap_or(false))
}

fn initial_tag_component(
    components: &Components,
    tag_remaps: &Table<GameplayTagRemap, i32>,
    repeat_boss_startup: bool,
) -> TagComponent {
    let mut result = TagComponent::default();
    let Some(monster) = &components.MonsterComponent else {
        return result;
    };
    let has_combat_attrs = has_combat_attributes(components);
    if let Some(startup_tags) = &monster.InitGasTag {
        for tag_name in startup_tags {
            if has_combat_attrs && tag_name == "功能.逻辑状态标识.隐藏状态条" {
                continue;
            }
            let Some(mut tag_id) = gameplay_tags::id_from_name(tag_remaps, tag_name) else {
                continue;
            };
            if tag_id == 0 {
                continue;
            }
            if repeat_boss_startup && tag_id == STORY_BOSS_TAG_ID {
                tag_id = REPEAT_BOSS_TAG_ID;
            }
            result.add_tag(tag_id, 1);
        }
    }

    if has_combat_attrs {
        if let Some(fight_tag_id) =
            gameplay_tags::id_from_name(tag_remaps, "行为状态.逻辑状态.进入战斗")
            && fight_tag_id != 0
        {
            result.add_tag(fight_tag_id, 1);
            if !result.entity_common_tags.contains(&fight_tag_id) {
                result.entity_common_tags.push(fight_tag_id);
            }
        }
    }
    result
}

fn spawn_candidate_score(
    assets: &Assets,
    scene_map_id: i32,
    entity_config: &LevelEntityConfig,
    blueprint_config: &BlueprintConfig,
) -> i32 {
    let mut score = 0;
    if entity_config.MapId == scene_map_id {
        score += 10000;
    }
    if let Some(base) = spawn_base_from_logic(blueprint_config.EntityLogic) {
        if use_ai_runtime(base.entity_type) {
            score += 1000;
        }
        if base.entity_type == EntityType::Monster {
            score += 3000;
        }
    }
    if has_usable_fsm(assets, &entity_config.ComponentsData) {
        score += 2500;
    }
    if has_combat_attributes(&entity_config.ComponentsData) {
        score += 1500;
    }
    if !entity_config.IsHidden {
        score += 100;
    }
    if !entity_config.InSleep {
        score += 50;
    }
    score
}

fn select_spawn_config(
    assets: &Assets,
    entity_id: i64,
    scene_map_id: i32,
    source_map_id: Option<i32>,
) -> Option<SpawnConfig> {
    let mut best: Option<SpawnConfig> = None;
    for &idx in assets.indexes.level_entity_indices_by_id(entity_id) {
        let Some(cfg) = assets.get_level_entity_by_idx(idx) else {
            continue;
        };
        if let Some(map_id) = source_map_id
            && cfg.MapId != map_id
        {
            continue;
        }
        let Some(blueprint_config) = assets.find_blueprint_config(&cfg.BlueprintType) else {
            continue;
        };
        let Some(template_config) = assets.find_template_config(&cfg.BlueprintType) else {
            continue;
        };
        let mut entity_config = cfg.into_owned();
        template_config
            .ComponentsData
            .merge_into(&mut entity_config.ComponentsData);

        let score = spawn_candidate_score(
            assets,
            source_map_id.unwrap_or(scene_map_id),
            &entity_config,
            blueprint_config,
        );
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(SpawnConfig {
                entity_config,
                blueprint_config: blueprint_config.clone(),
                template_config: template_config.into_owned(),
                score,
            });
        }
    }
    best
}

fn spawn_camp(
    base_info: &BaseInfoComponent,
    template_config: &TemplateConfig,
    spawn_base: EntitySpawnBase,
) -> i32 {
    let base_camp = base_info.Camp;
    let tpl_camp = template_config
        .ComponentsData
        .BaseInfoComponent
        .as_ref()
        .and_then(|b| b.Camp);
    if spawn_base.entity_type == EntityType::Monster
        && let Some(camp) = tpl_camp
        && matches!(base_camp.unwrap_or(0), 0 | 2)
    {
        return camp;
    }
    base_camp.or(tpl_camp).unwrap_or(0)
}

fn gravity_for_spawn(base_info: &BaseInfoComponent, rotation: [f32; 3]) -> [f32; 3] {
    let Some(gc) = &base_info.GravityConfig else {
        return [0.0, 0.0, -1.0];
    };
    let Some(dir) = &gc.GravityDirection else {
        return [0.0, 0.0, -1.0];
    };
    if let Some(vec) = &dir.Direction {
        return [vec.X, vec.Y, vec.Z];
    }
    if let Some(axis) = &dir.WorldAxis {
        return match axis.as_str() {
            "PositiveX" => [1.0, 0.0, 0.0],
            "NegativeX" => [-1.0, 0.0, 0.0],
            "PositiveY" => [0.0, 1.0, 0.0],
            "NegativeY" => [0.0, -1.0, 0.0],
            "PositiveZ" => [0.0, 0.0, 1.0],
            "NegativeZ" => [0.0, 0.0, -1.0],
            _ => [0.0, 0.0, -1.0],
        };
    }
    if dir.Type.as_deref() == Some("SelfRotation") {
        let to_rad = std::f32::consts::PI / 180.0;
        let (sr, cr) = (rotation[0] * to_rad).sin_cos();
        let (sp, cp) = (rotation[1] * to_rad).sin_cos();
        let (sy, cy) = (rotation[2] * to_rad).sin_cos();
        return [cp * sr * cy + sp * sy, cp * sr * sy - sp * cy, -cp * cr];
    }
    [0.0, 0.0, -1.0]
}

fn append_configured_buff_ids(buff_ids: &mut Vec<i64>, components: &Components) {
    let Some(attr) = &components.AttributeComponent else {
        return;
    };
    if let Some(configured_ids) = &attr.AppendBuffIds {
        buff_ids.extend_from_slice(configured_ids);
    }
}

pub fn attach_dynamic_interact_infos(scene: &Scene, entity_pb: &mut EntityPb) {
    let net_id = entity_pb.Id;
    let mut infos = Vec::new();
    for opt in &scene.dynamic_interact_options {
        if opt.entity_net_id != net_id || opt.option_guid.is_empty() {
            continue;
        }
        let game_ctx = if opt.quest_id > 0 {
            GameCtxPb {
                CtxType: Some(GameCtxType::BehaviorTree),
                CtxInfo: Some(GameCtxPbCtxInfoOneof::BehaviorTree(BehaviorTreeCtxPb {
                    IncId: i64::from(opt.quest_id),
                    BtType: Some(opt.bt_type),
                    BtId: opt.quest_id,
                    NodeId: opt.node_id,
                })),
            }
        } else {
            GameCtxPb {
                CtxType: Some(GameCtxType::Entity),
                CtxInfo: Some(GameCtxPbCtxInfoOneof::Entity(EntityCtxPb {
                    ConfigId: opt.entity_config_id,
                    IncId: opt.entity_net_id,
                })),
            }
        };
        infos.push(DynamicInteractInfo {
            OptionGuid: opt.option_guid.as_bytes().to_vec(),
            GameCtx: Some(game_ctx),
            Text: opt.text.as_bytes().to_vec(),
            DelayRemove: false,
        });
    }
    if infos.is_empty() {
        return;
    }
    for comp in &mut entity_pb.ComponentPbs {
        if let Some(EntityComponentPbComponentPbOneof::InteractComponent(ic)) = &mut comp.ComponentPb
        {
            for info in infos {
                if !ic
                    .DynamicInteractInfos
                    .iter()
                    .any(|cur| cur.OptionGuid == info.OptionGuid)
                {
                    ic.DynamicInteractInfos.push(info);
                }
            }
            return;
        }
    }
    entity_pb.ComponentPbs.push(EntityComponentPb {
        ComponentPb: Some(EntityComponentPbComponentPbOneof::InteractComponent(
            InteractComponentPb {
                DynamicInteractInfos: infos,
                ..Default::default()
            },
        )),
    });
}

#[allow(clippy::too_many_arguments)]
fn prepare_debug_spawn<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    _sink: &mut S,
    entity: EntityHandle,
    config_id: i64,
    source_map_id: i32,
    repeat_boss: bool,
    components: &Components,
    is_frozen: bool,
    is_tune_broken: bool,
) -> Result<(EntityPb, String), &'static str> {
    scene.register_debug_spawn_route(entity.net_id, config_id, source_map_id, repeat_boss)?;

    let mut buff_ids = Vec::new();
    append_configured_buff_ids(&mut buff_ids, components);
    if is_tune_broken {
        buff_ids.extend_from_slice(TUNE_BROKEN_BUFFS);
    }
    if is_frozen {
        buff_ids.extend_from_slice(FROZEN_BUFFS);
    }
    if !buff_ids.is_empty() {
        let born_buffs: Vec<crate::events::BuffAdditionEntry> = buff_list_from_ids(&buff_ids)
            .into_iter()
            .map(|b| crate::events::BuffAdditionEntry {
                id: b.id,
                is_active: b.is_active,
                stack_count: b.stack_count,
                ..Default::default()
            })
            .collect();
        let buff_infos = scene.build_fight_buff_infos(&born_buffs, entity.net_id);
        if let Some(storage) = scene.entity_mut(entity.net_id)
            && let Some(buffs) = storage.buffs.as_mut()
        {
            buffs.buffs = buff_infos;
        }
        let _ = scene.save_entity(fs, entity);
    }

    let now_ms = now_millis();
    let mut entity_pb = scene
        .entity(entity.net_id)
        .ok_or("EntityNotFound")?
        .to_proto(assets, now_ms);
    attach_dynamic_interact_infos(scene, &mut entity_pb);
    let patch_js = format!(
        "globalThis.__akiSetEntitySourceMap?.({config_id},{source_map_id},{},{});",
        if repeat_boss { "true" } else { "false" },
        entity.net_id
    );
    Ok((entity_pb, patch_js))
}

fn publish_debug_spawn<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    entity: EntityHandle,
    config_id: i64,
    source_map_id: i32,
    repeat_boss: bool,
    components: &Components,
    is_frozen: bool,
    is_tune_broken: bool,
) -> Result<(), &'static str> {
    let (entity_pb, patch_js) = prepare_debug_spawn(
        scene,
        fs,
        assets,
        sink,
        entity,
        config_id,
        source_map_id,
        repeat_boss,
        components,
        is_frozen,
        is_tune_broken,
    )?;

    sink.push(
        JSPatchNotify::MSG_ID,
        &JSPatchNotify {
            Content: patch_js.into(),
        },
    );
    sink.push(
        EntityAddNotify::MSG_ID,
        &EntityAddNotify {
            EntityPbs: vec![entity_pb],
            RemoveTagIds: false,
        },
    );
    Ok(())
}

fn parse_optional_int(value: Option<&Value>) -> i32 {
    let Some(raw) = value else {
        return 0;
    };
    match raw {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i32::try_from(i).unwrap_or(0)
            } else if let Some(f) = n.as_f64()
                && f.is_finite()
            {
                f.clamp(-2147483648.0, 2147483647.0) as i32
            } else {
                0
            }
        }
        Value::String(s) => s.parse::<i32>().unwrap_or(0),
        _ => 0,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn call<S: PushSink + ?Sized>(
    events: &mut EventQueue,
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    basic_comp: &PlayerBasicComponent,
    entity_id: i64,
    is_frozen: Option<bool>,
    is_tune_broken: Option<bool>,
) -> Result<(), &'static str> {
    spawn_with_location(
        events,
        scene,
        fs,
        assets,
        sink,
        basic_comp,
        entity_id,
        is_frozen,
        is_tune_broken,
        None,
        None,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_with_location<S: PushSink + ?Sized>(
    events: &mut EventQueue,
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    basic_comp: &PlayerBasicComponent,
    entity_id: i64,
    is_frozen: Option<bool>,
    is_tune_broken: Option<bool>,
    source_map_id: Option<i32>,
    location_override: Option<[f32; 3]>,
    silent: bool,
) -> Result<(), &'static str> {
    let _ = spawn_with_location_internal(
        events,
        scene,
        fs,
        assets,
        sink,
        basic_comp,
        entity_id,
        is_frozen,
        is_tune_broken,
        source_map_id,
        location_override,
        silent,
        None,
        false,
        false,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_with_location_and_progress<S: PushSink + ?Sized>(
    events: &mut EventQueue,
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    basic_comp: &PlayerBasicComponent,
    entity_id: i64,
    is_frozen: Option<bool>,
    is_tune_broken: Option<bool>,
    source_map_id: Option<i32>,
    location_override: Option<[f32; 3]>,
    silent: bool,
    progress_info: &ProgressInfo,
    respect_sleep_hidden: bool,
    defer_publish: bool,
) -> Result<Option<EntityHandle>, &'static str> {
    spawn_with_location_internal(
        events,
        scene,
        fs,
        assets,
        sink,
        basic_comp,
        entity_id,
        is_frozen,
        is_tune_broken,
        source_map_id,
        location_override,
        silent,
        Some(progress_info),
        respect_sleep_hidden,
        defer_publish,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_synthetic_template_entity<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    basic_comp: &PlayerBasicComponent,
    blueprint_type: &str,
    config_id: i64,
    location: [f32; 3],
    rotation: [f32; 3],
) -> Option<i64> {
    let blueprint_config = assets.find_blueprint_config(blueprint_type)?;
    let template_config = assets.find_template_config(blueprint_type)?;
    let spawn_base = spawn_base_from_logic(blueprint_config.EntityLogic)?;
    let components_data = &template_config.ComponentsData;
    let base_info = components_data
        .BaseInfoComponent
        .as_ref()
        .map(Cow::Borrowed)
        .unwrap_or_else(|| Cow::Owned(BaseInfoComponent::default()));
    let final_camp = spawn_camp(&base_info, &template_config, spawn_base);
    let model_id = components_data
        .ModelComponent
        .as_ref()
        .map_or(0, |m| m.ModelType.ModelId);
    let ai_runtime = use_ai_runtime(spawn_base.entity_type);
    let mut storage = EntityComponentStorage::new(
        0,
        ConfigComponent {
            camp: final_camp,
            config_id: i32::try_from(config_id).unwrap_or(0),
            config_type: spawn_base.config_type,
            entity_type: spawn_base.entity_type,
            state: spawn_base.entity_state,
        },
    );
    storage.position = Some(PositionComponent { location, rotation });
    storage.buffs = Some(FightBuffComponent::default());
    if ai_runtime {
        let ai_id = components_data.AiComponent.as_ref().and_then(|c| c.AiId);
        storage.fsm =
            ai_id.and_then(|aid| FsmComponent::from_ai_base_id(assets, aid, now_millis()).ok().flatten());
        storage.attribute = Some(
            entity_attributes::create_combat_attributes(
                assets,
                components_data,
                blueprint_config.EntityLogic,
                1,
                basic_comp.info.attributes.cur_world_level,
            )
            .unwrap_or_default(),
        );
        storage.tag = Some(initial_tag_component(
            components_data,
            &assets.tables.gameplay_tag_remap,
            false,
        ));
        storage.logic_state = Some(LogicStateComponent::default());
        storage.character_attach = Some(CharacterAttachComponent::default());
        if let Some(aid) = ai_id
            && let Some(ab) = assets.tables.ai_base.get_data_by_id(&aid)
        {
            let mut hatred_id = 0i64;
            if let Some(sbc) = &ab.SubBehaviorConfigs
                && let Some(hate_str) = &sbc.AiHate
            {
                hatred_id = hate_str.parse::<i64>().unwrap_or(0);
            }
            storage.monster_ai = Some(MonsterAiComponent {
                weapon_id: 0,
                hatred_group_id: 0,
                ai_team_init_id: 100,
                combat_message_id: 1,
                hatred_id,
                perception_ids: [0i32; 8],
                perception_len: 0,
            });
        }
        if spawn_base.entity_type == EntityType::Monster {
            storage.passive_ga_skill = build_monster_passive_ga_skill(assets, model_id);
        }
    }
    if spawn_base.entity_type == EntityType::Vehicle
        || components_data.has_extra(ExtraKind::VehicleComponent)
    {
        storage.vehicle = Some(VehicleComponent {
            source: Some(VehicleSource::VehicleSourceNone),
            occupied: false,
        });
    }
    if spawn_base.entity_type == EntityType::SceneItem {
        let state_tag_id = gameplay_tags::id_from_name(
            &assets.tables.gameplay_tag_remap,
            "关卡.Common.状态.常态",
        )
        .unwrap_or(0);
        storage.state_tag = Some(StateTagComponent { state_tag_id });
        storage.world_item.scene_item = Some(SceneItemComponent::from_progress(
            &ProgressInfo::default(),
            scene.player_id,
            storage.config.config_id,
        ));
    }

    let handle = scene.spawn(storage);
    let _ = scene.save_entity(fs, handle);
    let now_ms = now_millis();
    let mut entity_pb = scene.entity(handle.net_id)?.to_proto(assets, now_ms);
    attach_dynamic_interact_infos(scene, &mut entity_pb);
    sink.push(
        EntityAddNotify::MSG_ID,
        &EntityAddNotify {
            EntityPbs: vec![entity_pb],
            RemoveTagIds: false,
        },
    );
    Some(handle.net_id)
}

#[allow(clippy::too_many_arguments)]
const WORLD_SPAWN_BATCH_SIZE: usize = 25;

pub fn publish_world_spawn_batch<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    items: &[(EntityHandle, i64, i32)],
) -> Vec<EntityHandle> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut prepared: Vec<(EntityHandle, EntityPb, String)> = Vec::with_capacity(items.len());
    for &(entity, entity_id, source_map_id) in items {
        let Some(selected) =
            select_spawn_config(assets, entity_id, scene.instance_id, Some(source_map_id))
        else {
            prepared.push((entity, EntityPb::default(), String::new()));
            continue;
        };
        let ai_id = selected
            .entity_config
            .ComponentsData
            .AiComponent
            .as_ref()
            .and_then(|c| c.AiId);
        let repeat_boss = use_repeat_boss_startup(ai_id, assets);
        match prepare_debug_spawn(
            scene,
            fs,
            assets,
            sink,
            entity,
            entity_id,
            selected.entity_config.MapId,
            repeat_boss,
            &selected.entity_config.ComponentsData,
            false,
            false,
        ) {
            Ok((pb, js)) => prepared.push((entity, pb, js)),
            Err(_) => {
                sink.push(
                    JSPatchNotify::MSG_ID,
                    &JSPatchNotify {
                        Content: format!(
                            "globalThis.__akiRemoveEntitySourceOwner?.({entity_id},{});",
                            entity.net_id
                        )
                        .into(),
                    },
                );
                scene.rollback_spawn(entity.net_id);
            }
        }
    }

    let mut published = Vec::with_capacity(prepared.len());
    for chunk in prepared.chunks(WORLD_SPAWN_BATCH_SIZE) {
        let mut js_batch = String::new();
        let mut pbs = Vec::with_capacity(chunk.len());
        for (handle, pb, js) in chunk {
            published.push(*handle);
            if pb.Id == 0 && js.is_empty() {
                continue;
            }
            if !js.is_empty() {
                if !js_batch.is_empty() {
                    js_batch.push('\n');
                }
                js_batch.push_str(js);
            }
            pbs.push(pb.clone());
        }
        if !js_batch.is_empty() {
            sink.push(
                JSPatchNotify::MSG_ID,
                &JSPatchNotify {
                    Content: js_batch.into(),
                },
            );
        }
        if !pbs.is_empty() {
            sink.push(
                EntityAddNotify::MSG_ID,
                &EntityAddNotify {
                    EntityPbs: pbs,
                    RemoveTagIds: false,
                },
            );
        }
    }
    published
}

pub fn publish_world_spawn<S: PushSink + ?Sized>(
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    entity: EntityHandle,
    entity_id: i64,
    source_map_id: i32,
) -> Result<(), &'static str> {
    if publish_world_spawn_batch(scene, fs, assets, sink, &[(entity, entity_id, source_map_id)])
        .is_empty()
    {
        return Err("PublishWorldSpawnFailed");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn spawn_with_location_internal<S: PushSink + ?Sized>(
    events: &mut EventQueue,
    scene: &mut Scene,
    fs: &FileSystem,
    assets: &Assets,
    sink: &mut S,
    basic_comp: &PlayerBasicComponent,
    entity_id: i64,
    is_frozen: Option<bool>,
    is_tune_broken: Option<bool>,
    source_map_id: Option<i32>,
    location_override: Option<[f32; 3]>,
    silent: bool,
    progress_info: Option<&ProgressInfo>,
    respect_sleep_hidden: bool,
    defer_publish: bool,
) -> Result<Option<EntityHandle>, &'static str> {
    let Some(selected) = select_spawn_config(assets, entity_id, scene.instance_id, source_map_id)
    else {
        if !silent {
            if let Some(map_id) = source_map_id {
                respond(
                    events,
                    scene.player_id,
                    format!(
                        "{entity_id} couldn't be spawned, no LevelEntityConfig row for map {map_id}"
                    ),
                );
            } else {
                respond(
                    events,
                    scene.player_id,
                    format!(
                        "{entity_id} couldn't be spawned, couldn't find it in LevelEntityConfig"
                    ),
                );
            }
        }
        return Ok(None);
    };
    let entity_config = &selected.entity_config;
    let blueprint_config = &selected.blueprint_config;
    let template_config = &selected.template_config;

    let Some(spawn_base) = spawn_base_from_logic(blueprint_config.EntityLogic) else {
        if !silent {
            respond(
                events,
                scene.player_id,
                format!(
                    "{entity_id} couldn't be spawned, unhandled entity logic: {:?}",
                    blueprint_config.EntityLogic
                ),
            );
        }
        return Ok(None);
    };
    let components_data = &entity_config.ComponentsData;
    let base_info = components_data
        .BaseInfoComponent
        .as_ref()
        .map(Cow::Borrowed)
        .unwrap_or_else(|| Cow::Owned(BaseInfoComponent::default()));

    let ai_comp = components_data.AiComponent.as_ref();
    let ai_id = ai_comp.and_then(|c| c.AiId);
    let repeat_boss_startup = use_repeat_boss_startup(ai_id, assets);
    if let Some(route) = scene.debug_spawn_route(entity_id)
        && !route.matches(entity_config.MapId, repeat_boss_startup)
    {
        respond(
            events,
            scene.player_id,
            format!(
                "{entity_id} couldn't be spawned, active instances use map {} with repeat_boss={}",
                route.source_map_id, route.repeat_boss
            ),
        );
        return Ok(None);
    }

    let weapon_id = ai_comp.map_or(0, |c| parse_optional_int(c.WeaponId.as_ref()));
    let final_camp = spawn_camp(&base_info, template_config, spawn_base);
    let world_rot = asset_indexes::entity_world_rot(entity_config);
    let gravity = gravity_for_spawn(&base_info, world_rot);
    let model_id = components_data
        .ModelComponent
        .as_ref()
        .map_or(0, |m| m.ModelType.ModelId);

    let ai_runtime = use_ai_runtime(spawn_base.entity_type);
    let mut fsm_component = None;
    let mut attribute_component = None;
    let mut tag_component = None;
    let mut part_component = None;
    let mut state_tag_component = None;
    let mut passive_ga_skill_component = None;
    let mut monster_ai_component = None;

    if ai_runtime {
        fsm_component =
            ai_id.and_then(|aid| FsmComponent::from_ai_base_id(assets, aid, now_millis()).ok().flatten());
        attribute_component = Some(
            entity_attributes::create_combat_attributes(
                assets,
                components_data,
                blueprint_config.EntityLogic,
                entity_config.AreaId,
                basic_comp.info.attributes.cur_world_level,
            )
            .unwrap_or_default(),
        );
        tag_component = Some(initial_tag_component(
            components_data,
            &assets.tables.gameplay_tag_remap,
            repeat_boss_startup,
        ));
        let life_max = attribute_component
            .as_ref()
            .map_or(0, |a| a.get_current(EAttributeType::LifeMax));
        if let Some(part_cfg) = assets.tables.character_part_config.get_data_by_id(&model_id) {
            part_component = Some(PartComponent::from_config(&part_cfg, life_max));
        }
    }

    if spawn_base.entity_type == EntityType::SceneItem || components_data.EntityStateComponent.is_some()
    {
        let default_state: Option<&str> = if let Some(es) = &components_data.EntityStateComponent {
            es.State.as_deref().or(if spawn_base.entity_type == EntityType::SceneItem {
                Some("关卡.Common.状态.常态")
            } else {
                None
            })
        } else if spawn_base.entity_type == EntityType::SceneItem {
            Some("关卡.Common.状态.常态")
        } else {
            None
        };
        let state_tag_id = default_state
            .and_then(|s| gameplay_tags::id_from_name(&assets.tables.gameplay_tag_remap, s))
            .unwrap_or(0);
        state_tag_component = Some(StateTagComponent { state_tag_id });
        if tag_component.is_none() {
            tag_component = Some(initial_tag_component(
                components_data,
                &assets.tables.gameplay_tag_remap,
                repeat_boss_startup,
            ));
        }
        if state_tag_id != 0
            && let Some(tags) = tag_component.as_mut()
        {
            tags.add_tag(state_tag_id, 1);
            if !tags.entity_common_tags.contains(&state_tag_id) {
                tags.entity_common_tags.push(state_tag_id);
            }
        }
    }

    let interact_state_component = components_data
        .InteractComponent
        .as_ref()
        .filter(|ic| !ic.Disabled.unwrap_or(false))
        .map(|ic| {
            InteractStateComponent::from_config(
                ic,
                scene.player_id,
                scene.instance_id,
                entity_id as i32,
            )
        });

    if let Some(part) = part_component.as_ref() {
        part.sync_active_tags(tag_component.as_mut());
    }

    let initially_hidden =
        respect_sleep_hidden && (entity_config.IsHidden || entity_config.InSleep);
    if spawn_base.entity_type == EntityType::Monster {
        passive_ga_skill_component = build_monster_passive_ga_skill(assets, model_id);
    }

    if ai_runtime {
        let mut hatred_id = 0i64;
        let mut perception_ids = [0i32; 8];
        let mut perception_len = 0u8;
        if let Some(aid) = ai_id
            && let Some(ab) = assets.tables.ai_base.get_data_by_id(&aid)
            && let Some(sbc) = &ab.SubBehaviorConfigs
        {
            if let Some(hate_str) = &sbc.AiHate {
                hatred_id = hate_str.parse::<i64>().unwrap_or(0);
            }
            if let Some(sense_str) = &sbc.AiSense
                && let Ok(sg_id) = sense_str.parse::<i32>()
                && sg_id > 0
                && let Some(sg) = assets.tables.ai_sense_group.get_data_by_id(&sg_id)
            {
                for &sid in &sg.AiSenseIds {
                    if usize::from(perception_len) >= perception_ids.len() {
                        break;
                    }
                    if sid != 0 {
                        perception_ids[usize::from(perception_len)] = sid;
                        perception_len += 1;
                    }
                }
            }
        }
        monster_ai_component = Some(MonsterAiComponent {
            weapon_id,
            hatred_group_id: 0,
            ai_team_init_id: 100,
            combat_message_id: 1,
            hatred_id,
            perception_ids,
            perception_len,
        });
    }

    let (default_loc, default_rot) = scene
        .instance
        .players
        .first()
        .map_or(([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]), |p| {
            (p.location, p.rotation)
        });
    let location = location_override.unwrap_or(default_loc);
    let rotation = if location_override.is_some() {
        world_rot
    } else {
        default_rot
    };

    let mut storage = EntityComponentStorage::new(
        0,
        ConfigComponent {
            camp: final_camp,
            config_id: entity_id as i32,
            config_type: spawn_base.config_type,
            entity_type: spawn_base.entity_type,
            state: spawn_base.entity_state,
        },
    );
    storage.visible = !initially_hidden;
    storage.actor_visible = !initially_hidden;
    storage.position = Some(PositionComponent { location, rotation });
    storage.buffs = Some(FightBuffComponent::default());
    storage.attribute = attribute_component;
    if ai_runtime {
        storage.logic_state = Some(LogicStateComponent::default());
        storage.character_attach = Some(CharacterAttachComponent::default());
    }
    storage.monster_ai = monster_ai_component;
    if spawn_base.entity_type == EntityType::Vehicle
        || components_data.has_extra(ExtraKind::VehicleComponent)
    {
        storage.vehicle = Some(VehicleComponent {
            source: Some(VehicleSource::VehicleSourceNone),
            occupied: false,
        });
    }
    storage.tag = tag_component;
    storage.part = part_component;
    storage.fsm = fsm_component;
    storage.state_tag = state_tag_component;
    storage.passive_ga_skill = passive_ga_skill_component;

    if base_info.GravityConfig.is_some() {
        storage.world_item.gravity = Some(crate::component::entity::small::GravityComponent {
            location: gravity,
        });
    }
    if let Some(gfc) = &components_data.GravityFlipComponent
        && let Some(dg) = gfc.DefaultGravity
    {
        storage.world_item.gravity_flip = gravity_flip_from_default(dg);
    }
    if components_data.TeleControl2.is_some() {
        storage.world_item.be_controlled_present = true;
    }
    if matches!(
        blueprint_config.EntityType,
        sonoro_gamesv_data::tables::blueprint_config::EntityType::Drop
            | sonoro_gamesv_data::tables::blueprint_config::EntityType::DropItem
    ) {
        storage.world_item.drop_present = true;
    }
    if spawn_base.entity_type == EntityType::Npc {
        if let Some(nt) = base_info.Category.as_ref().and_then(|c| c.NpcType) {
            storage.world_item.sub_entity_type = nt;
        } else if blueprint_config.EntityType
            == sonoro_gamesv_data::tables::blueprint_config::EntityType::PasserbyNpc
        {
            storage.world_item.sub_entity_type = 2;
        } else if matches!(
            entity_config.BlueprintType.as_str(),
            "SimpleNpc" | "SimpleNPc"
        ) {
            storage.world_item.sub_entity_type = 1;
        }
    }
    if components_data.RangeComponent.is_some() {
        storage.world_item.range_present = true;
    }
    if components_data.TriggerComponent.is_some() {
        storage.world_item.trigger_present = true;
    }
    if let Some(dc) = &components_data.DestructibleItem
        && !dc.Disabled.unwrap_or(false)
    {
        let saved_durability = progress_info
            .map(|info| info.get_key_int(&format!("durability:{entity_id}")))
            .unwrap_or(0);
        storage.world_item.durability =
            crate::helpers::destructible::initial_durability(dc.Durability, saved_durability);
    }
    if let Some(lc) = &components_data.LiftComponent
        && !lc.Disabled.unwrap_or(false)
    {
        let floor = progress_info
            .and_then(|info| {
                let v = info.get_key_int(&format!("lift_target:{entity_id}"));
                if v > 0 {
                    i32::try_from(v).ok()
                } else {
                    let e = info.get_key_int(&format!("elevator:{entity_id}"));
                    (e > 0).then(|| i32::try_from(e).ok()).flatten()
                }
            })
            .unwrap_or_else(|| lc.InitialFloor.unwrap_or(1));
        let stay_positions =
            resolve_lift_stay_positions(assets, &entity_config.BlueprintType, lc);
        storage.world_item.lift = Some(LiftComponent { location: floor });
        storage.world_item.lift_init_pos = Some(location);
        let floor_pos = resolve_lift_floor_pos(location, stay_positions.as_ref(), floor);
        storage.position = Some(PositionComponent {
            location: floor_pos,
            rotation,
        });
    }
    if let Some(aic) = &components_data.AiComponent {
        storage.world_item.patrol_dir = Some(false);
        if let Some(Value::Number(wid)) = &aic.WeaponId
            && let Some(w) = wid.as_i64()
            && w > 0
            && let Ok(w32) = i32::try_from(w)
        {
            storage.world_item.monster_weapon_id = Some(w32);
        }
    }
    for entry in components_data
        .extra
        .iter()
        .chain(components_data.template_extra.iter())
    {
        match entry {
            ExtraEntry::FanComponent(fc) => {
                storage.world_item.fan_turns =
                    Some(fc.InitCircle.and_then(|v| i32::try_from(v).ok()).unwrap_or(0));
            }
            ExtraEntry::HookLockPoint(hlp) => {
                storage.world_item.hook_disabled = Some(
                    hlp.DisableAtCreateStage.unwrap_or(false) || hlp.Disabled.unwrap_or(false),
                );
            }
            ExtraEntry::ConnectorComponent(_) => {
                storage.world_item.connector_initiated = Some(false);
            }
            ExtraEntry::ElementalHolderComponent(eh) => {
                storage.world_item.elemental_holder_enable = Some(!eh.Disabled.unwrap_or(false));
            }
            ExtraEntry::NearbyTrackingComponent(nt) => {
                storage.world_item.nearby_tracking_enable =
                    Some(nt.IsEnable.unwrap_or(!nt.Disabled.unwrap_or(false)));
            }
            ExtraEntry::FlowerPollutionComponent(_) => {
                storage.world_item.flower_pollution = true;
            }
            ExtraEntry::BuffProducerComponent(_) => {
                storage.world_item.buff_producer_present = true;
            }
            ExtraEntry::BuffConsumerComponent(_) => {
                storage.world_item.buff_consumer_present = true;
            }
            ExtraEntry::SceneItemEventListenerComponent(_) => {
                storage.world_item.listener_present = true;
            }
            ExtraEntry::BatchBulletCasterComponent(_) => {
                storage.world_item.batch_bullet_cast_present = true;
            }
            ExtraEntry::QuickHackComponent(_) => {
                storage.world_item.hacking_present = true;
            }
            ExtraEntry::HackManagementComponent(_) => {
                storage.world_item.hack_target_present = true;
            }
            ExtraEntry::PullingObjectFoundation(_) => {
                storage.world_item.pulling_foundation_present = true;
            }
            ExtraEntry::SunSpiritGearComponent(_) => {
                storage.world_item.spirit_gear_present = true;
            }
            _ => {}
        }
    }
    storage.world_item.interact = interact_state_component;
    if spawn_base.entity_type == EntityType::Npc
        && let Ok(cid32) = i32::try_from(entity_id)
    {
        storage.world_item.npc_spawn_id = cid32;
    }
    let cfg_id = storage.config.config_id;
    if spawn_base.entity_type == EntityType::SceneItem {
        let default_progress = ProgressInfo::default();
        storage.world_item.scene_item = Some(SceneItemComponent::from_progress(
            progress_info.unwrap_or(&default_progress),
            scene.player_id,
            cfg_id,
        ));
    }

    let handle = scene.spawn(storage);
    let _ = scene.save_entity(fs, handle);

    if !defer_publish
        && let Err(err) = publish_debug_spawn(
            scene,
            fs,
            assets,
            sink,
            handle,
            entity_id,
            entity_config.MapId,
            repeat_boss_startup,
            components_data,
            is_frozen.unwrap_or(false),
            is_tune_broken.unwrap_or(false),
        )
    {
        sink.push(
            JSPatchNotify::MSG_ID,
            &JSPatchNotify {
                Content: format!(
                    "globalThis.__akiRemoveEntitySourceOwner?.({entity_id},{});",
                    handle.net_id
                )
                .into(),
            },
        );
        scene.rollback_spawn(handle.net_id);
        return Err(err);
    }

    if !silent {
        respond(
            events,
            scene.player_id,
            format!(
                "spawned {entity_id}, map: {}, bp: {}, camp: {final_camp}, ai_id: {ai_id:?}",
                entity_config.MapId, entity_config.BlueprintType
            ),
        );
    }
    Ok(Some(handle))
}

pub mod spawn_map {
    use super::*;

    pub const ALIAS: &str = "sm";
    pub const DESCRIPTION: &str = "spawns an entity using an exact source map row.\nusage: spawn_map [entity_id] [source_map_id] [freeze?] [tune_break?]";

    #[allow(clippy::too_many_arguments)]
    pub fn call<S: PushSink + ?Sized>(
        events: &mut EventQueue,
        scene: &mut Scene,
        fs: &FileSystem,
        assets: &Assets,
        sink: &mut S,
        basic_comp: &PlayerBasicComponent,
        entity_id: i64,
        source_map_id: i32,
        is_frozen: Option<bool>,
        is_tune_broken: Option<bool>,
    ) -> Result<(), &'static str> {
        spawn_with_location(
            events,
            scene,
            fs,
            assets,
            sink,
            basic_comp,
            entity_id,
            is_frozen,
            is_tune_broken,
            Some(source_map_id),
            None,
            false,
        )
    }
}

pub mod reset_formation {
    use super::*;

    pub const ALIAS: &str = "rf";
    pub const DESCRIPTION: &str =
        "resets the current formation by respawning all role entities.\nusage: reset_formation";

    #[allow(clippy::too_many_arguments)]
    pub fn call<S: PushSink + ?Sized>(
        events: &mut EventQueue,
        scene: &mut Scene,
        fs: &FileSystem,
        assets: &Assets,
        role_comp: &mut PlayerRoleComponent,
        weapon_comp: &PlayerWeaponComponent,
        echo_comp: &PlayerEchoComponent,
        sink: &mut S,
        buff_timers: &mut BuffTimerScheduler,
        now_ms: i64,
    ) -> Result<(), &'static str> {
        role_helper::reset_roles(
            scene,
            fs,
            assets,
            role_comp,
            weapon_comp,
            echo_comp,
            sink,
            None,
            buff_timers,
            now_ms,
        )?;
        respond(events, scene.player_id, "reset formation successfully");
        Ok(())
    }
}
