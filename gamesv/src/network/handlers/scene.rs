use sonoro_gamesv_data::asset_indexes::entity_world_pos;
use sonoro_gamesv_logic::component::comp_util;
use sonoro_gamesv_logic::component::entity::small::{ConfigComponent, ConfigType, EntityState, EntityType, PositionComponent, VehicleComponent};
use sonoro_gamesv_logic::component::entity::EntityComponentStorage;
use sonoro_gamesv_logic::event_queue::EventQueue;
use sonoro_gamesv_logic::events::Event;
use sonoro_gamesv_logic::helpers::{
    entity_proto,
    level_play::{self, LevelPlayContext},
    map as map_helper, quest_actions, quest_engine, role as role_helper,
};
use sonoro_gamesv_logic::scene::{DynamicInteractKind, now_millis};
use sonoro_proto::enums::{BtType, ErrorCode, ExitVehicleType, GameCtxType, VehicleSource};
use sonoro_proto::pb::{
    AnimalDestroyRequest, AnimalDestroyResponse, AnimalDieRequest, AnimalDieResponse,
    AnimalDropRequest, AnimalDropResponse, ApplySceneClientEntityIncIdRequest,
    ApplySceneClientEntityIncIdResponse, AwakeItemVehicleRequest, AwakeItemVehicleResponse,
    BehaviorTreeCtxPb, ChangeWeatherRequest, ChangeWeatherResponse, ClientEntityInfoPb,
    ClientEntityInfoPbDOLLARFyOneof, ClientEntityInfoPbWFyOneof, DoInteractChildQuestNodeCtxPb,
    DynamicInteractCtxPb, EntityActiveRequest, EntityActiveResponse, EntityCtxPb,
    EntityDynamicInteractRequest, EntityDynamicInteractResponse,
    EntityEnterVehicleRequest, EntityEnterVehicleRequestExitOneof, EntityEnterVehicleResponse,
    EntityInteractRequest, EntityInteractResponse, EntityPosAbnormalRequest,
    EntityPosAbnormalResponse, EntityPosResetNotify, EntityPositionRequest,
    EntityPositionResponse, FlowStartNotify, GameCtxPb, GameCtxPbCtxInfoOneof,
    GetRewardTreasureBoxRequest, GetRewardTreasureBoxResponse, JoinSceneNotifyConfirmRequest,
    JoinSceneNotifyConfirmResponse, MovePackageNotify, MovePackagePush, MoveReplaySample,
    MovingEntityData, PlayerMotionRequest, PlayerMotionResponse, Rotator,
    SceneLoadingFinishRequest, SceneLoadingFinishResponse, TeleportUpdateNotify,
    UpdateSceneDateRequest, UpdateSceneDateResponse, Vector, VehicleChangePassengerRequest,
    VehicleChangePassengerResponse, VehicleEnterSharingRequest, VehicleEnterSharingResponse,
    VehicleFinishRequest, VehicleFinishResponse, VehiclePassengerUpdateNotify,
    VehicleRemovePassengerRequest, VehicleRemovePassengerResponse, VehicleRequest, VehicleResponse,
    VehicleUpdateEntityNotify, VehicleUpdateNotify,
};

use super::State;

pub fn handle_join_scene_notify_confirm_request(
    state: &mut State,
    events: &mut EventQueue,
    req: JoinSceneNotifyConfirmRequest,
) -> Result<JoinSceneNotifyConfirmResponse, String> {
    if state.scene.is_some() && req.SceneTraceId > 0 {
        let mut qctx = state.quest_context(events);
        if let Err(err) = quest_engine::on_scene_trace(&mut qctx, req.SceneTraceId) {
            tracing::warn!(target: "scene", "on_scene_trace failed: {err}");
        }
    }
    let key = format!("scene:join_confirm:{}", req.SceneTraceId);
    state.player_components.progress.info.mark_claim(&key);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(JoinSceneNotifyConfirmResponse {})
}

pub fn handle_update_scene_date_request(
    state: &mut State,
    events: &mut EventQueue,
    req: UpdateSceneDateRequest,
) -> Result<UpdateSceneDateResponse, String> {
    let mut hour = req.Hour;
    let mut minute = req.Minute;
    if req.AddDays != 0
        && hour == 0
        && minute == 0
        && let Some(s) = state.scene.as_ref()
    {
        hour = s.instance.map_time.hour;
        minute = s.instance.map_time.minute;
    }
    {
        let mut qctx = state.quest_context(events);
        quest_engine::set_scene_time(&mut qctx, hour, minute, false).map_err(|e| e.to_string())?;
    }
    let mut day = 1u32;
    if let Some(s) = state.scene.as_mut() {
        s.instance.map_time.day = s.instance.map_time.day.saturating_add(req.AddDays);
        day = s.instance.map_time.day;
        s.save_instance(&state.fs).map_err(|e| e.to_string())?;
    }
    Ok(UpdateSceneDateResponse {
        ErrorCode: Some(ErrorCode::Success),
        CurrDate: day,
    })
}

pub fn handle_change_weather_request(
    state: &mut State,
    events: &mut EventQueue,
    req: ChangeWeatherRequest,
) -> Result<ChangeWeatherResponse, String> {
    let mut qctx = state.quest_context(events);
    quest_engine::change_weather(&mut qctx, req.WeatherId).map_err(|e| e.to_string())?;
    Ok(ChangeWeatherResponse {
        WeatherId: req.WeatherId,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_active_request(
    state: &mut State,
    req: EntityActiveRequest,
) -> Result<EntityActiveResponse, String> {
    let Some(scene) = state.scene.as_mut() else {
        return Ok(EntityActiveResponse {
            ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
            IsVisible: false,
            Pos: Some(Vector::default()),
            Rot: Some(Rotator::default()),
            AiControlPlayerId: state.player_id,
            ..Default::default()
        });
    };
    let ai_ctrl_id = if scene.player_id != 0 {
        scene.player_id
    } else {
        state.player_id
    };
    if scene.net_id_map.contains_key(&req.EntityId)
        && let Ok(entity_pb) = entity_proto::build(
            &state.assets,
            scene,
            &mut state.buff_timers,
            req.EntityId,
            now_millis(),
        )
    {
        return Ok(EntityActiveResponse {
            IsVisible: entity_pb.IsVisible,
            Rot: entity_pb.Rot,
            Pos: entity_pb.Pos,
            ComponentPbs: entity_pb.ComponentPbs,
            AiControlPlayerId: ai_ctrl_id,
            ..Default::default()
        });
    }

    Ok(EntityActiveResponse {
        ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
        IsVisible: false,
        Pos: Some(Vector::default()),
        Rot: Some(Rotator::default()),
        AiControlPlayerId: ai_ctrl_id,
        ..Default::default()
    })
}

pub fn handle_player_motion_request(
    state: &mut State,
    events: &mut EventQueue,
    req: PlayerMotionRequest,
) -> Result<PlayerMotionResponse, String> {
    if let Some(m) = req.Motion
        && let Some(scene) = state.scene.as_mut()
    {
        scene.player_motion_type = m;
        scene.has_player_motion = true;
        let mut qctx = state.quest_context(events);
        if let Err(err) = quest_engine::events::on_condition_state_changed(&mut qctx) {
            tracing::warn!(target: "scene", "on_player_motion failed: {err}");
        }
    }
    Ok(PlayerMotionResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub(crate) fn resolve_entity_world_position(
    state: &State,
    target_inst_or_map: i32,
    req_cfg_id: i32,
) -> Option<Vector> {
    if req_cfg_id <= 0 {
        return None;
    }
    let resolved_map_id = state.assets.resolve_map_config_id(target_inst_or_map);
    if let Some(scene) = state.scene.as_ref()
        && (scene.instance_id == target_inst_or_map
            || state.assets.resolve_map_config_id(scene.instance_id) == resolved_map_id)
        && let Some(net_id) = scene.find_entity_net_id_by_config_id(req_cfg_id)
        && let Some(pos) = scene.entity(net_id).and_then(|s| s.position)
    {
        return Some(Vector {
            X: pos.location[0],
            Y: pos.location[1],
            Z: pos.location[2],
        });
    }
    if let Some(cfg) = state
        .assets
        .find_level_entity(resolved_map_id, i64::from(req_cfg_id))
        .or_else(|| {
            state
                .assets
                .find_level_entity(target_inst_or_map, i64::from(req_cfg_id))
        })
        && let Some(pos) = entity_world_pos(&cfg)
    {
        return Some(Vector {
            X: pos[0],
            Y: pos[1],
            Z: pos[2],
        });
    }
    None
}

pub fn handle_entity_position_request(
    state: &mut State,
    req: EntityPositionRequest,
) -> Result<EntityPositionResponse, String> {
    let target_map_id = if req.DungeonInstanceId > 0 {
        req.DungeonInstanceId
    } else if let Some(scene) = state.scene.as_ref() {
        scene.instance_id
    } else {
        8
    };

    if let Some(pos) = resolve_entity_world_position(state, target_map_id, req.ConfigId) {
        return Ok(EntityPositionResponse {
            ErrorCode: Some(ErrorCode::Success),
            Pos: Some(pos),
        });
    }

    Ok(EntityPositionResponse {
        ErrorCode: Some(ErrorCode::ErrSceneEntityNotExist),
        Pos: Some(Vector::default()),
    })
}

pub fn handle_scene_loading_finish_request(
    state: &mut State,
    events: &mut EventQueue,
    _req: SceneLoadingFinishRequest,
) -> Result<SceneLoadingFinishResponse, String> {
    if let Some(scene) = state.scene.as_mut() {
        scene.scene_loading_finished = true;
        let run_deferred = scene.deferred_join_sync_pending;
        scene.deferred_join_sync_pending = false;
        let already_login_synced = std::mem::take(&mut scene.login_world_synced);
        let player_loc = scene.instance.players.first().map(|p| p.location);
        if let Some(loc) = player_loc {
            scene.last_spatial_check_loc = loc;
        }
        let inst_id = scene.instance_id;
        let deferred_flow = scene.deferred_join_flow.take();

        super::flow::clear_client_black_screen_if_idle(state);
        if run_deferred && !already_login_synced {
            let mut qctx = state.quest_context(events);
            quest_actions::sync_world_state_on_join(&mut qctx).map_err(|e| e.to_string())?;
            if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
                level_play::sync_on_login(&mut lp_ctx).map_err(|e| e.to_string())?;
            }
        } else if inst_id != 8 {
            let mut qctx = state.quest_context(events);
            if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
                level_play::sync_dungeon_level_plays(&mut lp_ctx).map_err(|e| e.to_string())?;
            }
        }
        if player_loc.is_some() {
            events.enqueue(Event::SpatialSpawnsCheck {
                player_id: state.player_id,
            });
        }
        {
            let mut qctx = state.quest_context(events);
            quest_engine::on_scene_loading_finish(&mut qctx).map_err(|e| e.to_string())?;
            if inst_id != 8 {
                quest_engine::events::on_dungeon_event(&mut qctx, inst_id, false)
                    .map_err(|e| e.to_string())?;
            }
        }
        if let Some(flow) = deferred_flow {
            let _ = state.conn.push_msg(
                FlowStartNotify::MSG_ID,
                &FlowStartNotify {
                    FlowIncId: i64::from(flow.inc_id),
                    FlowListName: flow.namespace.into_bytes(),
                    FlowId: flow.id,
                    StateId: flow.state,
                    ..Default::default()
                },
            );
        }
    }
    super::flow::clear_client_black_screen_if_idle(state);
    Ok(SceneLoadingFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

fn current_role_position(state: &State) -> Option<PositionComponent> {
    let scene = state.scene.as_ref()?;
    let f_idx = usize::try_from(scene.formation_info.cur_formation).ok()?;
    let cur_role = scene.formation_info.formations.get(f_idx)?.cur_role;
    scene.entities.iter().find_map(|s| {
        if s.direct_control
            && s.config.entity_type == EntityType::Player
            && s.config.config_type == ConfigType::Character
            && s.config.config_id == cur_role
        {
            s.position
        } else {
            None
        }
    })
}

fn apply_vehicle_transition(
    state: &mut State,
    events: &mut EventQueue,
    entity_id: i64,
    is_entering: bool,
    seat: i32,
    exit_type: Option<ExitVehicleType>,
    client_predicted: bool,
) -> Result<ErrorCode, String> {
    let role_pos = current_role_position(state);
    let (player_loc, veh_cfg_id) = {
        let Some(scene) = state.scene.as_mut() else {
            return Ok(if is_entering {
                ErrorCode::ErrVehicleEntityNotExist
            } else {
                ErrorCode::Success
            });
        };
        if let Some(&idx) = scene.net_id_map.get(&entity_id) {
            let storage = &mut scene.entities[idx];
            if storage.vehicle.is_none() {
                storage.vehicle = Some(VehicleComponent {
                    source: Some(VehicleSource::VehicleSourceNone),
                    ..Default::default()
                });
            }
            if let Some(veh) = storage.vehicle.as_mut() {
                veh.occupied = is_entering;
            }
            if is_entering && let Some(rp) = role_pos {
                if let Some(pos) = storage.position.as_mut() {
                    pos.location = rp.location;
                    pos.rotation = rp.rotation;
                } else {
                    storage.position = Some(rp);
                }
            }
            let cfg_id = i64::from(storage.config.config_id);
            scene
                .save_entity_by_net_id(&state.fs, entity_id)
                .map_err(|e| e.to_string())?;
            let loc = scene
                .instance
                .players
                .first()
                .map_or([0.0, 0.0, 0.0], |p| p.location);
            (loc, cfg_id)
        } else if is_entering {
            return Ok(ErrorCode::ErrVehicleEntityNotExist);
        } else {
            let loc = scene
                .instance
                .players
                .first()
                .map_or([0.0, 0.0, 0.0], |p| p.location);
            (loc, 0)
        }
    };

    let _ = state.conn.push_msg(
        VehicleUpdateNotify::MSG_ID,
        &VehicleUpdateNotify {
            EntityId: if is_entering { entity_id } else { 0 },
            PlayerId: state.player_id,
            SeatId: if is_entering { seat } else { -1 },
            ExitType: if is_entering { None } else { exit_type },
            ClientPredicted: client_predicted,
        },
    );

    state.player_components.progress.info.set_key_int(
        "vehicle:cur",
        if is_entering { veh_cfg_id } else { 0 },
    );
    state.player_components.progress.info.set_key_int(
        "vehicle:seat",
        if is_entering { i64::from(seat) } else { -1 },
    );
    let _ = state.player_components.progress.save(&state.fs);

    let mut qctx = state.quest_context(events);
    if let Err(err) =
        quest_engine::events::on_player_move(&mut qctx, player_loc[0], player_loc[1], player_loc[2])
    {
        tracing::warn!(target: "scene", "on_player_move failed: {err}");
    }
    if let Err(err) = quest_engine::events::on_condition_state_changed(&mut qctx) {
        tracing::warn!(target: "scene", "on_condition_state_changed failed: {err}");
    }
    Ok(ErrorCode::Success)
}

pub fn handle_vehicle_request(
    state: &mut State,
    events: &mut EventQueue,
    req: VehicleRequest,
) -> Result<VehicleResponse, String> {
    let code = apply_vehicle_transition(
        state,
        events,
        req.EntityId,
        req.IsGetOn,
        req.SeatId,
        req.ExitType,
        req.ClientPredicted,
    )?;
    Ok(VehicleResponse {
        ErrorCode: Some(code),
    })
}

pub fn handle_vehicle_finish_request(
    _state: &mut State,
    _req: VehicleFinishRequest,
) -> Result<VehicleFinishResponse, String> {
    Ok(VehicleFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_vehicle_change_passenger_request(
    state: &mut State,
    req: VehicleChangePassengerRequest,
) -> Result<VehicleChangePassengerResponse, String> {
    let _ = state.conn.push_msg(
        VehiclePassengerUpdateNotify::MSG_ID,
        &VehiclePassengerUpdateNotify {
            PlayerId: state.player_id,
            RoleId: req.RoleId,
            SeatId: req.SeatId,
            ..Default::default()
        },
    );
    Ok(VehicleChangePassengerResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_vehicle_remove_passenger_request(
    state: &mut State,
    req: VehicleRemovePassengerRequest,
) -> Result<VehicleRemovePassengerResponse, String> {
    let _ = state.conn.push_msg(
        VehiclePassengerUpdateNotify::MSG_ID,
        &VehiclePassengerUpdateNotify {
            PlayerId: state.player_id,
            RoleId: req.RoleId,
            SeatId: -1,
            ..Default::default()
        },
    );
    Ok(VehicleRemovePassengerResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_vehicle_enter_sharing_request(
    state: &mut State,
    req: VehicleEnterSharingRequest,
) -> Result<VehicleEnterSharingResponse, String> {
    state
        .player_components
        .progress
        .info
        .set_key_int("vehicle:sharing", if req.IsEnter { 1 } else { 0 });
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(VehicleEnterSharingResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_awake_item_vehicle_request(
    state: &mut State,
    events: &mut EventQueue,
    req: AwakeItemVehicleRequest,
) -> Result<AwakeItemVehicleResponse, String> {
    state.player_components.progress.info.set_key_int(
        &format!("vehicle:awake:{}", req.ItemID),
        i64::from(req.PosEntityId),
    );
    if let Some(scene) = state.scene.as_mut()
        && let Some(pos) = state
            .assets
            .find_level_entity(scene.instance_id, i64::from(req.PosEntityId))
            .and_then(|cfg| entity_world_pos(&cfg))
    {
        for (field, val) in ["x", "y", "z"].into_iter().zip(pos) {
            state.player_components.progress.info.set_key_int(
                &format!("world_pos:{}:{field}", req.ItemID),
                i64::from(val.to_bits()),
            );
        }
        if let Some(veh_nid) = scene.find_entity_net_id_by_config_id(req.ItemID)
            && scene.set_entity_position(veh_nid, pos, None)
        {
            let _ = state.conn.push_msg(
                MovePackageNotify::MSG_ID,
                &MovePackageNotify {
                    MovingEntities: vec![MovingEntityData {
                        EntityId: veh_nid,
                        Originator: veh_nid,
                        ForcePush: true,
                        MoveInfos: vec![MoveReplaySample {
                            Location: Some(Vector {
                                X: pos[0],
                                Y: pos[1],
                                Z: pos[2],
                            }),
                            ..Default::default()
                        }],
                    }],
                },
            );
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let mut qctx = state.quest_context(events);
    quest_engine::events::on_condition_state_changed(&mut qctx).map_err(|e| e.to_string())?;
    Ok(AwakeItemVehicleResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_enter_vehicle_request(
    state: &mut State,
    req: EntityEnterVehicleRequest,
) -> Result<EntityEnterVehicleResponse, String> {
    let Some(scene) = state.scene.as_mut() else {
        return Ok(EntityEnterVehicleResponse {
            ErrorCode: Some(ErrorCode::ErrVehicleEntityNotExist),
        });
    };
    let Some(&idx) = scene.net_id_map.get(&req.VehicleCreatureId) else {
        return Ok(EntityEnterVehicleResponse {
            ErrorCode: Some(ErrorCode::ErrVehicleEntityNotExist),
        });
    };
    let storage = &mut scene.entities[idx];
    if storage.vehicle.is_none() {
        storage.vehicle = Some(VehicleComponent {
            source: Some(VehicleSource::VehicleSourceNone),
            ..Default::default()
        });
    }
    if let Some(veh) = storage.vehicle.as_mut() {
        veh.occupied = req.IsEntering;
    }
    scene
        .save_entity_by_net_id(&state.fs, req.VehicleCreatureId)
        .map_err(|e| e.to_string())?;

    let exit_type = if req.IsEntering {
        None
    } else if let Some(EntityEnterVehicleRequestExitOneof::ExitType(et)) = req.Exit {
        Some(et)
    } else {
        None
    };
    let _ = state.conn.push_msg(
        VehicleUpdateEntityNotify::MSG_ID,
        &VehicleUpdateEntityNotify {
            EntityId: req.EntityId,
            VehicleCreatureId: req.VehicleCreatureId,
            Seat: if req.IsEntering { req.Seat } else { -1 },
            IsEntering: req.IsEntering,
            ExitType: exit_type,
            ClientPredicted: req.ClientPredicted,
        },
    );
    Ok(EntityEnterVehicleResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_move_package_push(
    state: &mut State,
    events: &mut EventQueue,
    req: MovePackagePush,
) -> Result<(), String> {
    let sender_id = state.player_id;
    let moving_entities = req.MovingEntities;
    for data in &moving_entities {
        let Some(last_move_replay) = data.MoveInfos.last() else {
            continue;
        };
        let Some(location) = last_move_replay.Location.as_ref() else {
            continue;
        };
        let next_location = [location.X, location.Y, location.Z];
        if !next_location.iter().all(|v| v.is_finite()) {
            continue;
        }
        if let Some(rotation) = last_move_replay.Rotation.as_ref()
            && (!rotation.Roll.is_finite()
                || !rotation.Pitch.is_finite()
                || !rotation.Yaw.is_finite())
        {
            continue;
        }

        let (controlled_by_player, move_quest_pos, mode_changed, next_rot) = {
            let Some(scene) = state.scene.as_mut() else {
                continue;
            };
            let Some(&eidx) = scene.net_id_map.get(&data.EntityId) else {
                continue;
            };
            if scene.entities[eidx].position.is_none() {
                continue;
            }
            let is_occupied_player_vehicle = scene.entities[eidx]
                .vehicle
                .as_ref()
                .is_some_and(|v| v.occupied)
                && scene.entities[eidx]
                    .player_id
                    .is_none_or(|pid| pid == sender_id);
            let controlled = scene.entities[eidx].direct_control || is_occupied_player_vehicle;

            let mut move_quest_pos = None;
            let mut mode_changed = false;
            if controlled {
                mode_changed = scene.player_movement_mode != last_move_replay.MovementMode;
                scene.player_movement_mode = last_move_replay.MovementMode;
                scene.has_player_movement_mode = true;
                for player in &mut scene.instance.players {
                    if player.id != sender_id {
                        continue;
                    }
                    player.location = next_location;
                    let dx = player.location[0] - scene.last_spatial_check_loc[0];
                    let dy = player.location[1] - scene.last_spatial_check_loc[1];
                    let dz = player.location[2] - scene.last_spatial_check_loc[2];
                    if dx * dx + dy * dy + dz * dz >= 1200.0 * 1200.0 {
                        scene.last_spatial_check_loc = player.location;
                        events.enqueue(Event::SpatialSpawnsCheck {
                            player_id: sender_id,
                        });
                    }
                    let mdx = player.location[0] - scene.last_quest_move_loc[0];
                    let mdy = player.location[1] - scene.last_quest_move_loc[1];
                    let mdz = player.location[2] - scene.last_quest_move_loc[2];
                    if !scene.has_quest_move_loc || mdx * mdx + mdy * mdy + mdz * mdz >= 50.0 * 50.0
                    {
                        move_quest_pos = Some(player.location);
                    }
                }
            }

            if let Some(pos_comp) = scene.entities[eidx].position.as_mut() {
                pos_comp.location = next_location;
                if let Some(rot) = last_move_replay.Rotation.as_ref() {
                    let r = [rot.Roll, rot.Pitch, rot.Yaw];
                    pos_comp.rotation = r;
                    if controlled {
                        for player in &mut scene.instance.players {
                            if player.id == sender_id {
                                player.rotation = r;
                            }
                        }
                    }
                }
            }
            let final_rot = scene.entities[eidx]
                .position
                .map_or([0.0, 0.0, 0.0], |p| p.rotation);
            (controlled, move_quest_pos, mode_changed, final_rot)
        };

        events.enqueue(Event::EntityMovement {
            player_id: sender_id,
            entity_net_id: data.EntityId,
            pos: next_location,
            rot: next_rot,
        });

        if controlled_by_player {
            let mut scene = state.scene.take();
            if let Some(sc) = scene.as_mut() {
                role_helper::heal_formation_if_near_nexus(
                    &mut state.conn,
                    sc,
                    &state.assets,
                    &mut state.player_components.role,
                    events,
                    &state.fs,
                    next_location,
                );
            }
            state.scene = scene;
        }

        if let Some(pos) = move_quest_pos {
            if let Some(scene) = state.scene.as_mut() {
                scene.last_quest_move_loc = pos;
                scene.has_quest_move_loc = true;
            }
            let mut qctx = state.quest_context(events);
            quest_engine::events::on_player_move(&mut qctx, pos[0], pos[1], pos[2])
                .map_err(|e| e.to_string())?;
        } else if mode_changed {
            let mut qctx = state.quest_context(events);
            if let Err(err) = quest_engine::events::on_condition_state_changed(&mut qctx) {
                tracing::warn!(target: "scene", "on_move_mode failed: {err}");
            }
        }
    }

    if let Some(coop) = state.coop_service.as_ref() {
        coop.broadcast_move_except(sender_id, moving_entities);
    }
    Ok(())
}

pub fn handle_apply_scene_client_entity_inc_id_request(
    state: &mut State,
    req: ApplySceneClientEntityIncIdRequest,
) -> Result<ApplySceneClientEntityIncIdResponse, String> {
    let Some(scene) = state.scene.as_mut() else {
        return Ok(ApplySceneClientEntityIncIdResponse::default());
    };
    let sender_id = state.player_id;
    let mut infos = Vec::with_capacity(req.EntityConfigId.len());
    for cfg_id in req.EntityConfigId {
        let found = scene.find_entity_net_id_by_config_id(cfg_id);
        let net_id = match found {
            Some(nid) => nid,
            None => {
                let net_id = scene.alloc_next_entity_id();
                let mut storage = EntityComponentStorage::new(
                    net_id,
                    ConfigComponent {
                        camp: 0,
                        state: EntityState::Default,
                        entity_type: EntityType::SceneEntity,
                        config_type: ConfigType::Template,
                        config_id: cfg_id,
                    },
                );
                storage.position = Some(PositionComponent {
                    location: [-10_000.0, -10_000.0, -10_000.0],
                    rotation: [0.0, 0.0, 0.0],
                });
                storage.player_id = Some(sender_id);
                storage.actor_visible = true;
                scene.spawn(storage);
                net_id
            }
        };
        infos.push(ClientEntityInfoPb {
            EntityConfigId: cfg_id,
            IsClientEntityConfig: true,
            DOLLARFy: Some(ClientEntityInfoPbDOLLARFyOneof::IncId(net_id)),
            WFy: Some(ClientEntityInfoPbWFyOneof::ServerRealExist(found.is_some())),
        });
    }
    Ok(ApplySceneClientEntityIncIdResponse {
        ErrorCode: 0,
        InstId: scene.instance_id,
        ClientEntityInfo: infos,
    })
}

fn normal_interact_request_error(
    state: &State,
    entity_net_id: i64,
    option_index: i32,
) -> Option<ErrorCode> {
    if option_index < 0 {
        return Some(ErrorCode::ErrInteractOptionGuidInvalid);
    }
    let scene = state.scene.as_ref()?;
    let config_id = match scene.find_config_id_by_net_id(entity_net_id) {
        Some(cid) => cid,
        None => return Some(ErrorCode::ErrSceneEntityNotExist),
    };
    let level_entity = state
        .assets
        .find_level_entity(scene.instance_id, i64::from(config_id))?;
    let mut components = level_entity.ComponentsData.clone();
    if let Some(tpl) = state.assets.find_template_config(&level_entity.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut components);
    }
    let interact = components.InteractComponent.as_ref()?;
    if interact.Disabled.unwrap_or(false) {
        return Some(ErrorCode::ErrInteractComponentNotExist);
    }
    let idx = usize::try_from(option_index).ok()?;
    if idx >= interact.Options.len() {
        return Some(ErrorCode::ErrInteractOptionGuidInvalid);
    }
    None
}

fn try_unlock_teleporter_for_interaction(state: &mut State, entity_id: i64) -> Result<bool, String> {
    let Some(scene) = state.scene.as_mut() else {
        return Ok(false);
    };
    let mut is_nexus_interaction = false;
    let target_config_id = scene.find_config_id_by_net_id(entity_id);
    if let Some(cid) = target_config_id
        && role_helper::is_resonance_nexus_config(&state.assets, i64::from(cid))
    {
        is_nexus_interaction = true;
    }

    let player_loc = scene.instance.players.first().map(|p| p.location);
    let map_id = state.assets.resolve_map_config_id(scene.instance_id);
    let mut changed = false;
    let mut newly_unlocked_tps = Vec::new();
    let mut quest_dirty = false;

    for &tp_idx in state.assets.indexes.teleporter_indices_for_map(map_id) {
        let tp = &state.assets.tables.teleporter.items[tp_idx as usize];
        let mut matched = target_config_id.is_some_and(|cid| i64::from(cid) == tp.TeleportEntityConfigId)
            || entity_id == tp.TeleportEntityConfigId;
        if !matched
            && target_config_id.is_none()
            && let Some(loc) = player_loc
            && let Some(cfg) = state
                .assets
                .find_level_entity(tp.MapId, tp.TeleportEntityConfigId)
            && let Some(tp_pos) = entity_world_pos(&cfg)
        {
            let dx = tp_pos[0] - loc[0];
            let dy = tp_pos[1] - loc[1];
            let dz = tp_pos[2] - loc[2];
            if dx * dx + dy * dy + dz * dz <= 800.0 * 800.0 {
                matched = true;
            }
        }
        if !matched {
            continue;
        }
        if role_helper::is_resonance_nexus_config(&state.assets, tp.TeleportEntityConfigId)
            || tp.Type == 1
        {
            is_nexus_interaction = true;
        }
        if state.player_components.map.info.unlock_teleporter(tp.Id) {
            changed = true;
            newly_unlocked_tps.push(tp.Id);
            if let Ok(cfg_id32) = i32::try_from(tp.TeleportEntityConfigId)
                && cfg_id32 > 0
            {
                state
                    .player_components
                    .quest
                    .info
                    .set_entity_state(tp.TeleportEntityConfigId, "关卡.Common.状态.激活");
                quest_dirty = true;
                if let Some(nid) = scene.find_entity_net_id_by_config_id(cfg_id32)
                    && let Some(upd) = scene.update_entity_state_tag(
                        &state.assets.tables.gameplay_tag_remap,
                        nid,
                        "关卡.Common.状态.激活",
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
            changed = true;
        }
    }

    if quest_dirty {
        state
            .player_components
            .quest
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    if changed {
        comp_util::save_struct(
            &state.fs,
            &state.player_components.map.info,
            &format!(
                "player/{}/{}",
                state.player_id,
                sonoro_gamesv_fs::map_info::MapInfo::DATA_PATH
            ),
        )
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

    Ok(is_nexus_interaction)
}

pub fn handle_entity_interact_request(
    state: &mut State,
    events: &mut EventQueue,
    req: EntityInteractRequest,
) -> Result<EntityInteractResponse, String> {
    if let Some(err_code) = normal_interact_request_error(state, req.EntityId, req.OptionIndex) {
        return Ok(EntityInteractResponse {
            ErrorCode: Some(err_code),
            Interacting: false,
        });
    }
    if try_unlock_teleporter_for_interaction(state, req.EntityId)? {
        let mut scene = state.scene.take();
        if let Some(sc) = scene.as_mut() {
            role_helper::revive_and_heal_formation(
                &mut state.conn,
                sc,
                &mut state.player_components.role,
                events,
                &state.fs,
            );
        }
        state.scene = scene;
    }

    let mut qctx = state.quest_context(events);
    let handled = quest_engine::events::on_interact_handled(&mut qctx, req.EntityId, req.OptionIndex)
        .map_err(|e| e.to_string())?;
    if !handled && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
        let _ = level_play::on_entity_interact_fallback(&mut lp_ctx, req.EntityId)
            .map_err(|e| e.to_string())?;
    }

    Ok(EntityInteractResponse {
        ErrorCode: Some(ErrorCode::Success),
        Interacting: false,
    })
}

pub fn handle_get_reward_treasure_box_request(
    state: &mut State,
    events: &mut EventQueue,
    req: GetRewardTreasureBoxRequest,
) -> Result<GetRewardTreasureBoxResponse, String> {
    let mut qctx = state.quest_context(events);
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
        level_play::on_get_reward_treasure_box(&mut lp_ctx, req.EntityId)
            .map_err(|e| e.to_string())?;
    }
    Ok(GetRewardTreasureBoxResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_animal_drop_request(
    state: &mut State,
    events: &mut EventQueue,
    req: AnimalDropRequest,
) -> Result<AnimalDropResponse, String> {
    let mut qctx = state.quest_context(events);
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
        level_play::on_animal_drop(&mut lp_ctx, req.EntityId).map_err(|e| e.to_string())?;
    }
    Ok(AnimalDropResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_animal_die_request(
    state: &mut State,
    events: &mut EventQueue,
    req: AnimalDieRequest,
) -> Result<AnimalDieResponse, String> {
    let mut qctx = state.quest_context(events);
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
        level_play::on_animal_drop(&mut lp_ctx, req.EntityId).map_err(|e| e.to_string())?;
    }
    Ok(AnimalDieResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_animal_destroy_request(
    state: &mut State,
    events: &mut EventQueue,
    req: AnimalDestroyRequest,
) -> Result<AnimalDestroyResponse, String> {
    let mut qctx = state.quest_context(events);
    if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
        level_play::on_animal_destroy(&mut lp_ctx, req.EntityId).map_err(|e| e.to_string())?;
    }
    Ok(AnimalDestroyResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_entity_dynamic_interact_request(
    state: &mut State,
    events: &mut EventQueue,
    req: EntityDynamicInteractRequest,
) -> Result<EntityDynamicInteractResponse, String> {
    let guid = String::from_utf8_lossy(&req.OptionGuid).into_owned();
    let dynamic_option = state
        .scene
        .as_ref()
        .and_then(|s| s.find_dynamic_interact_option(&guid).cloned());
    let Some(opt) = dynamic_option else {
        return Ok(EntityDynamicInteractResponse {
            ErrorCode: Some(ErrorCode::ErrInteractOptionGuidInvalid),
            Interacting: false,
        });
    };
    if opt.entity_net_id != req.EntityId || opt.kind == DynamicInteractKind::None {
        return Ok(EntityDynamicInteractResponse {
            ErrorCode: Some(ErrorCode::ErrInteractOptionGuidInvalid),
            Interacting: false,
        });
    }

    if try_unlock_teleporter_for_interaction(state, req.EntityId)? {
        let mut scene = state.scene.take();
        if let Some(sc) = scene.as_mut() {
            role_helper::revive_and_heal_formation(
                &mut state.conn,
                sc,
                &mut state.player_components.role,
                events,
                &state.fs,
            );
        }
        state.scene = scene;
    }

    let mut qctx = state.quest_context(events);
    let target_entity_id = if req.EntityId != 0 {
        req.EntityId
    } else {
        opt.entity_net_id
    };
    let mut handled = false;
    match opt.kind {
        DynamicInteractKind::Flow => {
            if !opt.flow_list_name.is_empty() && opt.flow_id > 0 {
                quest_engine::triggers::start_flow_params(
                    &mut qctx,
                    opt.quest_id,
                    opt.node_id,
                    &opt.flow_list_name,
                    opt.flow_id,
                    opt.state_id,
                    true,
                )
                .map_err(|e| e.to_string())?;
                if let Some(sc) = qctx.scene.as_deref_mut()
                    && sc.active_flow_inc_id != 0
                    && sc.active_flow_quest_id == opt.quest_id
                    && sc.active_flow_node_id == opt.node_id
                {
                    sc.active_flow_owner_entity_id = target_entity_id;
                }
                handled = true;
            } else if opt.bt_type == BtType::BtTypeLevelPlay
                && opt.quest_id > 0
                && opt.node_id > 0
                && let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx)
                && level_play::on_dynamic_interact(
                    &mut lp_ctx,
                    opt.quest_id,
                    opt.node_id,
                    target_entity_id,
                )
                .map_err(|e| e.to_string())?
            {
                handled = true;
            }
        }
        DynamicInteractKind::Actions => {
            if let Ok(acts) = serde_json::from_str::<Vec<serde_json::Value>>(&opt.actions_json)
                && !acts.is_empty()
            {
                let cfg_id = if opt.entity_config_id > 0 {
                    opt.entity_config_id
                } else {
                    qctx.scene
                        .as_deref()
                        .and_then(|s| s.find_config_id_by_net_id(target_entity_id))
                        .unwrap_or(0)
                };
                let entity_ctx = EntityCtxPb {
                    ConfigId: cfg_id,
                    IncId: target_entity_id,
                };
                // LevelGeneralContextUtil.CreateDynamicInteractContext reads
                // finalOptionCtx.CtxType unconditionally without a null check.
                let final_option_ctx = if opt.quest_id > 0 && opt.node_id > 0 {
                    GameCtxPb {
                        CtxType: Some(GameCtxType::DoInteractChildQuestNode),
                        CtxInfo: Some(GameCtxPbCtxInfoOneof::DoInteractChildQuestNode(
                            DoInteractChildQuestNodeCtxPb {
                                BehaviorTreeCtx: Some(BehaviorTreeCtxPb {
                                    IncId: i64::from(opt.quest_id),
                                    BtType: Some(opt.bt_type),
                                    BtId: opt.quest_id,
                                    NodeId: opt.node_id,
                                }),
                            },
                        )),
                    }
                } else {
                    GameCtxPb {
                        CtxType: Some(GameCtxType::Entity),
                        CtxInfo: Some(GameCtxPbCtxInfoOneof::Entity(entity_ctx.clone())),
                    }
                };
                quest_engine::triggers::push_entity_level_events(
                    &mut qctx,
                    GameCtxType::DynamicInteract,
                    GameCtxPbCtxInfoOneof::DynamicInteract(Box::new(DynamicInteractCtxPb {
                        EntityCtx: Some(entity_ctx),
                        OptionGuid: guid.as_bytes().to_vec(),
                        finalOptionCtx: Some(final_option_ctx),
                    })),
                    &acts,
                );
                let prev_owner = if let Some(sc) = qctx.scene.as_deref_mut() {
                    let prev = sc.active_flow_owner_entity_id;
                    sc.active_flow_owner_entity_id = target_entity_id;
                    prev
                } else {
                    0
                };
                let mut cbs = quest_engine::action_callbacks();
                let res = quest_actions::execute_action_list(
                    &mut qctx,
                    &mut cbs,
                    opt.quest_id,
                    opt.node_id,
                    &acts,
                );
                if let Some(sc) = qctx.scene.as_deref_mut() {
                    sc.active_flow_owner_entity_id = prev_owner;
                }
                res.map_err(|e| e.to_string())?;
            }
            if opt.bt_type == BtType::BtTypeLevelPlay && opt.quest_id > 0 && opt.node_id > 0 {
                if let Some(mut lp_ctx) = LevelPlayContext::from_quest_ctx(&mut qctx) {
                    let _ = level_play::on_dynamic_interact(
                        &mut lp_ctx,
                        opt.quest_id,
                        opt.node_id,
                        target_entity_id,
                    );
                }
                handled = true;
            } else if opt.quest_id > 0 && opt.node_id > 0 {
                if !quest_engine::triggers::complete_do_interact_from_dynamic_option(
                    &mut qctx,
                    opt.quest_id,
                    opt.node_id,
                    target_entity_id,
                    &guid,
                )
                .map_err(|e| e.to_string())?
                {
                    let _ = quest_engine::events::on_submit_node(
                        &mut qctx,
                        opt.quest_id,
                        i64::from(opt.quest_id),
                        opt.node_id,
                    )
                    .map_err(|e| e.to_string())?;
                }
                handled = true;
            }
        }
        DynamicInteractKind::None => {}
    }

    if !handled {
        let _ = quest_engine::events::on_interact_handled(&mut qctx, target_entity_id, 0)
            .map_err(|e| e.to_string())?;
    }

    Ok(EntityDynamicInteractResponse {
        ErrorCode: Some(ErrorCode::Success),
        Interacting: false,
    })
}

pub fn handle_entity_pos_abnormal_request(
    state: &mut State,
    req: EntityPosAbnormalRequest,
) -> Result<EntityPosAbnormalResponse, String> {
    if req.IsAbnormal
        && let Some(scene) = state.scene.as_mut()
        && let Some(cfg_id) = scene.find_config_id_by_net_id(req.EntityId)
        && cfg_id > 0
        && let Some(pos) = state
            .assets
            .find_level_entity(scene.instance_id, i64::from(cfg_id))
            .and_then(|cfg| entity_world_pos(&cfg))
    {
        let _ = scene.set_entity_position(req.EntityId, pos, None);
        let _ = state.conn.push_msg(
            EntityPosResetNotify::MSG_ID,
            &EntityPosResetNotify { EntityId: cfg_id },
        );
    }
    Ok(EntityPosAbnormalResponse {
        Code: Some(ErrorCode::Success),
    })
}
