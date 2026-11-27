use std::collections::BTreeMap;

use sonoro_gamesv_data::data_tables::TableRef;

use sonoro_gamesv_data::tables::entity_components::extra::ExtraEntry;
use sonoro_gamesv_logic::event_queue::EventQueue;
use sonoro_gamesv_logic::helpers::consume::CostItem;
use sonoro_gamesv_logic::helpers::level_play::LevelPlayEventKind;
use sonoro_gamesv_logic::helpers::quest_actions;
use sonoro_gamesv_logic::helpers::quest_engine;
use sonoro_gamesv_logic::helpers::reward::RewardItem;
use sonoro_gamesv_logic::helpers::{consume, energy, level_play, pb_kv, reward};
use sonoro_proto::enums::{BathCameraType, ConsumptiveTaskType, ErrorCode, GiantEcoNetStatus};
use sonoro_proto::pb::{
    ActivityBuyItem, ActivityFirstReadRequest, ActivityFirstReadResponse,
    ActivityPayShopBuyRequest, ActivityPayShopBuyResponse, ActivityPreOpenRequest,
    ActivityPreOpenResponse, ActivityResultRewardRequest, ActivityResultRewardResponse,
    AreaTerminalPinOperationRequest, AreaTerminalPinOperationResponse, ArtemisNodeUpdateRequest,
    ArtemisNodeUpdateResponse, AvignonRewardRequest, AvignonRewardResponse,
    BathChangeCameraRequest, BathChangeCameraResponse, BathPhotoRequest, BathPhotoResponse,
    BlackCoastThemeActiveRewardRequest, BlackCoastThemeActiveRewardResponse,
    BlackCoastThemeTaskRewardRequest, BlackCoastThemeTaskRewardResponse,
    BlackSwordChallengeInfo, BlackSwordChallengeInfoRequest, BlackSwordChallengeInfoResponse,
    BlackSwordChallengeMarkItemPanelInfoRequest, BlackSwordChallengeMarkItemPanelInfoResponse,
    BrochureRewardRequest, BrochureRewardResponse, BuildingInfo, BuildingLevelUpRequest,
    BuildingLevelUpResponse, BuildingUnLockRequest, BuildingUnLockResponse,
    CertificateLevelRewardRequest, CertificateLevelRewardResponse, CommitBoardRequest,
    CommitBoardResponse, CommitBoardSuccessNotify, ConstructTrapRequest, ConstructTrapResponse,
    ConsumptiveActivityInfo, ConsumptiveActivityInfoRequest, ConsumptiveActivityInfoResponse,
    ConsumptiveActivityScoreRequest, ConsumptiveActivityScoreResponse, ConsumptiveRewardRequest,
    ConsumptiveRewardResponse, ConsumptiveTaskInfo, ConsumptiveTaskProcess, ConsumptiveTaskReward,
    CorniceChallengeTransRequest, CorniceChallengeTransResponse, DailyAdventurePtRewardRequest,
    DailyAdventurePtRewardResponse, DailyAdventureTaskRewardRequest,
    DailyAdventureTaskRewardResponse, DarkCoastInfoRequest, DarkCoastInfoResponse,
    DeconstructTrapRequest, DeconstructTrapResponse, DragonPoolConf, DragonPoolConfRequest,
    DragonPoolConfResponse, DropCatchInfoRequest, DropCatchInfoResponse, DropCatchLevelFinishRequest,
    DropCatchLevelFinishResponse, DropCatchLevelInfo, DropCatchLevelRewardRequest,
    DropCatchLevelRewardResponse, EdgeRunnerFunctionPreUnlockRequest,
    EdgeRunnerFunctionPreUnlockResponse, EdgeRunnerScoreRewardRequest,
    EdgeRunnerScoreRewardResponse, EdgeRunnerTaskRewardRequest, EdgeRunnerTaskRewardResponse,
    EncircleChallengeCompleteRequest, EncircleChallengeCompleteResponse,
    EncircleChallengeStartRequest, EncircleChallengeStartResponse,
    EntityFoundationCoreChangeNotify, ExhibitionItemSaveRequest, ExhibitionItemSaveResponse,
    FarmGoldLevelDiffRequest, FarmGoldLevelDiffResponse, FarmGoldLevelPlayRepeatedRequest,
    FarmGoldLevelPlayRepeatedResponse, FarmGoldPointRepeatedRequest,
    FarmGoldPointRepeatedResponse, FarmGoldResultRequest, FarmGoldResultResponse,
    FeiXuePreheatInfoRewardRequest, FeiXuePreheatInfoRewardResponse, FixToolRequest,
    FixToolResponse, FlagChallengeRewardRequest, FlagChallengeRewardResponse,
    FlowerPollutionRequest, FlowerPollutionResponse, FlowerPollutionSplinePush, FoundationCoreInfo,
    FoundationCoreOperateRequest, FoundationCoreOperateResponse, FoundationRequest,
    FoundationResponse, FunPlayChallengeAwardRequest, FunPlayChallengeAwardResponse,
    FuncShowRequest, FuncShowResponse, GatherActivityRewardRequest, GatherActivityRewardResponse,
    GiantDeliveryItemRequest, GiantDeliveryItemResponse, GiantEcoInfoPb,
    GiantMaxLevelDeliveryItemRequest, GiantMaxLevelDeliveryItemResponse,
    GiantStableValueActivityRequest, GiantStableValueActivityResponse, GivebackRewardRequest,
    GivebackRewardResponse, GolemCrackUpdateRequest, GolemCrackUpdateResponse,
    H5CircumFluenceActivityReadRequest, H5CircumFluenceActivityReadResponse,
    H5ViewActivityReadRequest, H5ViewActivityReadResponse, HarvestActivityLevelDiffRequest,
    HarvestActivityLevelDiffResponse, HarvestActivityLevelRequest, HarvestActivityLevelResponse,
    HarvestActivityPointRequest, HarvestActivityPointResponse, HarvestActivityResultRequest,
    HarvestActivityResultResponse, InspirationUnlockChoiceRequest, InspirationUnlockChoiceResponse,
    InvestRequest, InvestResponse, LifePointChallengeStartRequest, LifePointChallengeStartResponse,
    LineCrossChallengeStartRequest, LineCrossChallengeStartResponse, LinkageCheckInRewardRequest,
    LinkageCheckInRewardResponse, LinkageRewardRequest, LinkageRewardResponse,
    LongShanScoreRewardRequest, LongShanScoreRewardResponse, LongShanTaskRewardRequest,
    LongShanTaskRewardResponse, MatchThreeEndRequest, MatchThreeEndResponse,
    MatchThreeStarRequest, MatchThreeStarResponse, MiniWriteLetterSaveContentRequest,
    MiniWriteLetterSaveContentResponse, MixDrinkRoleInviteRequest, MixDrinkRoleInviteResponse,
    MixDrinkRoleRewardRequest, MixDrinkRoleRewardResponse, MixDrinkSettleRequest,
    MixDrinkSettleResponse, MoonPhaseRandomRequest, MoonPhaseRandomResponse,
    MoonPhaseRewardRequest, MoonPhaseRewardResponse, MoonPhaseSelect, MovePlacementNotify,
    MowTowerLevelsRewardRequest, MowTowerLevelsRewardResponse, MowTowerResultNotify,
    MowTowerSettlementRequest, MowTowerSettlementResponse, MulCorniceMeetingRewardRequest,
    MulCorniceMeetingRewardResponse, OccupyRewardRequest, OccupyRewardResponse,
    OpenSystemBoardResultRequest, OpenSystemBoardResultResponse, ParkourChallenge,
    ParkourChallengeRequest, ParkourChallengeResponse, ParkourChallengeTakeRequest,
    ParkourChallengeTakeResponse, ParkourChallengeTransRequest, ParkourChallengeTransResponse,
    PhotoFightSettleRequest, PhotoFightSettleResponse, PhotoFightTargetPb,
    PhotoFightTaskRewardRequest, PhotoFightTaskRewardResponse, PhotoMemoryRewardRequest,
    PhotoMemoryRewardResponse, PlaceItemOnBoardRequest, PlaceItemOnBoardResponse,
    PreheatSignRewardRequest, PreheatSignRewardResponse, PreheatSignSurveyInfo,
    PreheatSignSurveyInfoRequest, PreheatSignSurveyInfoResponse, PullingFoundationRequest,
    PullingFoundationResponse, QingXiaoTaskRewardRequest, QingXiaoTaskRewardResponse,
    RenjuExitMatchedActionRequest, RenjuExitMatchedActionResponse, RenjuResetRequest,
    RenjuResetResponse, RiskHarvestInstRewardRequest, RiskHarvestInstRewardResponse,
    RiskHarvestScoreRewardRequest, RiskHarvestScoreRewardResponse, RiskHarvestSettleRequest,
    RiskHarvestSettleResponse, RiskHarvestStarRewardRequest, RiskHarvestStarRewardResponse,
    RoleProperSettle, RunTurntableActivityRequest, RunTurntableActivityResponse,
    ScheduleRewardRequest, ScheduleRewardResponse, ScratchCardActivityInfo,
    ScratchCardActivityInfoRequest, ScratchCardActivityInfoResponse, ScratchCardRewardRequest,
    ScratchCardRewardResponse, SignActivityFreeDropRequest, SignActivityFreeDropResponse,
    SignActivityRequest, SignActivityResponse, SingleGachaAwardPb, SunSpiritActionOperationEndPush,
    TetrisFinishRequest, TetrisFinishResponse, TetrisUpdateRequest, TetrisUpdateResponse,
    ThemePersonalScoreRequest, ThemePersonalScoreResponse, ThemeWorldScoreRequest,
    ThemeWorldScoreResponse, TimePointActivityBatchRewardRequest,
    TimePointActivityBatchRewardResponse, TimePointActivityRewardRequest,
    TimePointActivityRewardResponse, TraceMoonPhaseRewardRequest, TraceMoonPhaseRewardResponse,
    TrialRoleRewardRequest, TrialRoleRewardResponse, TrialRoleSkinRewardRequest,
    TrialRoleSkinRewardResponse, TurntableCompleteRequest, TurntableCompleteResponse,
    VacuumCleanerRequest, VacuumCleanerResponse, WuWuKujiGachaRequest, WuWuKujiGachaResponse,
    WuWuPackageRewardRequest, WuWuPackageRewardResponse, WuWuTaskRewardRequest,
    WuWuTaskRewardResponse, ZitherPlayNotePush, ZitherPlayStateRequest, ZitherPlayStateResponse,
};
use sonoro_proto::enums::ActivityTaskState;

use super::super::connection::drain_event_queue_best_effort;

use super::State;

fn claim_id_list(
    state: &mut State,
    prefix: &str,
    table: &str,
    ids: &[i32],
) -> Result<BTreeMap<i32, i32>, String> {
    const FALLBACK: [RewardItem; 2] = [
        RewardItem {
            item_id: 3,
            count: 40,
        },
        RewardItem {
            item_id: 2,
            count: 15000,
        },
    ];
    let mut granted_map = BTreeMap::new();
    for &id in ids {
        let key = format!("{prefix}:{id}");
        if state.player_components.progress.info.mark_claim(&key) {
            let mut tmp = Vec::new();
            let mut rctx = state.reward_context();
            reward::grant_table_reward(
                &mut rctx,
                table,
                i64::from(id),
                false,
                true,
                19500,
                &FALLBACK,
                Some(&mut tmp),
            )
            .map_err(|e| e.to_string())?;
            for it in tmp {
                *granted_map.entry(it.item_id).or_insert(0) += it.count;
            }
        }
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(granted_map)
}

fn claim_one(
    state: &mut State,
    prefix: &str,
    table: &str,
    id: i32,
) -> Result<BTreeMap<i32, i32>, String> {
    claim_id_list(state, prefix, table, &[id])
}

// =========================================================================
// General Activity, PayShop, Sign, Daily, Scratch, Turntable, WuWu
// =========================================================================

pub fn handle_activity_first_read_request(
    state: &mut State,
    req: ActivityFirstReadRequest,
) -> Result<ActivityFirstReadResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("act_read:{}", req.ActivityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(ActivityFirstReadResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_activity_pre_open_request(
    state: &mut State,
    req: ActivityPreOpenRequest,
) -> Result<ActivityPreOpenResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("act:preopen:{}", req.ActivityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(ActivityPreOpenResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_activity_pay_shop_buy_request(
    state: &mut State,
    req: ActivityPayShopBuyRequest,
) -> Result<ActivityPayShopBuyResponse, String> {
    let info = &mut state.player_components.progress.info;
    let out: Vec<ActivityBuyItem> = req.BuyItems;
    for bi in &out {
        info.add_key_int(&format!("act_shop:{}", bi.Id), i64::from(bi.Count));
    }
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(ActivityPayShopBuyResponse {
        ErrorCode: Some(ErrorCode::Success),
        BuyItems: out,
    })
}

pub fn handle_activity_result_reward_request(
    state: &mut State,
    req: ActivityResultRewardRequest,
) -> Result<ActivityResultRewardResponse, String> {
    let _ = claim_one(state, "act_res_rw", "ActivityReward", req.ResultId)?;
    Ok(ActivityResultRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
        TakeRewardResultIds: vec![req.ResultId],
    })
}

pub fn handle_sign_activity_request(
    state: &mut State,
    req: SignActivityRequest,
) -> Result<SignActivityResponse, String> {
    const FALLBACK: [RewardItem; 2] = [
        RewardItem {
            item_id: 3,
            count: 40,
        },
        RewardItem {
            item_id: 2,
            count: 15000,
        },
    ];
    let key = format!("sign_act:{}:{}", req.ActivityId, req.Index);
    let mut item_map = BTreeMap::new();
    if state.player_components.progress.info.mark_claim(&key) {
        let slot_item = if req.Index >= 0 {
            state
                .assets
                .reward_index
                .get_row("ActivitySign", i64::from(req.ActivityId))
                .and_then(|row| {
                    row.items
                        .get(req.Index as usize)
                        .filter(|p| p.item_id > 0 && p.count > 0)
                        .map(|p| RewardItem {
                            item_id: p.item_id,
                            count: p.count,
                        })
                })
        } else {
            None
        };
        let mut rctx = state.reward_context();
        if let Some(one) = slot_item {
            reward::grant_items(&mut rctx, 0, true, &[one], 19500).map_err(|e| e.to_string())?;
            item_map.insert(one.item_id, one.count);
        } else {
            let mut tmp = Vec::new();
            reward::grant_table_reward(
                &mut rctx,
                "ActivitySign",
                i64::from(req.Index),
                false,
                true,
                19500,
                &FALLBACK,
                Some(&mut tmp),
            )
            .map_err(|e| e.to_string())?;
            for it in tmp {
                *item_map.entry(it.item_id).or_insert(0) += it.count;
            }
        }
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(SignActivityResponse {
        ErrorCode: Some(ErrorCode::Success),
        ItemMap: item_map,
    })
}

pub fn handle_sign_activity_free_drop_request(
    state: &mut State,
    req: SignActivityFreeDropRequest,
) -> Result<SignActivityFreeDropResponse, String> {
    let _ = claim_one(state, "sign_free", "ActivitySign", req.ActivityId)?;
    Ok(SignActivityFreeDropResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_daily_adventure_pt_reward_request(
    state: &mut State,
    req: DailyAdventurePtRewardRequest,
) -> Result<DailyAdventurePtRewardResponse, String> {
    let _ = claim_one(state, "dadv_pt", "DailyAdventurePoint", req.Id)?;
    Ok(DailyAdventurePtRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_daily_adventure_task_reward_request(
    state: &mut State,
    req: DailyAdventureTaskRewardRequest,
) -> Result<DailyAdventureTaskRewardResponse, String> {
    let _ = claim_one(state, "dadv_task", "DailyAdventureTask", req.Id)?;
    Ok(DailyAdventureTaskRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_run_turntable_activity_request(
    state: &mut State,
    req: RunTurntableActivityRequest,
) -> Result<RunTurntableActivityResponse, String> {
    let spin_key = format!("turntable:spin:{}", req.ActivityId);
    let roll = state
        .player_components
        .progress
        .info
        .add_key_int(&spin_key, 1);
    let round = (roll - 1).div_euclid(8) + 1;
    let rw_id = ((roll - 1).rem_euclid(8) + 1) as i32;
    let rw_prefix = format!("turntable_rw:{}:{round}", req.ActivityId);
    let item_map = claim_one(state, &rw_prefix, "TurntableAwards", rw_id)?;
    Ok(RunTurntableActivityResponse {
        ErrorCode: Some(ErrorCode::Success),
        IsFinishCurRound: roll.rem_euclid(8) == 0,
        RewardId: rw_id,
        ItemMap: item_map,
    })
}

pub fn handle_turntable_complete_request(
    state: &mut State,
    req: TurntableCompleteRequest,
) -> Result<TurntableCompleteResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("turntable:done:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(TurntableCompleteResponse {
        ErrCode: Some(ErrorCode::Success),
        EntityId: req.EntityId,
    })
}

pub fn handle_scratch_card_activity_info_request(
    state: &mut State,
    req: ScratchCardActivityInfoRequest,
) -> Result<ScratchCardActivityInfoResponse, String> {
    let key = format!("scratch:info:{}", req.ActivityId);
    let info = pb_kv::load_msg::<ScratchCardActivityInfo>(
        &state.player_components.progress,
        &key,
    )
    .unwrap_or_default();
    Ok(ScratchCardActivityInfoResponse {
        Error: Some(ErrorCode::Success),
        Info: Some(info),
    })
}

pub fn handle_scratch_card_reward_request(
    state: &mut State,
    req: ScratchCardRewardRequest,
) -> Result<ScratchCardRewardResponse, String> {
    let key = format!("scratch:{}:{}", req.RoundId, req.Index);
    let item_map = claim_one(state, &key, "ScratchCard", req.Index)?;
    Ok(ScratchCardRewardResponse {
        ItemMap: item_map,
        OpenType: 1,
        Error: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_wu_wu_kuji_gacha_request(
    state: &mut State,
    _req: WuWuKujiGachaRequest,
) -> Result<WuWuKujiGachaResponse, String> {
    let spin = state
        .player_components
        .progress
        .info
        .add_key_int("wuwu:spin", 1) as i32;
    let item_map = claim_one(state, "wuwu_gacha", "WuWuKuji", spin)?;
    Ok(WuWuKujiGachaResponse {
        TrickType: 1,
        ErrorCode: Some(ErrorCode::Success),
        Awards: vec![SingleGachaAwardPb {
            GroupId: 1,
            Times: 1,
            ItemMap: item_map,
        }],
    })
}

pub fn handle_wu_wu_package_reward_request(
    state: &mut State,
    req: WuWuPackageRewardRequest,
) -> Result<WuWuPackageRewardResponse, String> {
    let _ = claim_one(state, "wuwu_pkg", "WuWuTaskPackage", req.WrapId)?;
    Ok(WuWuPackageRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_wu_wu_task_reward_request(
    state: &mut State,
    req: WuWuTaskRewardRequest,
) -> Result<WuWuTaskRewardResponse, String> {
    let _ = claim_id_list(state, "wuwu_task", "WuWuWeekTask", &req.Ids)?;
    Ok(WuWuTaskRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_consumptive_activity_info_request(
    state: &mut State,
    _req: ConsumptiveActivityInfoRequest,
) -> Result<ConsumptiveActivityInfoResponse, String> {
    Ok(ConsumptiveActivityInfoResponse {
        ConsumptiveActivityInfo: Some(ConsumptiveActivityInfo {
            TotalScore: state
                .player_components
                .progress
                .info
                .get_key_int("cons:score") as i32,
            ..Default::default()
        }),
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_consumptive_activity_score_request(
    state: &mut State,
    _req: ConsumptiveActivityScoreRequest,
) -> Result<ConsumptiveActivityScoreResponse, String> {
    Ok(ConsumptiveActivityScoreResponse {
        ErrorCode: Some(ErrorCode::Success),
        TotalScore: state
            .player_components
            .progress
            .info
            .get_key_int("cons:score") as i32,
    })
}

pub fn handle_consumptive_reward_request(
    state: &mut State,
    req: ConsumptiveRewardRequest,
) -> Result<ConsumptiveRewardResponse, String> {
    let _ = claim_id_list(state, "cons_task", "ConsumptiveTask", &req.TaskIds)?;
    let task_infos = req
        .TaskIds
        .into_iter()
        .map(|tid| {
            let cfg = state.assets.tables.consumptive_task.get_data_by_id(&tid);
            let target = cfg.as_ref().map(|c| c.MaxFinishCount.max(1)).unwrap_or(1);
            let ttype = match cfg.as_ref().map(|c| c.TaskTab) {
                Some(2) => ConsumptiveTaskType::Cycle,
                _ => ConsumptiveTaskType::Single,
            };
            ConsumptiveTaskInfo {
                Id: tid,
                Type: Some(ttype),
                Process: Some(ConsumptiveTaskProcess {
                    Current: target,
                    Target: target,
                }),
                Reward: Some(ConsumptiveTaskReward {
                    Rewarded: 1,
                    WaitReward: 0,
                    MaxReward: target,
                }),
            }
        })
        .collect();
    Ok(ConsumptiveRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
        TaskInfos: task_infos,
        ..Default::default()
    })
}

pub fn handle_preheat_sign_survey_info_request(
    state: &mut State,
    req: PreheatSignSurveyInfoRequest,
) -> Result<PreheatSignSurveyInfoResponse, String> {
    let answered = state
        .player_components
        .progress
        .info
        .has_claim(&format!("preheat:{}", req.PreheatNodeId));
    Ok(PreheatSignSurveyInfoResponse {
        Error: Some(ErrorCode::Success),
        PreheatSignSurveyInfo: Some(PreheatSignSurveyInfo {
            SupportNum: 100,
            OpposeNum: 10,
            AnswerStatus: answered,
        }),
    })
}

pub fn handle_preheat_sign_reward_request(
    state: &mut State,
    req: PreheatSignRewardRequest,
) -> Result<PreheatSignRewardResponse, String> {
    let key = format!("preheat:{}", req.PreheatNodeId);
    let _ = claim_one(state, &key, "PreheatSign", req.PreheatNodeId)?;
    Ok(PreheatSignRewardResponse {
        Error: Some(ErrorCode::Success),
        PreheatSignSurveyInfo: Some(PreheatSignSurveyInfo {
            SupportNum: 101,
            OpposeNum: 10,
            AnswerStatus: req.AnswerStatus,
        }),
    })
}

pub fn handle_h5_circum_fluence_activity_read_request(
    state: &mut State,
    _req: H5CircumFluenceActivityReadRequest,
) -> Result<H5CircumFluenceActivityReadResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        "h5:cf_read",
    )
    .map_err(|e| e.to_string())?;
    Ok(H5CircumFluenceActivityReadResponse::default())
}

pub fn handle_h5_view_activity_read_request(
    state: &mut State,
    req: H5ViewActivityReadRequest,
) -> Result<H5ViewActivityReadResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("h5:view_read:{}", req.ActivityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(H5ViewActivityReadResponse::default())
}

// =========================================================================
// Theme, Linkage, Coast, Festival, Building & Collection Events
// =========================================================================

pub fn handle_artemis_node_update_request(
    state: &mut State,
    req: ArtemisNodeUpdateRequest,
) -> Result<ArtemisNodeUpdateResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("artemis:{}:{}", req.ActivityId, req.Index),
    )
    .map_err(|e| e.to_string())?;
    Ok(ArtemisNodeUpdateResponse {
        Error: Some(ErrorCode::Success),
    })
}

pub fn handle_avignon_reward_request(
    state: &mut State,
    req: AvignonRewardRequest,
) -> Result<AvignonRewardResponse, String> {
    let _ = claim_one(state, "avignon_rw", "Avignon", req.TaskId)?;
    Ok(AvignonRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_black_coast_theme_active_reward_request(
    state: &mut State,
    req: BlackCoastThemeActiveRewardRequest,
) -> Result<BlackCoastThemeActiveRewardResponse, String> {
    let _ = claim_id_list(
        state,
        "bc_act_rw",
        "BlackCoastThemeRewardRe",
        &req.ActiveRewardIds,
    )?;
    Ok(BlackCoastThemeActiveRewardResponse {
        Error: Some(ErrorCode::Success),
        ActiveRewardedIds: req.ActiveRewardIds,
    })
}

pub fn handle_black_coast_theme_task_reward_request(
    state: &mut State,
    req: BlackCoastThemeTaskRewardRequest,
) -> Result<BlackCoastThemeTaskRewardResponse, String> {
    let _ = claim_one(
        state,
        "bc_task_rw",
        "BlackCoastThemeTaskRe",
        req.TaskId,
    )?;
    Ok(BlackCoastThemeTaskRewardResponse {
        Error: Some(ErrorCode::Success),
    })
}

fn collect_black_sword_list(state: &State) -> Vec<BlackSwordChallengeInfo> {
    state
        .player_components
        .progress
        .info
        .dynamic
        .ints
        .iter()
        .filter_map(|entry| {
            let lp_id = entry.key.strip_prefix("bsword:lp:")?.parse::<i32>().ok()?;
            Some(BlackSwordChallengeInfo {
                LevelPlayId: lp_id,
                IsUnLocked: true,
                CompleteNumber: entry.val as i32,
            })
        })
        .collect()
}

pub fn handle_black_sword_challenge_info_request(
    state: &mut State,
    req: BlackSwordChallengeInfoRequest,
) -> Result<BlackSwordChallengeInfoResponse, String> {
    Ok(BlackSwordChallengeInfoResponse {
        LevelPlayUiInfoList: collect_black_sword_list(state),
        ErrorCode: Some(ErrorCode::Success),
        ChallengeId: req.ChallengeId,
    })
}

pub fn handle_black_sword_challenge_mark_item_panel_info_request(
    state: &mut State,
    req: BlackSwordChallengeMarkItemPanelInfoRequest,
) -> Result<BlackSwordChallengeMarkItemPanelInfoResponse, String> {
    Ok(BlackSwordChallengeMarkItemPanelInfoResponse {
        LevelPlayUiInfoList: collect_black_sword_list(state),
        ErrorCode: Some(ErrorCode::Success),
        ChallengeId: req.ChallengeId,
    })
}

pub fn handle_brochure_reward_request(
    state: &mut State,
    req: BrochureRewardRequest,
) -> Result<BrochureRewardResponse, String> {
    let _ = claim_id_list(state, "brochure", "Brochure", &req.BookItems)?;
    Ok(BrochureRewardResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_building_un_lock_request(
    state: &mut State,
    req: BuildingUnLockRequest,
) -> Result<BuildingUnLockResponse, String> {
    let info = &mut state.player_components.progress.info;
    let key = format!("bld:lv:{}", req.BuildingId);
    if info.get_key_int(&key) == 0 {
        info.set_key_int(&key, 1);
    }
    let like = info.add_key_int("bld:like", 10) as i32;
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(BuildingUnLockResponse {
        ErrorCode: Some(ErrorCode::Success),
        PeopleLike: like,
    })
}

pub fn handle_building_level_up_request(
    state: &mut State,
    req: BuildingLevelUpRequest,
) -> Result<BuildingLevelUpResponse, String> {
    let info = &mut state.player_components.progress.info;
    let lv = info.add_key_int(&format!("bld:lv:{}", req.BuildingId), 1) as i32;
    let like = info.add_key_int("bld:like", 15) as i32;
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(BuildingLevelUpResponse {
        BuildingInfo: Some(BuildingInfo {
            BuildingId: req.BuildingId,
            Level: lv,
            IsUnlock: true,
        }),
        ErrorCode: Some(ErrorCode::Success),
        PeopleLike: like,
    })
}

pub fn handle_certificate_level_reward_request(
    state: &mut State,
    _req: CertificateLevelRewardRequest,
) -> Result<CertificateLevelRewardResponse, String> {
    let _ = claim_one(state, "cert_lv_rw", "CertificateReward", 1)?;
    Ok(CertificateLevelRewardResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_cornice_challenge_trans_request(
    state: &mut State,
    req: CorniceChallengeTransRequest,
) -> Result<CorniceChallengeTransResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "cornice:cur_lp",
        i64::from(req.LevelPlayId),
    )
    .map_err(|e| e.to_string())?;
    Ok(CorniceChallengeTransResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_mul_cornice_meeting_reward_request(
    state: &mut State,
    req: MulCorniceMeetingRewardRequest,
) -> Result<MulCorniceMeetingRewardResponse, String> {
    for entry in req.CorniceMeetingRewardScoreLists.values() {
        let _ = claim_id_list(state, "cornice_rw", "CorniceMeeting", &entry.ScoreIndexes)?;
    }
    Ok(MulCorniceMeetingRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_dark_coast_info_request(
    state: &mut State,
    req: DarkCoastInfoRequest,
) -> Result<DarkCoastInfoResponse, String> {
    let pool_id = if req.DragonPoolId > 0 {
        req.DragonPoolId
    } else {
        3
    };
    let pc = &state.player_components.progress.info;
    let mut drop_ids = Vec::new();
    let mut level_gains = pc.get_key_int(&format!("mingsu:lvl:{pool_id}")).max(0) as i32;
    if let Some(pool) = state.assets.tables.dragon_pool.get_data_by_id(&pool_id) {
        drop_ids.extend_from_slice(&pool.DropIds);
        for lvl in 1..=(pool.DropIds.len() as i32) {
            if pc.has_claim(&format!("mingsu:clm:{pool_id}:{lvl}")) && lvl > level_gains {
                level_gains = lvl;
            }
        }
    }
    let mut guard_ids = Vec::new();
    let mut box_done = Vec::new();
    for dc in &state.assets.tables.dark_coast_delivery.items {
        if pc.has_claim(&format!("darkcoast:guard:{}", dc.Id))
            || pc.get_key_int(&format!("lp_state:{}", dc.LevelPlayId)) >= 3
        {
            guard_ids.push(dc.Id);
        }
        if pc.has_claim(&format!("darkcoast:box:{}", dc.Id)) {
            box_done.push(dc.Id);
        }
    }
    Ok(DarkCoastInfoResponse {
        DragonPoolConf: Some(DragonPoolConf {
            DragonPoolId: pool_id,
            DropIds: drop_ids,
            ..Default::default()
        }),
        GuardIds: guard_ids,
        TreasureBoxDone: box_done,
        LevelGains: level_gains,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_dragon_pool_conf_request(
    _state: &mut State,
    req: DragonPoolConfRequest,
) -> Result<DragonPoolConfResponse, String> {
    Ok(DragonPoolConfResponse {
        DragonPoolConf: Some(DragonPoolConf {
            DragonPoolId: req.DragonPoolId,
            ..Default::default()
        }),
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_edge_runner_function_pre_unlock_request(
    state: &mut State,
    req: EdgeRunnerFunctionPreUnlockRequest,
) -> Result<EdgeRunnerFunctionPreUnlockResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("edgerunner:fn:{}", req.FuncId),
    )
    .map_err(|e| e.to_string())?;
    Ok(EdgeRunnerFunctionPreUnlockResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_edge_runner_score_reward_request(
    state: &mut State,
    req: EdgeRunnerScoreRewardRequest,
) -> Result<EdgeRunnerScoreRewardResponse, String> {
    let _ = claim_one(state, "er_score", "EdgeRunnerReward", req.ScoreId)?;
    Ok(EdgeRunnerScoreRewardResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_edge_runner_task_reward_request(
    state: &mut State,
    req: EdgeRunnerTaskRewardRequest,
) -> Result<EdgeRunnerTaskRewardResponse, String> {
    let _ = claim_id_list(state, "er_task", "EdgeRunnerReward", &req.TaskIds)?;
    Ok(EdgeRunnerTaskRewardResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_fei_xue_preheat_info_reward_request(
    state: &mut State,
    req: FeiXuePreheatInfoRewardRequest,
) -> Result<FeiXuePreheatInfoRewardResponse, String> {
    let _ = claim_id_list(
        state,
        "feixue",
        "FeiXuePreheat",
        &req.FeiXuePreheatInfoIds,
    )?;
    Ok(FeiXuePreheatInfoRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_flag_challenge_reward_request(
    state: &mut State,
    req: FlagChallengeRewardRequest,
) -> Result<FlagChallengeRewardResponse, String> {
    let _ = claim_id_list(
        state,
        "flag_rw",
        "FlagChallenge",
        &req.FlagChallengeTaskIds,
    )?;
    Ok(FlagChallengeRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_fun_play_challenge_award_request(
    state: &mut State,
    req: FunPlayChallengeAwardRequest,
) -> Result<FunPlayChallengeAwardResponse, String> {
    let _ = claim_one(state, "funplay_rw", "FunPlay", req.ChallengeId)?;
    Ok(FunPlayChallengeAwardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_func_show_request(
    state: &mut State,
    req: FuncShowRequest,
) -> Result<FuncShowResponse, String> {
    let mut dirty = false;
    for fid in req.FuncId {
        if state
            .player_components
            .progress
            .info
            .mark_claim(&format!("func:shown:{fid}"))
        {
            dirty = true;
        }
    }
    if dirty {
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    Ok(FuncShowResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_gather_activity_reward_request(
    state: &mut State,
    req: GatherActivityRewardRequest,
) -> Result<GatherActivityRewardResponse, String> {
    let _ = claim_one(state, "gather_rw", "GatherActivity", req.GatherTaskId)?;
    Ok(GatherActivityRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

struct GiantDeliveryResult {
    err: Option<ErrorCode>,
    level: i32,
    stable_value: i32,
    status: GiantEcoNetStatus,
    top_quality: i32,
    has_reward: bool,
}

fn giant_level(goal: &[i32], stable_value: i32) -> i32 {
    for (i, &g) in goal.iter().enumerate() {
        if stable_value < g {
            return (i + 1) as i32;
        }
    }
    (goal.len() + 1) as i32
}

fn giant_status(max_stable: i32, goal_len: usize, level: i32, stable_value: i32) -> GiantEcoNetStatus {
    if stable_value >= max_stable {
        GiantEcoNetStatus::MaxStableValue
    } else if level as usize > goal_len {
        GiantEcoNetStatus::MaxLevel
    } else {
        GiantEcoNetStatus::Noraml
    }
}

fn deliver_giant_items(
    state: &mut State,
    eco_type: i32,
    items: &BTreeMap<i32, i32>,
    max_level_delivery: bool,
) -> Result<GiantDeliveryResult, String> {
    let Some(table) = state
        .assets
        .tables
        .giant_delivery_reward
        .get_data_by_id(&eco_type)
    else {
        return Ok(GiantDeliveryResult {
            err: Some(ErrorCode::ErrGiantItemInvaild),
            level: 1,
            stable_value: 0,
            status: GiantEcoNetStatus::Noraml,
            top_quality: 0,
            has_reward: false,
        });
    };
    let max_stable_value = table.MaxStableValue;
    let goals = table.Goal.clone();
    let item_ids = table.ItemIds.clone();
    let max_delivery_count = table.MaxDeliveryCount;
    let max_level_drop_id = table.MaxLevelDropId;
    let drop_ids = table.DropIds.clone();
    drop(table);
    let key = format!("giant:stable:{eco_type}");
    let old_stable = state
        .player_components
        .progress
        .info
        .get_key_int(&key)
        .clamp(0, i64::from(max_stable_value)) as i32;
    let old_level = giant_level(&goals, old_stable);
    let err_res = |e: ErrorCode| GiantDeliveryResult {
        err: Some(e),
        level: 1,
        stable_value: 0,
        status: GiantEcoNetStatus::Noraml,
        top_quality: 0,
        has_reward: false,
    };
    if max_level_delivery && (old_level as usize) <= goals.len() {
        return Ok(err_res(ErrorCode::ErrGiantNotMaxLevel));
    }
    if !max_level_delivery && (old_level as usize) > goals.len() {
        return Ok(err_res(ErrorCode::ErrGiantMaxLevel));
    }

    let mut costs = Vec::new();
    let mut quality_counts = [0i32; 8];
    let mut total_add = 0i64;
    let mut top_quality = 0i32;
    for (&item_id, &count) in items {
        if item_id <= 0 || count <= 0 {
            return Ok(err_res(ErrorCode::ErrGiantItemInvaild));
        }
        if !item_ids.contains(&item_id) {
            return Ok(err_res(ErrorCode::ErrGiantItemNotMatch));
        }
        let Some(item) = state.assets.tables.giant_item.get_data_by_id(&item_id) else {
            return Ok(err_res(ErrorCode::ErrGiantItemInvaild));
        };
        if item.GiantEcoType != eco_type {
            return Ok(err_res(ErrorCode::ErrGiantItemNotMatch));
        }
        if state
            .player_components
            .inventory
            .info
            .normal_item_count(item_id)
            < count
        {
            return Ok(err_res(ErrorCode::ErrGiantItemNotEnough));
        }
        if !max_level_delivery {
            let Ok(q_idx) = usize::try_from(item.Quality) else {
                return Ok(err_res(ErrorCode::ErrGiantItemInvaild));
            };
            if q_idx >= quality_counts.len() {
                return Ok(err_res(ErrorCode::ErrGiantItemInvaild));
            }
            quality_counts[q_idx] += count;
            if quality_counts[q_idx] > max_delivery_count {
                return Ok(err_res(ErrorCode::ErrGiantItemInvaild));
            }
        }
        top_quality = top_quality.max(item.Quality);
        total_add += i64::from(item.AddStableValue) * i64::from(count);
        costs.push(CostItem { id: item_id, count });
    }
    if costs.is_empty() {
        return Ok(err_res(ErrorCode::ErrGiantItemInvaild));
    }

    let new_stable = (i64::from(old_stable) + total_add).min(i64::from(max_stable_value)) as i32;
    if new_stable <= old_stable {
        return Ok(err_res(ErrorCode::ErrGiantMaxLevel));
    }
    let consumed = consume::consume_costs(
        &mut state.conn,
        &state.fs,
        &mut state.player_components.basic,
        &mut state.player_components.inventory,
        &costs,
    )
    .map_err(|e| e.to_string())?;
    if !consumed {
        return Ok(err_res(ErrorCode::ErrGiantItemNotEnough));
    }

    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &key,
        i64::from(new_stable),
    )
    .map_err(|e| e.to_string())?;

    let mut has_reward = false;
    let mut rctx = state.reward_context();
    if max_level_delivery {
        if max_level_drop_id > 0 {
            let grant = reward::grant_drop_package_popup(&mut rctx, max_level_drop_id, 19500)
                .map_err(|e| e.to_string())?;
            has_reward = grant.granted_count > 0;
        }
    } else {
        for (i, &goal) in goals.iter().enumerate() {
            if old_stable < goal
                && new_stable >= goal
                && let Some(&drop_id) = drop_ids.get(i)
                && drop_id > 0
            {
                let grant = reward::grant_drop_package_popup(&mut rctx, drop_id, 19500)
                    .map_err(|e| e.to_string())?;
                has_reward = has_reward || grant.granted_count > 0;
            }
        }
    }

    let level = giant_level(&goals, new_stable);
    Ok(GiantDeliveryResult {
        err: None,
        level,
        stable_value: new_stable,
        status: giant_status(max_stable_value, goals.len(), level, new_stable),
        top_quality,
        has_reward,
    })
}

pub fn handle_giant_delivery_item_request(
    state: &mut State,
    req: GiantDeliveryItemRequest,
) -> Result<GiantDeliveryItemResponse, String> {
    let Some(eco_type) = req.GiantEcoType else {
        return Ok(GiantDeliveryItemResponse {
            ErrorCode: Some(ErrorCode::ErrGiantItemInvaild),
            ..Default::default()
        });
    };
    let result = deliver_giant_items(state, eco_type as i32, &req.ItemInfos, false)?;
    if let Some(err) = result.err {
        return Ok(GiantDeliveryItemResponse {
            ErrorCode: Some(err),
            ..Default::default()
        });
    }
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(GiantDeliveryItemResponse {
        HasReward: result.has_reward,
        ErrorCode: Some(ErrorCode::Success),
        TopQuality: result.top_quality,
        GiantEcoInfo: Some(GiantEcoInfoPb {
            GiantEcoType: Some(eco_type),
            Level: result.level,
            StableValue: result.stable_value,
            Status: Some(result.status),
        }),
    })
}

pub fn handle_giant_max_level_delivery_item_request(
    state: &mut State,
    req: GiantMaxLevelDeliveryItemRequest,
) -> Result<GiantMaxLevelDeliveryItemResponse, String> {
    let Some(eco_type) = req.GiantEcoType else {
        return Ok(GiantMaxLevelDeliveryItemResponse {
            ErrorCode: Some(ErrorCode::ErrGiantItemInvaild),
            ..Default::default()
        });
    };
    let result = deliver_giant_items(state, eco_type as i32, &req.ItemInfos, true)?;
    if let Some(err) = result.err {
        return Ok(GiantMaxLevelDeliveryItemResponse {
            ErrorCode: Some(err),
            ..Default::default()
        });
    }
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(GiantMaxLevelDeliveryItemResponse {
        GiantEcoInfo: Some(GiantEcoInfoPb {
            GiantEcoType: Some(eco_type),
            Level: result.level,
            StableValue: result.stable_value,
            Status: Some(result.status),
        }),
        TopQuality: result.top_quality,
        ErrorCode: Some(ErrorCode::Success),
        HasReward: result.has_reward,
    })
}

pub fn handle_giant_stable_value_activity_request(
    state: &mut State,
    req: GiantStableValueActivityRequest,
) -> Result<GiantStableValueActivityResponse, String> {
    let _ = claim_id_list(state, "giant_sv", "GiantActivity", &req.RewardIds)?;
    Ok(GiantStableValueActivityResponse {
        ErrorCode: Some(ErrorCode::Success),
        GotRewardIds: req.RewardIds,
    })
}

pub fn handle_giveback_reward_request(
    state: &mut State,
    req: GivebackRewardRequest,
) -> Result<GivebackRewardResponse, String> {
    if req.RewardId != 0 {
        let _ = claim_one(state, "giveback_rw", "Giveback", req.RewardId)?;
    }
    let _ = claim_id_list(state, "giveback_rw", "Giveback", &req.RwdId)?;
    Ok(GivebackRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_linkage_check_in_reward_request(
    state: &mut State,
    req: LinkageCheckInRewardRequest,
) -> Result<LinkageCheckInRewardResponse, String> {
    let _ = claim_id_list(state, "link_ci", "LinkageCheckIn", &req.TaskIds)?;
    Ok(LinkageCheckInRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_linkage_reward_request(
    state: &mut State,
    req: LinkageRewardRequest,
) -> Result<LinkageRewardResponse, String> {
    let _ = claim_one(state, "link_rw", "ActivityLinkage", req.ActivityPageId)?;
    Ok(LinkageRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_long_shan_score_reward_request(
    state: &mut State,
    req: LongShanScoreRewardRequest,
) -> Result<LongShanScoreRewardResponse, String> {
    let _ = claim_id_list(state, "longshan_sc", "LongShanScoreReward", &req.Ids)?;
    Ok(LongShanScoreRewardResponse {
        Error: Some(ErrorCode::Success),
        ScoreRewardedIds: req.Ids,
    })
}

pub fn handle_long_shan_task_reward_request(
    state: &mut State,
    req: LongShanTaskRewardRequest,
) -> Result<LongShanTaskRewardResponse, String> {
    let _ = claim_id_list(state, "longshan_task", "LongShanTask", &req.TaskIds)?;
    Ok(LongShanTaskRewardResponse {
        Error: Some(ErrorCode::Success),
    })
}

pub fn handle_moon_phase_random_request(
    _state: &mut State,
    _req: MoonPhaseRandomRequest,
) -> Result<MoonPhaseRandomResponse, String> {
    let now_s = energy::now_seconds().max(1);
    Ok(MoonPhaseRandomResponse {
        Result: Some(MoonPhaseSelect {
            MoonPhaseId: ((now_s % 6) + 1) as i32,
            Label1: 1,
            Label2: 2,
        }),
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_moon_phase_reward_request(
    state: &mut State,
    _req: MoonPhaseRewardRequest,
) -> Result<MoonPhaseRewardResponse, String> {
    let _ = claim_one(state, "moon_phase_rw", "MoonPhase", 1)?;
    Ok(MoonPhaseRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_trace_moon_phase_reward_request(
    state: &mut State,
    req: TraceMoonPhaseRewardRequest,
) -> Result<TraceMoonPhaseRewardResponse, String> {
    let _ = claim_one(state, "trace_mp_rw", "TrackMoonPhase", req.ActivityId)?;
    Ok(TraceMoonPhaseRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_occupy_reward_request(
    state: &mut State,
    req: OccupyRewardRequest,
) -> Result<OccupyRewardResponse, String> {
    let _ = claim_id_list(state, "occupy_rw", "OccupyReward", &req.Ids)?;
    Ok(OccupyRewardResponse {
        Ids: req.Ids,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_qing_xiao_task_reward_request(
    state: &mut State,
    req: QingXiaoTaskRewardRequest,
) -> Result<QingXiaoTaskRewardResponse, String> {
    let _ = claim_id_list(state, "qingxiao_rw", "QingXiao", &req.TaskIds)?;
    Ok(QingXiaoTaskRewardResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_schedule_reward_request(
    state: &mut State,
    req: ScheduleRewardRequest,
) -> Result<ScheduleRewardResponse, String> {
    let _ = claim_one(state, "sched_rw", "ActivityReward", req.RewardId)?;
    Ok(ScheduleRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_theme_personal_score_request(
    state: &mut State,
    req: ThemePersonalScoreRequest,
) -> Result<ThemePersonalScoreResponse, String> {
    let _ = claim_id_list(state, "theme_ps", "ThemeCelebration", &req.Ids)?;
    Ok(ThemePersonalScoreResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_theme_world_score_request(
    state: &mut State,
    req: ThemeWorldScoreRequest,
) -> Result<ThemeWorldScoreResponse, String> {
    let _ = claim_id_list(state, "theme_ws", "ThemeCelebration", &req.Ids)?;
    Ok(ThemeWorldScoreResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_time_point_activity_reward_request(
    state: &mut State,
    req: TimePointActivityRewardRequest,
) -> Result<TimePointActivityRewardResponse, String> {
    let _ = claim_one(state, "tpa_rw", "TimePointReward", req.Id)?;
    Ok(TimePointActivityRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_time_point_activity_batch_reward_request(
    state: &mut State,
    req: TimePointActivityBatchRewardRequest,
) -> Result<TimePointActivityBatchRewardResponse, String> {
    let _ = claim_id_list(state, "tpa_brw", "TimePointReward", &req.Ids)?;
    Ok(TimePointActivityBatchRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_trial_role_reward_request(
    state: &mut State,
    req: TrialRoleRewardRequest,
) -> Result<TrialRoleRewardResponse, String> {
    let _ = claim_one(state, "trial_role_rw", "RoleTrial", req.RoleId)?;
    Ok(TrialRoleRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_trial_role_skin_reward_request(
    state: &mut State,
    req: TrialRoleSkinRewardRequest,
) -> Result<TrialRoleSkinRewardResponse, String> {
    let _ = claim_one(
        state,
        "trial_skin_rw",
        "RoleSkinTrial",
        req.RoleSkinTrialInfoId,
    )?;
    Ok(TrialRoleSkinRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

// =========================================================================
// Mini-Games: Bath, Board, DropCatch, Encircle, FarmGold, Harvest, Match3,
// MixDrink, MowTower, Parkour, PhotoFight, Renju, RiskHarvest, Tetris, Zither
// =========================================================================

pub fn handle_area_terminal_pin_operation_request(
    state: &mut State,
    req: AreaTerminalPinOperationRequest,
) -> Result<AreaTerminalPinOperationResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:area_pin:{}", req.Id),
        i64::from(req.OpType),
    )
    .map_err(|e| e.to_string())?;
    Ok(AreaTerminalPinOperationResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_bath_change_camera_request(
    state: &mut State,
    req: BathChangeCameraRequest,
) -> Result<BathChangeCameraResponse, String> {
    let cam = req.Camera.unwrap_or(BathCameraType::Third) as i64;
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "misc:bath_cam",
        cam,
    )
    .map_err(|e| e.to_string())?;
    Ok(BathChangeCameraResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_bath_photo_request(
    state: &mut State,
    _req: BathPhotoRequest,
) -> Result<BathPhotoResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("misc:bath_photos", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(BathPhotoResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_place_item_on_board_request(
    state: &mut State,
    req: PlaceItemOnBoardRequest,
) -> Result<PlaceItemOnBoardResponse, String> {
    let key = format!(
        "misc:board_item:{}:{}",
        req.TargetBoardEntityId, req.PlacementEntityId
    );
    pb_kv::save_msg(&state.fs, &mut state.player_components.progress, &key, &req)
        .map_err(|e| e.to_string())?;
    let _ = state.conn.push_msg(
        MovePlacementNotify::MSG_ID,
        &MovePlacementNotify {
            TargetBoardEntityId: req.TargetBoardEntityId,
            PlacementEntityId: req.PlacementEntityId,
            PosRot: req.PosRot,
            PosOnBoard: req.PosOnBoard,
        },
    );
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(PlaceItemOnBoardResponse {
        ToPlace: req.ToPlace,
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_commit_board_request(
    state: &mut State,
    req: CommitBoardRequest,
) -> Result<CommitBoardResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:board_commit:{}", req.BoardEntityId),
    )
    .map_err(|e| e.to_string())?;
    let _ = state.conn.push_msg(
        CommitBoardSuccessNotify::MSG_ID,
        &CommitBoardSuccessNotify {
            BoardEntityId: req.BoardEntityId,
        },
    );
    if let Some(scene) = state.scene.as_mut() {
        let nid_opt = if scene.net_id_map.contains_key(&req.BoardEntityId) {
            Some(req.BoardEntityId)
        } else {
            i32::try_from(req.BoardEntityId)
                .ok()
                .and_then(|cid32| scene.find_entity_net_id_by_config_id(cid32))
        };
        if let Some(nid) = nid_opt {
            let cfg_id = scene
                .find_config_id_by_net_id(nid)
                .map(i64::from)
                .unwrap_or(req.BoardEntityId);
            if cfg_id > 0 {
                state
                    .player_components
                    .quest
                    .info
                    .set_entity_state(cfg_id, "关卡.Common.状态.激活");
                let _ = state.player_components.quest.save(&state.fs);
            }
            if let Some(upd) = scene.update_entity_state_tag(
                &state.assets.tables.gameplay_tag_remap,
                nid,
                "关卡.Common.状态.激活",
            ) {
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
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(CommitBoardResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_construct_trap_request(
    state: &mut State,
    _req: ConstructTrapRequest,
) -> Result<ConstructTrapResponse, String> {
    let nid = state
        .player_components
        .progress
        .info
        .add_key_int("trap_ent:next", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(ConstructTrapResponse {
        ErrorCode: Some(ErrorCode::Success),
        Id: nid,
    })
}

pub fn handle_deconstruct_trap_request(
    state: &mut State,
    req: DeconstructTrapRequest,
) -> Result<DeconstructTrapResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:trap_decon:{}", req.Id),
    )
    .map_err(|e| e.to_string())?;
    Ok(DeconstructTrapResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_drop_catch_info_request(
    state: &mut State,
    _req: DropCatchInfoRequest,
) -> Result<DropCatchInfoResponse, String> {
    let pc = &state.player_components.progress.info;
    let mut infos = Vec::new();
    for entry in &pc.dynamic.ints {
        let Some(rest) = entry.key.strip_prefix("dc:score:") else {
            continue;
        };
        let Ok(dc_id) = rest.parse::<i32>() else {
            continue;
        };
        let pfx = format!("dc_rw:{dc_id}:");
        let states = pc
            .dynamic
            .claims
            .iter()
            .filter_map(|c| c.strip_prefix(&pfx)?.parse::<i32>().ok())
            .collect();
        infos.push(DropCatchLevelInfo {
            DropCatchId: dc_id,
            Score: entry.val as i32,
            RewardStates: states,
            ..Default::default()
        });
    }
    Ok(DropCatchInfoResponse {
        ErrorCode: Some(ErrorCode::Success),
        Infos: infos,
    })
}

pub fn handle_drop_catch_level_finish_request(
    state: &mut State,
    req: DropCatchLevelFinishRequest,
) -> Result<DropCatchLevelFinishResponse, String> {
    let key = format!("dc:score:{}", req.DropCatchId);
    if i64::from(req.Score) > state.player_components.progress.info.get_key_int(&key) {
        pb_kv::persist_int(
            &state.fs,
            &mut state.player_components.progress,
            &key,
            i64::from(req.Score),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(DropCatchLevelFinishResponse {
        Error: Some(ErrorCode::Success),
    })
}

pub fn handle_drop_catch_level_reward_request(
    state: &mut State,
    req: DropCatchLevelRewardRequest,
) -> Result<DropCatchLevelRewardResponse, String> {
    let pfx = format!("dc_rw:{}", req.DropCatchId);
    let _ = claim_id_list(state, &pfx, "DropCatch", &req.RewardIdxs)?;
    Ok(DropCatchLevelRewardResponse {
        Error: Some(ErrorCode::Success),
    })
}

pub fn handle_encircle_challenge_start_request(
    state: &mut State,
    req: EncircleChallengeStartRequest,
) -> Result<EncircleChallengeStartResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "encircle:cur",
        i64::from(req.ChallengeId),
    )
    .map_err(|e| e.to_string())?;
    Ok(EncircleChallengeStartResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_encircle_challenge_complete_request(
    state: &mut State,
    req: EncircleChallengeCompleteRequest,
) -> Result<EncircleChallengeCompleteResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("encircle:done:{}", req.ChallengeId),
    )
    .map_err(|e| e.to_string())?;
    Ok(EncircleChallengeCompleteResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_exhibition_item_save_request(
    state: &mut State,
    req: ExhibitionItemSaveRequest,
) -> Result<ExhibitionItemSaveResponse, String> {
    pb_kv::save_msg(
        &state.fs,
        &mut state.player_components.progress,
        "misc:exhibition",
        &req,
    )
    .map_err(|e| e.to_string())?;
    Ok(ExhibitionItemSaveResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_farm_gold_level_diff_request(
    state: &mut State,
    req: FarmGoldLevelDiffRequest,
) -> Result<FarmGoldLevelDiffResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("fg:diff:{}", req.InstId),
        i64::from(req.Diff),
    )
    .map_err(|e| e.to_string())?;
    Ok(FarmGoldLevelDiffResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_farm_gold_level_play_repeated_request(
    state: &mut State,
    req: FarmGoldLevelPlayRepeatedRequest,
) -> Result<FarmGoldLevelPlayRepeatedResponse, String> {
    let _ = claim_id_list(state, "fg_lp", "FarmGold", &req.InstIdList)?;
    Ok(FarmGoldLevelPlayRepeatedResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_farm_gold_point_repeated_request(
    state: &mut State,
    req: FarmGoldPointRepeatedRequest,
) -> Result<FarmGoldPointRepeatedResponse, String> {
    let _ = claim_id_list(state, "fg_pt", "FarmGold", &req.Ids)?;
    Ok(FarmGoldPointRepeatedResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_farm_gold_result_request(
    state: &mut State,
    _req: FarmGoldResultRequest,
) -> Result<FarmGoldResultResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("fg:runs", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(FarmGoldResultResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_fix_tool_request(
    state: &mut State,
    req: FixToolRequest,
) -> Result<FixToolResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:fix_tool:{}", req.EntityId),
        i64::from(req.FixTool),
    )
    .map_err(|e| e.to_string())?;
    Ok(FixToolResponse {
        Code: Some(ErrorCode::Success),
        EntityId: req.EntityId,
    })
}

pub fn handle_flower_pollution_request(
    state: &mut State,
    req: FlowerPollutionRequest,
) -> Result<FlowerPollutionResponse, String> {
    let key = format!("misc:flower:{}", req.TargetEntityId);
    pb_kv::save_msg(&state.fs, &mut state.player_components.progress, &key, &req)
        .map_err(|e| e.to_string())?;
    Ok(FlowerPollutionResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_flower_pollution_spline_push(
    state: &mut State,
    push: FlowerPollutionSplinePush,
) -> Result<(), String> {
    let key = format!("flower_poll:{}", push.ManagerEntityId);
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &key,
        if push.IsRemovePollution {
            0
        } else {
            i64::from(push.SplineConfigId)
        },
    )
    .map_err(|e| e.to_string())?;
    if let Some(scene) = state.scene.as_mut()
        && let Some(&idx) = scene.net_id_map.get(&push.ManagerEntityId)
    {
        scene.entities[idx].world_item.flower_pollution = !push.IsRemovePollution;
    }
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(())
}

pub fn handle_foundation_request(
    state: &mut State,
    req: FoundationRequest,
) -> Result<FoundationResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:foundation:{}", req.EntityId),
        i64::from(req.ActiveOperate),
    )
    .map_err(|e| e.to_string())?;
    Ok(FoundationResponse {
        ErrorCode: Some(ErrorCode::Success),
        ActiveOperate: req.ActiveOperate,
    })
}

pub fn handle_foundation_core_operate_request(
    state: &mut State,
    req: FoundationCoreOperateRequest,
) -> Result<FoundationCoreOperateResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:foundation_core:{}", req.FoundationEntityIncId),
        if req.IsTakeOut { 0 } else { 1 },
    )
    .map_err(|e| e.to_string())?;

    let instance_id = state.scene.as_ref().map(|s| s.instance_id).unwrap_or(8);
    let cfg_id: i32 = if let Some(scene) = state.scene.as_ref() {
        if let Some(&idx) = scene.net_id_map.get(&req.FoundationEntityIncId) {
            scene.entities[idx].config.config_id
        } else if let Some(cid) = scene.find_config_id_by_net_id(req.FoundationEntityIncId) {
            cid.max(1)
        } else {
            req.FoundationEntityIncId.clamp(1, i64::from(i32::MAX)) as i32
        }
    } else {
        req.FoundationEntityIncId.clamp(1, i64::from(i32::MAX)) as i32
    };

    if !req.IsTakeOut {
        let info = &state.player_components.progress.info;
        let hand_box = info
            .get_key_int("misc:hand_core_box")
            .clamp(0, i64::from(i32::MAX)) as i32;
        let hand_src = info
            .get_key_int("misc:hand_core_src")
            .clamp(0, i64::from(i32::MAX)) as i32;
        let resolved_box = if hand_box > 0 { hand_box } else { 850001 };
        let resolved_src = if hand_src > 0 { hand_src } else { cfg_id };
        if let Some(scene) = state.scene.as_mut()
            && let Some(&idx) = scene.net_id_map.get(&req.FoundationEntityIncId)
        {
            scene.entities[idx].world_item.foundation_source_entity_id = resolved_src;
        }
        let info = &mut state.player_components.progress.info;
        info.set_key_int("misc:hand_core_box", 0);
        info.set_key_int("misc:hand_core_src", 0);
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
        let _ = state.conn.push_msg(
            EntityFoundationCoreChangeNotify::MSG_ID,
            &EntityFoundationCoreChangeNotify {
                FoundationEntityId: cfg_id,
                HasFoundationCore: true,
                FoundationCoreInfo: Some(FoundationCoreInfo {
                    SourceEntityId: resolved_src,
                    ElementalBoxId: resolved_box,
                    State: "关卡.Common.状态.激活".as_bytes().to_vec(),
                    Priority: 1,
                    EntityId: cfg_id,
                    ..Default::default()
                }),
            },
        );
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            let _ = quest_engine::on_condition_state_changed(&mut qctx);
        }
        drain_event_queue_best_effort(&mut events, state);
        return Ok(FoundationCoreOperateResponse {
            ErrorCode: Some(ErrorCode::Success),
            FromFoundationEntityId: resolved_src,
            ..Default::default()
        });
    }

    let mut box_id = 850001i32;
    let mut elem_type_opt: Option<String> = None;
    if let Some(le) = state
        .assets
        .find_level_entity(instance_id, i64::from(cfg_id))
    {
        let mut merged = le.ComponentsData.clone();
        if let Some(tpl) = state.assets.find_template_config(&le.BlueprintType) {
            tpl.ComponentsData.merge_into(&mut merged);
        }
        for entry in merged.extra.iter().chain(merged.template_extra.iter()) {
            if let ExtraEntry::ElementalBoxFoundationComponent(ef) = entry {
                if let Some(eid) = ef.InitialActiveElementalBoxId
                    && eid > 0
                {
                    box_id = eid as i32;
                }
                if let Some(et) = &ef.ElementalType
                    && !et.is_empty()
                {
                    elem_type_opt = Some(et.clone());
                }
            }
        }
    }

    let box_cfg = state
        .assets
        .tables
        .elemental_box_define
        .get_data_by_id(&box_id)
        .or_else(|| {
            state
                .assets
                .tables
                .elemental_box_define
                .get_data_by_id(&850001)
        })
        .or_else(|| {
            state
                .assets
                .tables
                .elemental_box_define
                .items
                .first()
                .map(TableRef::Borrowed)
        });
    let resolved_box_id = box_cfg.as_ref().map(|bc| bc.Id).unwrap_or(box_id);
    let elem_type = elem_type_opt.unwrap_or_else(|| {
        box_cfg
            .as_ref()
            .filter(|bc| !bc.ElementalType.is_empty())
            .map(|bc| bc.ElementalType.clone())
            .unwrap_or_else(|| "关卡.Common.匣子元素.火".to_string())
    });
    let state_str = box_cfg
        .as_ref()
        .filter(|bc| !bc.InitState.is_empty())
        .map(|bc| bc.InitState.clone())
        .unwrap_or_else(|| "关卡.Common.状态.激活".to_string());
    let priority = box_cfg.as_ref().map(|bc| bc.Priority).unwrap_or(1);

    if let Some(scene) = state.scene.as_mut()
        && let Some(&idx) = scene.net_id_map.get(&req.FoundationEntityIncId)
    {
        scene.entities[idx].world_item.foundation_source_entity_id = 0;
    }
    let info = &mut state.player_components.progress.info;
    info.set_key_int("misc:hand_core_box", i64::from(resolved_box_id));
    info.set_key_int("misc:hand_core_src", i64::from(cfg_id));
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let _ = state.conn.push_msg(
        EntityFoundationCoreChangeNotify::MSG_ID,
        &EntityFoundationCoreChangeNotify {
            FoundationEntityId: cfg_id,
            HasFoundationCore: false,
            FoundationCoreInfo: None,
        },
    );
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(FoundationCoreOperateResponse {
        ErrorCode: Some(ErrorCode::Success),
        FromFoundationEntityId: cfg_id,
        FoundationCoreInfo: Some(FoundationCoreInfo {
            SourceEntityId: cfg_id,
            ElementalBoxId: resolved_box_id,
            State: state_str.into_bytes(),
            ElementalType: elem_type.into_bytes(),
            Priority: priority,
            EntityId: cfg_id,
        }),
    })
}

pub fn handle_pulling_foundation_request(
    state: &mut State,
    req: PullingFoundationRequest,
) -> Result<PullingFoundationResponse, String> {
    let key = format!("misc:pull_foundation:{}", req.EntityId);
    pb_kv::save_msg(&state.fs, &mut state.player_components.progress, &key, &req)
        .map_err(|e| e.to_string())?;
    Ok(PullingFoundationResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_golem_crack_update_request(
    state: &mut State,
    req: GolemCrackUpdateRequest,
) -> Result<GolemCrackUpdateResponse, String> {
    if let Some(info) = &req.GolemCrackLevelInfo {
        let key = format!("misc:golem:{}:{}", req.ActivityId, info.id);
        pb_kv::save_msg(&state.fs, &mut state.player_components.progress, &key, info)
            .map_err(|e| e.to_string())?;
    }
    Ok(GolemCrackUpdateResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_harvest_activity_level_diff_request(
    state: &mut State,
    req: HarvestActivityLevelDiffRequest,
) -> Result<HarvestActivityLevelDiffResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("harv:diff:{}", req.InstId),
        i64::from(req.Diff),
    )
    .map_err(|e| e.to_string())?;
    Ok(HarvestActivityLevelDiffResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_harvest_activity_level_request(
    state: &mut State,
    req: HarvestActivityLevelRequest,
) -> Result<HarvestActivityLevelResponse, String> {
    let item_map = claim_one(state, "harv_lv", "RiskHarvestInst", req.InstId)?;
    Ok(HarvestActivityLevelResponse {
        ErrorCode: Some(ErrorCode::Success),
        ItemMap: item_map,
    })
}

pub fn handle_harvest_activity_point_request(
    state: &mut State,
    req: HarvestActivityPointRequest,
) -> Result<HarvestActivityPointResponse, String> {
    let item_map = claim_one(state, "harv_pt", "RiskHarvestScoreReward", req.Id)?;
    Ok(HarvestActivityPointResponse {
        ErrorCode: Some(ErrorCode::Success),
        ItemMap: item_map,
    })
}

pub fn handle_harvest_activity_result_request(
    state: &mut State,
    _req: HarvestActivityResultRequest,
) -> Result<HarvestActivityResultResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("harv:runs", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(HarvestActivityResultResponse::default())
}

pub fn handle_inspiration_unlock_choice_request(
    state: &mut State,
    req: InspirationUnlockChoiceRequest,
) -> Result<InspirationUnlockChoiceResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("insp:{}:{}", req.ChapterId, req.ChoiceId),
    )
    .map_err(|e| e.to_string())?;
    Ok(InspirationUnlockChoiceResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_invest_request(
    state: &mut State,
    req: InvestRequest,
) -> Result<InvestResponse, String> {
    if req.Amount > 0 {
        let costs = [CostItem {
            id: 15,
            count: req.Amount,
        }];
        let ok = consume::consume_costs(
            &mut state.conn,
            &state.fs,
            &mut state.player_components.basic,
            &mut state.player_components.inventory,
            &costs,
        )
        .map_err(|e| e.to_string())?;
        if !ok {
            return Ok(InvestResponse {
                ErrorCode: Some(ErrorCode::ErrItemNotEnough),
                Success: false,
                ..Default::default()
            });
        }
    }
    let gain = req.Amount * 2;
    let info = &mut state.player_components.progress.info;
    info.add_key_int(&format!("invest:{}", req.EntrustId), i64::from(gain));
    info.add_key_int("trackmoon:gold", i64::from(gain));
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;

    let delta = (req.Amount / 10).max(10);
    let role_settle = (1..=3)
        .map(|rid| RoleProperSettle {
            RoleId: rid,
            PropertyA: delta,
            PropertyB: delta,
            PropertyC: delta,
            Level: 1,
        })
        .collect();
    Ok(InvestResponse {
        Heat: 10,
        RoleProperSettle: role_settle,
        Gold: gain,
        BaseGold: req.Amount,
        EvaluationLevel: 3,
        ErrorCode: Some(ErrorCode::Success),
        Success: true,
    })
}

pub fn handle_life_point_challenge_start_request(
    state: &mut State,
    req: LifePointChallengeStartRequest,
) -> Result<LifePointChallengeStartResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "misc:lifepoint_cur",
        i64::from(req.Id),
    )
    .map_err(|e| e.to_string())?;
    Ok(LifePointChallengeStartResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_line_cross_challenge_start_request(
    state: &mut State,
    req: LineCrossChallengeStartRequest,
) -> Result<LineCrossChallengeStartResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "misc:linecross_cur",
        i64::from(req.Id),
    )
    .map_err(|e| e.to_string())?;
    Ok(LineCrossChallengeStartResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_match_three_star_request(
    state: &mut State,
    req: MatchThreeStarRequest,
) -> Result<MatchThreeStarResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:m3_role:{}", req.LevelId),
        i64::from(req.JoinRole),
    )
    .map_err(|e| e.to_string())?;
    Ok(MatchThreeStarResponse {
        TraceId: b"m3-1".to_vec(),
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_match_three_end_request(
    state: &mut State,
    req: MatchThreeEndRequest,
) -> Result<MatchThreeEndResponse, String> {
    let key = format!("misc:m3_end:{}", req.LevelId);
    pb_kv::save_msg(&state.fs, &mut state.player_components.progress, &key, &req)
        .map_err(|e| e.to_string())?;
    Ok(MatchThreeEndResponse {
        ErrorCode: Some(ErrorCode::Success),
        Success: true,
    })
}

pub fn handle_mini_write_letter_save_content_request(
    state: &mut State,
    req: MiniWriteLetterSaveContentRequest,
) -> Result<MiniWriteLetterSaveContentResponse, String> {
    pb_kv::save_msg(
        &state.fs,
        &mut state.player_components.progress,
        "misc:letter",
        &req,
    )
    .map_err(|e| e.to_string())?;
    Ok(MiniWriteLetterSaveContentResponse {
        ErrorId: Some(ErrorCode::Success),
    })
}

pub fn handle_mix_drink_role_invite_request(
    state: &mut State,
    req: MixDrinkRoleInviteRequest,
) -> Result<MixDrinkRoleInviteResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("mix_inv:{}", req.RoleId),
    )
    .map_err(|e| e.to_string())?;
    Ok(MixDrinkRoleInviteResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_mix_drink_role_reward_request(
    state: &mut State,
    req: MixDrinkRoleRewardRequest,
) -> Result<MixDrinkRoleRewardResponse, String> {
    let _ = claim_one(state, "mix_role_rw", "MixDrink", req.RoleInviteId)?;
    Ok(MixDrinkRoleRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_mix_drink_settle_request(
    state: &mut State,
    req: MixDrinkSettleRequest,
) -> Result<MixDrinkSettleResponse, String> {
    let key = format!("misc:mix_settle:{}", req.RequireId);
    pb_kv::save_msg(&state.fs, &mut state.player_components.progress, &key, &req)
        .map_err(|e| e.to_string())?;
    Ok(MixDrinkSettleResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_mow_tower_levels_reward_request(
    state: &mut State,
    req: MowTowerLevelsRewardRequest,
) -> Result<MowTowerLevelsRewardResponse, String> {
    let _ = claim_one(state, "mow_rw", "MowTower", req.RewardId)?;
    Ok(MowTowerLevelsRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_mow_tower_settlement_request(
    state: &mut State,
    _req: MowTowerSettlementRequest,
) -> Result<MowTowerSettlementResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("mow:settle_cnt", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let _ = state.conn.push_msg(
        MowTowerResultNotify::MSG_ID,
        &MowTowerResultNotify {
            LevelsId: 1,
            FirstKilMonsterScore: 1000,
            FirstTimerScore: 500,
            SecondKilMonsterScore: 1000,
            SecondTimerScore: 500,
            HighScore: 3000,
        },
    );
    Ok(MowTowerSettlementResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_open_system_board_result_request(
    state: &mut State,
    req: OpenSystemBoardResultRequest,
) -> Result<OpenSystemBoardResultResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:sys_board:{}", req.IncId),
        i64::from(req.Result),
    )
    .map_err(|e| e.to_string())?;
    if let Some(pending) = state
        .scene
        .as_mut()
        .and_then(|s| s.take_pending_system_board(req.IncId))
    {
        let mut events = EventQueue::new();
        {
            let mut qctx = state.quest_context(&mut events);
            let mut cbs = quest_engine::action_callbacks();
            quest_actions::apply_system_board_return_var(&mut qctx, &mut cbs, &pending, req.Result)
                .map_err(|e| e.to_string())?;
        }
        drain_event_queue_best_effort(&mut events, state);
    }
    Ok(OpenSystemBoardResultResponse {
        Code: Some(ErrorCode::Success),
    })
}

pub fn handle_parkour_challenge_request(
    state: &mut State,
    _req: ParkourChallengeRequest,
) -> Result<ParkourChallengeResponse, String> {
    let pc = &state.player_components.progress.info;
    let open_ids: Vec<i32> = (1..=6).collect();
    let challenges = (1..=6)
        .map(|cid| ParkourChallenge {
            ChallengeId: cid,
            MaxScore: pc.get_key_int(&format!("parkour:best:{cid}")) as i32,
            ..Default::default()
        })
        .collect();
    Ok(ParkourChallengeResponse {
        OpenIds: open_ids,
        Challenges: challenges,
    })
}

pub fn handle_parkour_challenge_take_request(
    state: &mut State,
    req: ParkourChallengeTakeRequest,
) -> Result<ParkourChallengeTakeResponse, String> {
    let pfx = format!("parkour:{}", req.ChallengeId);
    let rws = claim_one(state, &pfx, "Parkour", req.ScoreIndex)?;
    Ok(ParkourChallengeTakeResponse {
        Rewards: rws,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_parkour_challenge_trans_request(
    state: &mut State,
    req: ParkourChallengeTransRequest,
) -> Result<ParkourChallengeTransResponse, String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "parkour:cur",
        i64::from(req.ChallengeId),
    )
    .map_err(|e| e.to_string())?;
    Ok(ParkourChallengeTransResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_photo_fight_settle_request(
    state: &mut State,
    _req: PhotoFightSettleRequest,
) -> Result<PhotoFightSettleResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("pf:settle_cnt", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(PhotoFightSettleResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_photo_fight_task_reward_request(
    state: &mut State,
    req: PhotoFightTaskRewardRequest,
) -> Result<PhotoFightTaskRewardResponse, String> {
    let _ = claim_id_list(state, "pf_task", "PhotoFight", &req.TargetIds)?;
    let targets = req
        .TargetIds
        .into_iter()
        .map(|tid| PhotoFightTargetPb {
            Id: tid,
            Status: Some(ActivityTaskState::ActivityTaskTaken),
            ..Default::default()
        })
        .collect();
    Ok(PhotoFightTaskRewardResponse {
        Targets: targets,
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_photo_memory_reward_request(
    state: &mut State,
    req: PhotoMemoryRewardRequest,
) -> Result<PhotoMemoryRewardResponse, String> {
    let _ = claim_id_list(state, "pmem_rw", "PhotoMemory", &req.CollectIds)?;
    Ok(PhotoMemoryRewardResponse {
        Error: Some(ErrorCode::Success),
    })
}

pub fn handle_renju_exit_matched_action_request(
    state: &mut State,
    req: RenjuExitMatchedActionRequest,
) -> Result<RenjuExitMatchedActionResponse, String> {
    pb_kv::persist_claim(
        &state.fs,
        &mut state.player_components.progress,
        &format!("misc:renju_exit:{}", req.EntityId),
    )
    .map_err(|e| e.to_string())?;
    Ok(RenjuExitMatchedActionResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_renju_reset_request(
    state: &mut State,
    req: RenjuResetRequest,
) -> Result<RenjuResetResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int(&format!("misc:renju_reset:{}", req.EntityId), 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(RenjuResetResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}

pub fn handle_risk_harvest_inst_reward_request(
    state: &mut State,
    req: RiskHarvestInstRewardRequest,
) -> Result<RiskHarvestInstRewardResponse, String> {
    let _ = claim_one(state, "rh_inst", "RiskHarvestInst", req.Id)?;
    Ok(RiskHarvestInstRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_risk_harvest_score_reward_request(
    state: &mut State,
    req: RiskHarvestScoreRewardRequest,
) -> Result<RiskHarvestScoreRewardResponse, String> {
    let _ = claim_one(state, "rh_score", "RiskHarvestScoreReward", req.Id)?;
    Ok(RiskHarvestScoreRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_risk_harvest_star_reward_request(
    state: &mut State,
    req: RiskHarvestStarRewardRequest,
) -> Result<RiskHarvestStarRewardResponse, String> {
    let pfx = format!("rh_star:{}", req.Id);
    let _ = claim_one(state, &pfx, "RiskHarvestInst", req.Index)?;
    Ok(RiskHarvestStarRewardResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_risk_harvest_settle_request(
    state: &mut State,
    _req: RiskHarvestSettleRequest,
) -> Result<RiskHarvestSettleResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("harv:settle_cnt", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(RiskHarvestSettleResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_sun_spirit_action_operation_end_push(
    state: &mut State,
    push: SunSpiritActionOperationEndPush,
) -> Result<(), String> {
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        &format!("sun_spirit:op:{}", push.IncId),
        i64::from(push.Index),
    )
    .map_err(|e| e.to_string())?;
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(())
}

pub fn handle_tetris_finish_request(
    state: &mut State,
    req: TetrisFinishRequest,
) -> Result<TetrisFinishResponse, String> {
    let info = &mut state.player_components.progress.info;
    if i64::from(req.Score) > info.get_key_int("tetris:best") {
        info.set_key_int("tetris:best", i64::from(req.Score));
        state
            .player_components
            .progress
            .save(&state.fs)
            .map_err(|e| e.to_string())?;
    }
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        if let Some(mut lp_ctx) = level_play::LevelPlayContext::from_quest_ctx(&mut qctx) {
            let cfg_id = lp_ctx
                .scene
                .find_config_id_by_net_id(req.EntityId)
                .unwrap_or(req.EntityId as i32);
            level_play::on_condition_event(
                &mut lp_ctx,
                LevelPlayEventKind::FinishTetris,
                cfg_id,
                req.Score,
            )
            .map_err(|e| e.to_string())?;
        }
        quest_engine::on_condition_state_changed(&mut qctx).map_err(|e| e.to_string())?;
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(TetrisFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_tetris_update_request(
    state: &mut State,
    req: TetrisUpdateRequest,
) -> Result<TetrisUpdateResponse, String> {
    if let Some(info) = &req.TetrisLevelInfo {
        let key = format!("misc:tetris:{}", req.ActivityId);
        pb_kv::save_msg(&state.fs, &mut state.player_components.progress, &key, info)
            .map_err(|e| e.to_string())?;
    }
    Ok(TetrisUpdateResponse {
        ErrCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_vacuum_cleaner_request(
    state: &mut State,
    req: VacuumCleanerRequest,
) -> Result<VacuumCleanerResponse, String> {
    state
        .player_components
        .progress
        .info
        .add_key_int(&format!("misc:vacuum:{}", req.AdsorbEntityIncId), 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    Ok(VacuumCleanerResponse {
        ErrorCode: Some(ErrorCode::Success),
    })
}

pub fn handle_zither_play_note_push(
    state: &mut State,
    _push: ZitherPlayNotePush,
) -> Result<(), String> {
    state
        .player_components
        .progress
        .info
        .add_key_int("misc:zither_notes", 1);
    state
        .player_components
        .progress
        .save(&state.fs)
        .map_err(|e| e.to_string())?;
    let mut events = EventQueue::new();
    {
        let mut qctx = state.quest_context(&mut events);
        let _ = quest_engine::on_condition_state_changed(&mut qctx);
    }
    drain_event_queue_best_effort(&mut events, state);
    Ok(())
}

pub fn handle_zither_play_state_request(
    state: &mut State,
    req: ZitherPlayStateRequest,
) -> Result<ZitherPlayStateResponse, String> {
    let st = req.State.map(|s| s as i64).unwrap_or(0);
    pb_kv::persist_int(
        &state.fs,
        &mut state.player_components.progress,
        "misc:zither_state",
        st,
    )
    .map_err(|e| e.to_string())?;
    Ok(ZitherPlayStateResponse {
        ErrCode: Some(ErrorCode::Success),
    })
}
