pub mod attribute;
pub mod fight_buff;
pub mod fsm;
pub mod interact_state;
pub mod logic_state;
pub mod part;
pub mod small;
pub mod tag;

use crate::component::comp_util;
use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_fs::file_util::load_zon;
use sonoro_proto::enums::{DirectionType, EAttributeType};
use sonoro_proto::pb::{
    BatchBulletCastComponentPb, BeControlledComponentPb, BuffConsumerComponentPb,
    BuffProducerComponentPb, ConnectorComponentPb, DropComponentPb, ElementalHolderComponentPb,
    EntityComponentPb, EntityComponentPbComponentPbOneof, EntityFoundationComponentPb,
    EntityFoundationComponentPbEgMOneof, EntityPb, EntityPbD3sOneof, FanComponentPb,
    FlowerPollutionComponentPb, FoundationCoreInfo, GrapplingHookPointComponentPb,
    GravityFlipComponent, HackTargetComponentPb, HackingComponentPb, HoldHandComponentPb,
    MonsterWeaponComponentPb, NearbyTrackingComponentPb, NpcPb, PatrolComponentPb,
    PullingFoundationComponentPb, RangeComponentPb, SceneItemEventListenerComponentPb,
    ShieldComponentPb, ShieldInfoPb, SunSpiritGearComponentPb, TriggerComponentPb, Vector,
};
use sonoro_proto::wire::PbEnum;
use std::io;

pub use attribute::AttributeComponent;
pub use fight_buff::FightBuffComponent;
pub use fsm::FsmComponent;
pub use interact_state::InteractStateComponent;
pub use logic_state::LogicStateComponent;
pub use part::PartComponent;
pub use small::*;
pub use tag::TagComponent;

#[derive(Debug, Clone)]
pub struct WorldItemState {
    pub gravity: Option<GravityComponent>,
    pub lift: Option<LiftComponent>,
    pub lift_init_pos: Option<[f32; 3]>,
    pub move_spline: Option<MoveSplineComponent>,
    pub move_to_point: Option<MoveToPointComponent>,
    pub scene_item: Option<SceneItemComponent>,
    pub interact: Option<InteractStateComponent>,
    pub monster_capture: Option<MonsterCaptureComponent>,
    pub fan_turns: Option<i32>,
    pub gravity_flip: Option<i32>,
    pub hook_disabled: Option<bool>,
    pub connector_initiated: Option<bool>,
    pub elemental_holder_enable: Option<bool>,
    pub nearby_tracking_enable: Option<bool>,
    pub flower_pollution: bool,
    pub range_present: bool,
    pub trigger_present: bool,
    pub patrol_dir: Option<bool>,
    pub monster_weapon_id: Option<i32>,
    pub buff_producer_present: bool,
    pub buff_consumer_present: bool,
    pub listener_present: bool,
    pub batch_bullet_cast_present: bool,
    pub be_controlled_present: bool,
    pub drop_present: bool,
    pub hacking_present: bool,
    pub hack_target_present: bool,
    pub pulling_foundation_present: bool,
    pub spirit_gear_present: bool,
    pub foundation_source_entity_id: i32,
    pub sub_entity_type: i32,
    /// Remaining durability of a destructible item; 0 for non-destructible entities.
    pub durability: i32,
    pub drop_item_id: i32,
    pub drop_item_count: i32,
    pub in_range_entity_ids: [i64; 8],
    pub in_range_entity_len: u8,
    pub be_controlled_player_id: Option<i64>,
    pub be_controlled_show: bool,
    pub npc_spline_id: i32,
    pub npc_spawn_id: i32,
    pub hold_hand_target_id: Option<i64>,
    pub hold_hand_type: i32,
    pub in_range_player_id: i32,
    pub trigger_count: i32,
    pub exit_trigger_count: i32,
}

impl Default for WorldItemState {
    fn default() -> Self {
        Self {
            gravity: None,
            lift: None,
            lift_init_pos: None,
            move_spline: None,
            move_to_point: None,
            scene_item: None,
            interact: None,
            monster_capture: None,
            fan_turns: None,
            gravity_flip: None,
            hook_disabled: None,
            connector_initiated: None,
            elemental_holder_enable: None,
            nearby_tracking_enable: None,
            flower_pollution: false,
            range_present: false,
            trigger_present: false,
            patrol_dir: None,
            monster_weapon_id: None,
            buff_producer_present: false,
            buff_consumer_present: false,
            listener_present: false,
            batch_bullet_cast_present: false,
            be_controlled_present: false,
            drop_present: false,
            hacking_present: false,
            hack_target_present: false,
            pulling_foundation_present: false,
            spirit_gear_present: false,
            foundation_source_entity_id: 0,
            sub_entity_type: 0,
            durability: 0,
            drop_item_id: 0,
            drop_item_count: 1,
            in_range_entity_ids: [0; 8],
            in_range_entity_len: 0,
            be_controlled_player_id: None,
            be_controlled_show: false,
            npc_spline_id: 0,
            npc_spawn_id: 0,
            hold_hand_target_id: None,
            hold_hand_type: 0,
            in_range_player_id: 0,
            trigger_count: 0,
            exit_trigger_count: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntityComponentStorage {
    pub net_id: i64,
    pub config: ConfigComponent,
    pub position: Option<PositionComponent>,
    pub world_item: WorldItemState,
    pub player_id: Option<i32>,
    pub visible: bool,
    pub actor_visible: bool,
    pub direct_control: bool,
    pub attribute: Option<AttributeComponent>,
    pub tag: Option<TagComponent>,
    pub part: Option<PartComponent>,
    pub buffs: Option<FightBuffComponent>,
    pub character_attach: Option<CharacterAttachComponent>,
    pub concomitant: Option<ConcomitantComponent>,
    pub equip: Option<EquipComponent>,
    pub follower: Option<FollowerComponent>,
    pub vehicle: Option<VehicleComponent>,
    pub npc_drive_vehicle: Option<NpcDriveVehicleComponent>,
    pub motor_outlook: Option<MotorOutlookComponent>,
    pub motor_da_ctx: Option<MotorDaCtxComponent>,
    pub logic_state: Option<LogicStateComponent>,
    pub monster_ai: Option<MonsterAiComponent>,
    pub passive_ga_skill: Option<PassiveGaSkillComponent>,
    pub player_battle_binder: Option<PlayerBattleBinder>,
    pub summoner: Option<SummonerComponent>,
    pub follow_shooter: Option<FollowShooterComponent>,
    pub follow_entity: Option<FollowEntityComponent>,
    pub fsm: Option<FsmComponent>,
    pub vision_skills: Option<VisionSkillComponent>,
    pub base_skin: Option<BaseSkinComponent>,
    pub calabash_skin: Option<CalabashSkinComponent>,
    pub weapon_skin: Option<WeaponSkinComponent>,
    pub ornament: Option<OrnamentComponent>,
    pub state_tag: Option<StateTagComponent>,
}

impl EntityComponentStorage {
    pub fn new(net_id: i64, config: ConfigComponent) -> Self {
        Self {
            net_id,
            config,
            position: None,
            world_item: WorldItemState::default(),
            player_id: None,
            visible: false,
            actor_visible: false,
            direct_control: false,
            attribute: None,
            tag: None,
            part: None,
            buffs: None,
            character_attach: None,
            concomitant: None,
            equip: None,
            follower: None,
            vehicle: None,
            npc_drive_vehicle: None,
            motor_outlook: None,
            motor_da_ctx: None,
            logic_state: None,
            monster_ai: None,
            passive_ga_skill: None,
            player_battle_binder: None,
            summoner: None,
            follow_shooter: None,
            follow_entity: None,
            fsm: None,
            vision_skills: None,
            base_skin: None,
            calabash_skin: None,
            weapon_skin: None,
            ornament: None,
            state_tag: None,
        }
    }

    pub fn to_proto(&self, assets: &Assets, now_ms: i64) -> EntityPb {
        let base_skin_comp = self.base_skin.unwrap_or_default();
        let living_status = self.attribute.as_ref().and_then(|attr_comp| {
            let life_idx = EAttributeType::Life.to_i32() as usize;
            let max_idx = EAttributeType::LifeMax.to_i32() as usize;
            let life_max = attr_comp.attributes.get(max_idx).map_or(0, |a| a.current);
            if life_max <= 0 {
                return None;
            }
            let life = attr_comp.attributes.get(life_idx).map_or(0, |a| a.current);
            Some(if life <= 0 {
                sonoro_proto::enums::LivingStatus::Dead
            } else {
                sonoro_proto::enums::LivingStatus::Alive
            })
        });

        let resolved_sub_type = if self.world_item.sub_entity_type != 0 {
            self.world_item.sub_entity_type
        } else if self.config.entity_type == EntityType::Npc {
            if let Some(le) = assets.find_level_entity_any_map(i64::from(self.config.config_id)) {
                if let Some(nt) = le
                    .ComponentsData
                    .BaseInfoComponent
                    .as_ref()
                    .and_then(|bi| bi.Category.as_ref())
                    .and_then(|cat| cat.NpcType)
                {
                    nt
                } else if le.BlueprintType == "SimpleNpc" || le.BlueprintType == "SimpleNPc" {
                    1
                } else if le.BlueprintType == "PasserbyNpc" {
                    2
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            0
        };

        let mut entity = EntityPb {
            Id: self.net_id,
            ConfigId: self.config.config_id,
            ConfigType: Some(self.config.config_type.to_proto()),
            EntityType: Some(self.config.entity_type.to_proto()),
            SubEntityType: resolved_sub_type,
            DurabilityValue: self.world_item.durability,
            EntityState: Some(self.config.state.to_proto()),
            d3s: Some(EntityPbD3sOneof::Camp(self.config.camp)),
            LivingStatus: living_status,
            IsVisible: self.visible,
            IsActorVisible: self.actor_visible,
            PlayerId: self.player_id.unwrap_or(0),
            InitLinearVelocity: Some(Vector::default()),
            Gravity: Some(match self.world_item.gravity {
                Some(g) => Vector {
                    X: g.location[0],
                    Y: g.location[1],
                    Z: g.location[2],
                },
                None => Vector {
                    X: 0.0,
                    Y: 0.0,
                    Z: -1.0,
                },
            }),
            RoleSkinId: base_skin_comp.role_skin_id,
            ParaglidingSkinId: base_skin_comp.paragliding_skin_id,
            SoarWingSkinId: base_skin_comp.soar_skin_id,
            ..Default::default()
        };

        if let Some(pos_comp) = self.position {
            let t = pos_comp.to_proto();
            entity.Pos = t.Pos.clone();
            entity.Rot = t.Rot;
            entity.InitPos = match self.world_item.lift_init_pos {
                Some([x, y, z]) => Some(Vector { X: x, Y: y, Z: z }),
                None => t.Pos,
            };
        }

        let wi = &self.world_item;
        let push = |out: &mut Vec<EntityComponentPb>, oneof: EntityComponentPbComponentPbOneof| {
            out.push(EntityComponentPb {
                ComponentPb: Some(oneof),
            });
        };

        if let Some(comp) = wi.lift {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::LiftComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &wi.move_spline {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::EntityMoveSplineComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = wi.move_to_point {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::MoveToPointComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = wi.scene_item {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::SceneItemComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &wi.interact {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::InteractComponent(comp.to_proto()),
            );
        }
        if let Some(comp) = wi.monster_capture {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::MonsterCaptureComponent(comp.to_proto()),
            );
        }
        if let Some(turns) = wi.fan_turns {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::FanComponentPb(FanComponentPb {
                    NumOfTurns: turns,
                }),
            );
        }
        if let Some(gt) = wi.gravity_flip
            && (0..=3).contains(&gt)
        {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::GravityFlipComponent(GravityFlipComponent {
                    Direction: Some(DirectionType::from_i32_or_default(gt)),
                }),
            );
        }
        if let Some(disabled) = wi.hook_disabled {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::GrapplingHookPointComponentPb(
                    GrapplingHookPointComponentPb {
                        HookLockPointDisabled: disabled,
                    },
                ),
            );
        }
        if let Some(initiated) = wi.connector_initiated {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::ConnectorComponentPb(ConnectorComponentPb {
                    HasInitiatedConnect: initiated,
                }),
            );
        }
        if let Some(enabled) = wi.elemental_holder_enable {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::ElementalHolderComponentPb(
                    ElementalHolderComponentPb { Enable: enabled },
                ),
            );
        }
        if let Some(enabled) = wi.nearby_tracking_enable {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::NearbyTrackingComponentPb(
                    NearbyTrackingComponentPb { IsEnable: enabled },
                ),
            );
        }
        if wi.flower_pollution {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::FlowerPollutionComponentPb(
                    FlowerPollutionComponentPb::default(),
                ),
            );
        }
        if wi.range_present {
            let mut in_range_players = Vec::new();
            if wi.in_range_player_id > 0 {
                in_range_players.push(wi.in_range_player_id);
            }
            let n_ents = usize::from(wi.in_range_entity_len).min(wi.in_range_entity_ids.len());
            let in_range_entities = wi.in_range_entity_ids[..n_ents]
                .iter()
                .copied()
                .filter(|&eid| eid > 0)
                .collect();
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::RangeComponent(RangeComponentPb {
                    InRangePlayers: in_range_players,
                    InRangeEntities: in_range_entities,
                }),
            );
        }
        if wi.trigger_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::TriggerComponent(TriggerComponentPb {
                    TriggerCount: wi.trigger_count,
                    ExitTriggerCount: wi.exit_trigger_count,
                    ConstateId: 0,
                }),
            );
        }
        if let Some(dir) = wi.patrol_dir {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::PatrolComponent(PatrolComponentPb { Dir: dir }),
            );
        }
        if let Some(wid) = wi.monster_weapon_id {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::MonsterWeaponComponentPb(
                    MonsterWeaponComponentPb { WeaponId: wid },
                ),
            );
        }
        if wi.buff_producer_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::BuffProducerComponentPb(
                    BuffProducerComponentPb::default(),
                ),
            );
        }
        if wi.buff_consumer_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::BuffConsumerComponentPb(
                    BuffConsumerComponentPb::default(),
                ),
            );
        }
        if wi.listener_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::SceneItemEventListenerComponentPb(
                    SceneItemEventListenerComponentPb::default(),
                ),
            );
        }
        if wi.batch_bullet_cast_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::BatchBulletCastComponentPb(
                    BatchBulletCastComponentPb::default(),
                ),
            );
        }
        if wi.hacking_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::HackingComponentPb(
                    HackingComponentPb::default(),
                ),
            );
        }
        if wi.hack_target_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::HackTargetComponentPb(
                    HackTargetComponentPb::default(),
                ),
            );
        }
        if wi.pulling_foundation_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::PullingFoundationComponentPb(
                    PullingFoundationComponentPb::default(),
                ),
            );
        }
        if wi.spirit_gear_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::SpiritGearComponentPb(
                    SunSpiritGearComponentPb::default(),
                ),
            );
        }
        if wi.be_controlled_present || wi.be_controlled_player_id.is_some() || wi.be_controlled_show
        {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::BeControlledComponentPb(
                    BeControlledComponentPb {
                        PlayerEntityId: wi.be_controlled_player_id.unwrap_or(0),
                        IsShow: wi.be_controlled_show,
                        ..Default::default()
                    },
                ),
            );
        }
        if wi.drop_present {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::DropComponentPb(DropComponentPb {
                    ItemId: wi.drop_item_id,
                    ItemCount: wi.drop_item_count,
                    EntityConfigId: self.config.config_id,
                    ..Default::default()
                }),
            );
        }
        if wi.foundation_source_entity_id > 0 {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::EntityFoundationComponentPb(
                    EntityFoundationComponentPb {
                        egM: Some(EntityFoundationComponentPbEgMOneof::FoundationCoreInfo(
                            FoundationCoreInfo {
                                SourceEntityId: wi.foundation_source_entity_id,
                                State: "关卡.Common.状态.激活".as_bytes().to_vec(),
                                ..Default::default()
                            },
                        )),
                    },
                ),
            );
        }
        if self.config.entity_type == EntityType::Npc
            && (wi.npc_spline_id != 0 || wi.npc_spawn_id != 0)
        {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::NpcPb(NpcPb {
                    SplineEntityId: wi.npc_spline_id,
                    SpawnEntityId: wi.npc_spawn_id,
                }),
            );
        }
        if let Some(target_eid) = wi.hold_hand_target_id {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::HoldHandComponentPb(HoldHandComponentPb {
                    TargetEntityId: target_eid,
                    HandType: wi.hold_hand_type,
                    IsFollow: true,
                    ..Default::default()
                }),
            );
        }

        if let Some(attr_comp) = &self.attribute {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::AttributeComponent(attr_comp.to_proto()),
            );
            let mut shield_list = Vec::new();
            let mut shield_total = 0i32;
            if let Some(buffs) = &self.buffs {
                for sh in &buffs.shields {
                    if sh.current <= 0 {
                        continue;
                    }
                    shield_total = shield_total.saturating_add(sh.current);
                    shield_list.push(ShieldInfoPb {
                        Handle: sh.handle_id,
                        ConfigId: i32::try_from(sh.shield_id).unwrap_or(0),
                        ShieldValue: sh.current,
                        Priority: sh.priority,
                        IsValid: true,
                        ..Default::default()
                    });
                }
            }
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::ShieldComponentPb(ShieldComponentPb {
                    ShieldInfoPbList: shield_list,
                    ShieldValueTotal: shield_total,
                }),
            );
        }

        if let Some(tag_comp) = &self.tag {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::TagComponent(tag_comp.to_proto()),
            );
        }
        if let Some(part_comp) = &self.part {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::PartComponent(part_comp.to_proto()),
            );
        }
        if let Some(buff_comp) = &self.buffs {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::FightBuffComponent(buff_comp.to_proto()),
            );
        }
        if let Some(comp) = &self.character_attach {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::CharacterAttachComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &self.concomitant {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::ConcomitantsComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.equip {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::EquipComponent(comp.to_proto()),
            );
        }
        if let Some(comp) = &self.follower {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::FollowerComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &self.vehicle {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::VehiclePb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.npc_drive_vehicle {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::NpcDriveVehicleComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &self.motor_outlook {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::MotorOutlookComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.motor_da_ctx {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::MotorDaCtxComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.logic_state {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::LogicStateComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.monster_ai {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::MonsterAiComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &self.passive_ga_skill {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::PassiveGaSkillComponentPb(comp.to_proto()),
            );
            let passive_pb = comp.to_passive_skill_proto(assets, self.net_id);
            if !passive_pb.PassiveSkillItemPbList.is_empty() {
                push(
                    &mut entity.ComponentPbs,
                    EntityComponentPbComponentPbOneof::PassiveSkillComponentPb(passive_pb),
                );
            }
        }
        if let Some(comp) = self.player_battle_binder {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::PlayerSceneComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.summoner {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::SummonerComponent(comp.to_proto()),
            );
        }
        if let Some(comp) = self.follow_shooter {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::FollowShooterComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.follow_entity {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::FollowEntityComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &self.fsm {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::EntityFsmComponentPb(comp.to_proto(now_ms)),
            );
        }
        if let Some(comp) = &self.vision_skills {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::VisionSkillComponent(comp.to_proto()),
            );
        }
        if let Some(comp) = self.weapon_skin {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::WeaponSkinComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = &self.ornament {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::OrnamentComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.calabash_skin {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::CalabashSkinComponentPb(comp.to_proto()),
            );
        }
        if let Some(comp) = self.state_tag {
            push(
                &mut entity.ComponentPbs,
                EntityComponentPbComponentPbOneof::StateTagComponentPb(comp.to_proto()),
            );
        }

        entity
    }

    pub fn save(&self, fs: &FileSystem, player_id: i32, scene_id: i32) -> io::Result<()> {
        let base = format!("player/{player_id}/scene/{scene_id}/entity/{}", self.net_id);
        if let Some(pos) = self.position {
            #[derive(serde::Serialize)]
            struct DiskPos {
                location: [f32; 3],
                rotation: [f32; 3],
            }
            comp_util::save_struct(
                fs,
                &DiskPos {
                    location: pos.location,
                    rotation: pos.rotation,
                },
                &format!("{base}/position"),
            )?;
        }
        Ok(())
    }
}

pub fn load_position_file(
    fs: &FileSystem,
    player_id: i32,
    scene_id: i32,
    entity_id: i64,
) -> io::Result<Option<PositionComponent>> {
    #[derive(serde::Deserialize)]
    struct DiskPos {
        location: [f32; 3],
        rotation: [f32; 3],
    }
    let path = format!("player/{player_id}/scene/{scene_id}/entity/{entity_id}/position");
    Ok(load_zon::<DiskPos>(fs, &path)?.map(|d| PositionComponent {
        location: d.location,
        rotation: d.rotation,
    }))
}
