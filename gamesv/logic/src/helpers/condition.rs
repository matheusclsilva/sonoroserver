use std::cell::Cell;

use serde_json::{Map, Value};
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::asset_indexes;
use sonoro_gamesv_data::tables::condition::Condition;
use sonoro_gamesv_data::tables::entity_components::base_info::GravityDirection;
use sonoro_gamesv_data::tables::entity_components::components::Components;
use sonoro_gamesv_data::tables::entity_components::extra::ExtraEntry;
use sonoro_gamesv_data::tables::entity_components::range::ShapeConfig;
use sonoro_gamesv_data::tables::function_condition::FunctionCondition;
use sonoro_gamesv_fs::progress_info::ProgressInfo;
use sonoro_gamesv_fs::role_info::RoleInfo;
use sonoro_proto::enums::EAttributeType;
use sonoro_proto::pb::{FuncOpenNotify, Function};
use sonoro_proto::wire::PbEnum;

use crate::component::player::{
    PlayerBasicComponent, PlayerEchoComponent, PlayerGuideComponent, PlayerInventoryComponent,
    PlayerMapComponent, PlayerProgressComponent, PlayerQuestComponent, PlayerRoleComponent,
    PlayerWeaponComponent,
};
use crate::helpers::{calabash, gameplay_tags, gravity, morale};
use crate::scene::Scene;
use crate::sink::PushSink;

thread_local! {
    static QUEST_STATE_EVAL_DEPTH: Cell<u8> = const { Cell::new(0) };
}

struct QuestStateDepthGuard;

impl Drop for QuestStateDepthGuard {
    fn drop(&mut self) {
        QUEST_STATE_EVAL_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

#[derive(Clone)]
pub struct EvalContext<'a> {
    pub assets: Option<&'a Assets>,
    pub basic_comp: &'a PlayerBasicComponent,
    pub echo_comp: &'a PlayerEchoComponent,
    pub inventory_comp: &'a PlayerInventoryComponent,
    pub map_comp: &'a PlayerMapComponent,
    pub quest_comp: &'a PlayerQuestComponent,
    pub role_comp: Option<&'a PlayerRoleComponent>,
    pub weapon_comp: Option<&'a PlayerWeaponComponent>,
    pub guide_comp: Option<&'a PlayerGuideComponent>,
    pub progress_comp: Option<&'a PlayerProgressComponent>,
    pub scene: Option<&'a Scene>,
    pub owner_quest_id: i32,
    pub target_role_id: i32,
    pub self_entity_config_id: i32,
}

impl<'a> EvalContext<'a> {
    pub fn new(
        basic_comp: &'a PlayerBasicComponent,
        echo_comp: &'a PlayerEchoComponent,
        inventory_comp: &'a PlayerInventoryComponent,
        map_comp: &'a PlayerMapComponent,
        quest_comp: &'a PlayerQuestComponent,
    ) -> Self {
        Self {
            assets: None,
            basic_comp,
            echo_comp,
            inventory_comp,
            map_comp,
            quest_comp,
            role_comp: None,
            weapon_comp: None,
            guide_comp: None,
            progress_comp: None,
            scene: None,
            owner_quest_id: 0,
            target_role_id: 0,
            self_entity_config_id: 0,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn push_func_open_notify<S: PushSink + ?Sized>(
    sink: &mut S,
    assets: &Assets,
    basic_comp: &PlayerBasicComponent,
    echo_comp: &PlayerEchoComponent,
    inventory_comp: &PlayerInventoryComponent,
    map_comp: &PlayerMapComponent,
    quest_comp: &PlayerQuestComponent,
    role_comp: Option<&PlayerRoleComponent>,
) {
    let mut eval_ctx = EvalContext::new(basic_comp, echo_comp, inventory_comp, map_comp, quest_comp);
    eval_ctx.assets = Some(assets);
    eval_ctx.role_comp = role_comp;

    let mut funcs = Vec::with_capacity(assets.tables.function_condition.items.len());
    for config in &assets.tables.function_condition.items {
        let is_open = is_function_config_open(&eval_ctx, config);
        funcs.push(Function {
            Id: config.FunctionId,
            Flag: if is_open { 2 } else { 0 },
        });
    }

    sink.push(FuncOpenNotify::MSG_ID, &FuncOpenNotify { Func: funcs });
}

fn is_function_config_open(ctx: &EvalContext<'_>, config: &FunctionCondition) -> bool {
    let action_gated = ctx
        .assets
        .is_some_and(|a| a.indexes.is_action_opened_function(config.FunctionId));
    (config.OpenConditionId == 0 && !action_gated)
        || config.FunctionId == 10026010
        || ctx.quest_comp.info.has_unlocked_system(config.FunctionId)
        || (config.OpenConditionId != 0 && eval_condition_group(ctx, config.OpenConditionId))
}

pub fn is_function_open(ctx: &EvalContext<'_>, function_id: i32) -> bool {
    if function_id == 10026010 || ctx.quest_comp.info.has_unlocked_system(function_id) {
        return true;
    }
    let Some(assets) = ctx.assets else {
        return false;
    };
    let Some(config) = assets.tables.function_condition.get_data_by_id(&function_id) else {
        return false;
    };
    is_function_config_open(ctx, &config)
}

pub fn eval_condition_group(ctx: &EvalContext<'_>, group_id: i32) -> bool {
    if group_id <= 0 {
        return true;
    }
    let Some(a) = ctx.assets else {
        return false;
    };
    let Some(group) = a.tables.condition_group.get_data_by_id(&group_id) else {
        return false;
    };
    if group.GroupId.is_empty() {
        return true;
    }

    // Relation 0 = AND (all must pass); Relation 1 = OR (at least one passes)
    let is_or = group.Relation == 1;
    if is_or {
        group.GroupId.iter().any(|&cond_id| eval_condition_id(ctx, cond_id))
    } else {
        group.GroupId.iter().all(|&cond_id| eval_condition_id(ctx, cond_id))
    }
}

pub fn eval_condition_or_group(ctx: &EvalContext<'_>, id: i32) -> bool {
    if id <= 0 {
        return true;
    }
    let Some(a) = ctx.assets else {
        return false;
    };
    if a.tables.condition_group.get_data_by_id(&id).is_some() {
        return eval_condition_group(ctx, id);
    }
    if a.tables.condition.get_data_by_id(&id).is_some() {
        return eval_condition_id(ctx, id);
    }
    false
}

pub fn eval_condition_id(ctx: &EvalContext<'_>, cond_id: i32) -> bool {
    let Some(a) = ctx.assets else {
        return false;
    };
    let Some(cond) = a.tables.condition.get_data_by_id(&cond_id) else {
        return false;
    };
    eval_condition_record(ctx, &cond)
}

pub fn eval_condition_record(ctx: &EvalContext<'_>, cond: &Condition) -> bool {
    let empty = Map::new();
    let lp = cond
        .LimitParams
        .as_ref()
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    match cond.Type.as_str() {
        "CheckLevel" => {
            let req = get_param_int(lp, "Level").unwrap_or(1);
            ctx.basic_comp.info.attributes.level >= req
        }
        "CheckOriginWorldLevel" => {
            let req = get_param_int(lp, "Level").unwrap_or(0);
            ctx.basic_comp.info.attributes.origin_world_level >= req
        }
        "CheckCurWorldLevel" => {
            let req = get_param_int(lp, "Level").unwrap_or(0);
            ctx.basic_comp.info.attributes.cur_world_level >= req
        }
        "CalabashLevel" => {
            let req = get_param_int(lp, "Level").unwrap_or(1);
            let Some(a) = ctx.assets else {
                return false;
            };
            current_calabash_level(a, ctx.basic_comp, ctx.echo_comp) >= req
        }
        "NewQuestState" => {
            let Some(qid) = get_param_int(lp, "NewQuestId") else {
                return false;
            };
            let st = get_param_int(lp, "State").unwrap_or(3);
            quest_state_value(ctx, qid) == st
        }
        "NewQuestStepState" | "QuestNodeStatus" => {
            let Some(qid) = get_param_int(lp, "NewQuestId") else {
                return false;
            };
            let Some(nid) = get_param_int(lp, "QuestNodeId") else {
                return false;
            };
            let st = get_param_int(lp, "State").unwrap_or(3);
            eval_quest_node_status(ctx, qid, nid, st)
        }
        "CheckTeleportStatus" => {
            let Some(tid) = get_param_int(lp, "TeleportId") else {
                return false;
            };
            let want = get_param_int(lp, "State").unwrap_or(1) != 0;
            let unlocked = ctx.map_comp.info.unlocked_teleporters.contains(&tid);
            unlocked == want
        }
        "PassInst" => {
            let Some(inst_id) = get_param_int(lp, "InstanceId") else {
                return false;
            };
            ctx.quest_comp.info.completed_dungeons.contains(&inst_id)
        }
        "LevelPlayComplete" => {
            let Some(raw) = get_param_str(lp, "LevelPlayId") else {
                return false;
            };
            let count = raw
                .split('-')
                .filter_map(|tok| tok.parse::<i32>().ok())
                .filter(|lpid| ctx.quest_comp.info.completed_level_plays.contains(lpid))
                .count() as i32;
            count >= cond.NeedNum.max(1)
        }
        "LevelPlayCompleteNew" => {
            let Some(raw) = get_param_str(lp, "LevelPlayId") else {
                return false;
            };
            raw.split('-')
                .filter(|tok| !tok.is_empty())
                .filter_map(|tok| tok.parse::<i32>().ok())
                .any(|lpid| ctx.quest_comp.info.completed_level_plays.contains(&lpid))
        }
        "ItemCheck" | "GetItem" | "GetNewItem" | "CostItem" => {
            let Some(item_id) = get_param_int(lp, "ItemId").or_else(|| get_param_int(lp, "ItemID")) else {
                return false;
            };
            item_count(ctx, item_id) >= cond.NeedNum.max(1)
        }
        "CheckItemWithOperator" => {
            let Some(item_id) = get_param_int(lp, "ItemId").or_else(|| get_param_int(lp, "ItemID")) else {
                return false;
            };
            let req_count = get_param_int(lp, "Count").unwrap_or_else(|| cond.NeedNum.max(1));
            let op = get_param_str(lp, "Op").unwrap_or("Ge");
            compare_ints(item_count(ctx, item_id), req_count, op)
        }
        "FunctionUnlock" => {
            let Some(fid) = get_param_int(lp, "FunctionId") else {
                return false;
            };
            ctx.quest_comp.info.has_unlocked_system(fid)
        }
        "CheckInstanceState" => {
            let Some(inst_id) = get_param_int(lp, "InstanceId") else {
                return false;
            };
            let want_done = get_param_int(lp, "State").unwrap_or(1) != 0;
            ctx.quest_comp.info.completed_dungeons.contains(&inst_id) == want_done
        }
        "TriggerTeleport" => {
            let Some(tid) = get_param_int(lp, "TeleportId") else {
                return false;
            };
            ctx.map_comp.info.unlocked_teleporters.contains(&tid)
        }
        "VarDefineInt" => {
            let Some(vd_name) = get_param_str(lp, "VDName") else {
                return false;
            };
            let expected = get_param_int(lp, "VDValueInt").unwrap_or(0);
            ctx.quest_comp.info.get_var_int(vd_name) == expected
        }
        "CheckClientQuestNodeStatus" | "CheckClientQuest" => {
            let Some(qid) = get_param_int(lp, "任务Id") else {
                return false;
            };
            let nid_opt = get_param_int(lp, "步骤Id");
            let st = get_param_int(lp, "状态").unwrap_or(3);
            if let Some(nid) = nid_opt {
                eval_quest_node_status(ctx, qid, nid, st)
            } else {
                eval_quest_state(ctx, qid, st)
            }
        }
        "EntityStateById" | "EntityStateByBpTypeOnline" => {
            let Some(cid) = get_param_int(lp, "EntityConfigId") else {
                return false;
            };
            let Some(expected_tag) = get_param_int(lp, "Tag") else {
                return false;
            };
            let Some(state_str) = ctx.quest_comp.info.get_entity_state(i64::from(cid)) else {
                return false;
            };
            let Some(a) = ctx.assets else {
                return false;
            };
            let Some(cur_tag) = gameplay_tags::id_from_name(&a.tables.gameplay_tag_remap, state_str) else {
                return false;
            };
            cur_tag == expected_tag
        }
        "CheckActivityState" => {
            let Some(aid) = get_param_int(lp, "ActivityId") else {
                return false;
            };
            let want = get_param_int(lp, "ActivityState").unwrap_or(2);
            let op = get_param_str(lp, "Compare").unwrap_or("Eq");
            let cur = if is_activity_open(ctx, aid) { 2 } else { 0 };
            compare_ints(cur, want, op)
        }
        "ReadMail" => {
            let Some(mid) = get_param_int(lp, "MailId") else {
                return false;
            };
            ctx.quest_comp.info.progress.read_mails.contains(&mid)
        }
        "ReadPhoneMessage" => {
            let Some(pmid) = get_param_int(lp, "PhoneMessageId") else {
                return false;
            };
            ctx.quest_comp.info.progress.read_phone_messages.contains(&pmid)
        }
        "ReceiveTelecom" => {
            let Some(tid) = get_param_int(lp, "TelecomId") else {
                return false;
            };
            ctx.quest_comp.info.progress.received_telecoms.contains(&tid)
        }
        "FinishBvbChallenge" => {
            let Some(bid) = get_param_int(lp, "BvbChallengeId") else {
                return false;
            };
            ctx.quest_comp.info.progress.completed_bvb.contains(&bid)
        }
        "CheckPlayerGender" => match get_param_str(lp, "Gender") {
            Some("男") => ctx.basic_comp.info.attributes.sex == 1,
            Some("女") => ctx.basic_comp.info.attributes.sex == 0,
            _ => true,
        },
        "RoleBreach" => {
            let req = get_param_int(lp, "Breach").unwrap_or(0);
            eval_role_threshold(ctx, lp, RoleThresholdField::Breach, req)
        }
        "RoleLevel" | "CheckRoleLevel" | "RoleLevelAdd" => {
            let req = get_param_int(lp, "Level").unwrap_or(1);
            eval_role_threshold(ctx, lp, RoleThresholdField::Level, req)
        }
        "RoleFavorLevel" => {
            let req = get_param_int(lp, "Level").unwrap_or(1);
            eval_role_threshold(ctx, lp, RoleThresholdField::Favor, req)
        }
        "RoleResonance" => {
            let req = get_param_int(lp, "GroupIndex").unwrap_or(1);
            eval_role_threshold(ctx, lp, RoleThresholdField::Resonance, req)
        }
        "CheckRoleOwned" => {
            let Some(role_id) = get_param_int(lp, "RoleId") else {
                return false;
            };
            ctx.role_comp
                .is_some_and(|rc| rc.role_map.contains_key(&role_id))
        }
        "PassLordGymDifficulty" => {
            let Some(gym_id) = get_param_int(lp, "Id") else {
                return false;
            };
            ctx.progress_comp
                .is_some_and(|pc| pc.info.has_passed_lord_gym(gym_id))
        }
        "DailyQuestGroupCompleteTimes" => {
            let Some(gid) = get_param_int(lp, "DQGroupId") else {
                return false;
            };
            let need = get_param_int(lp, "DQTimes").unwrap_or(cond.NeedNum);
            ctx.progress_comp
                .is_some_and(|pc| ProgressInfo::get_id_count(&pc.info.counters.daily_quest_group_counts, gid) >= need)
        }
        "CheckDailyCounter" => {
            let Some(ctype) = get_param_int(lp, "CounterType") else {
                return false;
            };
            let extra = get_param_int(lp, "ExtraParam").unwrap_or(0);
            ctx.progress_comp
                .is_some_and(|pc| pc.info.get_daily_counter(ctype, extra) >= cond.NeedNum)
        }
        "LevelPlayNodeStatus" | "CheckLevelPlayNodeState" => {
            let Some(lpid) = get_param_int(lp, "LevelPlayId") else {
                return false;
            };
            let want = get_param_int(lp, "State").unwrap_or(3);
            let done = ctx.quest_comp.info.completed_level_plays.contains(&lpid);
            if want == 3 { done } else { !done }
        }
        "Record" => {
            let Some(rid) = get_param_int(lp, "RecordId") else {
                return false;
            };
            ctx.progress_comp
                .is_some_and(|pc| pc.info.counters.records.contains(&rid))
        }
        "FinishRogueInst" => {
            let Some(iid) = get_param_int(lp, "InstanceId") else {
                return false;
            };
            ctx.progress_comp
                .is_some_and(|pc| pc.info.challenge.finished_rogue_insts.contains(&iid))
        }
        "NewTowerTotalScore" => {
            let season = get_param_int(lp, "SeasonId").unwrap_or(1);
            let diff = get_param_int(lp, "Difficulty").unwrap_or(-1);
            ctx.progress_comp
                .is_some_and(|pc| pc.info.get_new_tower_score(season, diff) >= cond.NeedNum)
        }
        "CheckGuideStatus" => {
            let Some(gid) = get_param_int(lp, "GuideGroupId") else {
                return false;
            };
            let target_status = get_param_int(lp, "Status").unwrap_or(2);
            let finished = ctx.guide_comp.is_some_and(|gc| gc.info.has_finished(gid));
            let cur_status = if finished { 2 } else { 0 };
            let ope_opt = get_ope_str(cond.LimitParamsOpe.as_ref(), "Status");
            compare_ints(cur_status, target_status, ope_opt.unwrap_or("Eq"))
        }
        "CheckLevelOp" => {
            let req = get_param_int(lp, "Level").unwrap_or(1);
            let op = get_param_str(lp, "Op")
                .or_else(|| get_ope_str(cond.LimitParamsOpe.as_ref(), "Level"))
                .unwrap_or("Ge");
            compare_ints(ctx.basic_comp.info.attributes.level, req, op)
        }
        "CheckExplorationLevel" => {
            let req = get_param_int(lp, "Id")
                .or_else(|| get_param_int(lp, "Level"))
                .unwrap_or(1);
            ctx.basic_comp.info.attributes.level >= req
        }
        "CheckFuncOpen" => {
            let Some(fid) = get_param_int(lp, "Id").or_else(|| get_param_int(lp, "FunctionId")) else {
                return false;
            };
            ctx.quest_comp.info.has_unlocked_system(fid)
        }
        "ReduceWorldLevel" => {
            ctx.basic_comp.info.attributes.cur_world_level
                < ctx.basic_comp.info.attributes.origin_world_level
        }
        "VarDefineString" => {
            let Some(vd_name) = get_param_str(lp, "VDName") else {
                return false;
            };
            let expected = get_param_str(lp, "VDValueString").unwrap_or("");
            let actual = ctx.quest_comp.info.get_var_str(vd_name);
            if !actual.is_empty() {
                return actual == expected;
            }
            if let Some(pc) = ctx.progress_comp {
                return pc.info.get_key_str(&format!("var:{vd_name}")) == expected;
            }
            false
        }
        "CheckDungeonId" => {
            let Some(did) = get_param_int(lp, "Id") else {
                return false;
            };
            ctx.quest_comp.info.completed_dungeons.contains(&did)
        }
        "CheckInstPlayNodeState" => {
            let Some(iid) = get_param_int(lp, "InstanceId") else {
                return false;
            };
            ctx.quest_comp.info.completed_dungeons.contains(&iid)
        }
        "LevelPlaysDoneCount" | "ExploreLevelPlayComplete" => {
            let raw = get_param_str(lp, "LevelPlayId").unwrap_or("");
            if raw.is_empty() {
                return (ctx.quest_comp.info.completed_level_plays.len() as i32) >= cond.NeedNum.max(1);
            }
            let count = raw
                .split('-')
                .filter(|tok| !tok.is_empty())
                .filter_map(|tok| tok.parse::<i32>().ok())
                .filter(|lpid| ctx.quest_comp.info.completed_level_plays.contains(lpid))
                .count() as i32;
            count >= cond.NeedNum.max(1)
        }
        "LevelPlayTypeCompleteWithMulti" => {
            (ctx.quest_comp.info.completed_level_plays.len() as i32) >= cond.NeedNum.max(1)
        }
        "PassLordGymDifficultyV2" | "PassLordGymDifficultyWithLevel" => {
            let gid = get_param_int(lp, "Id").unwrap_or(0);
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            if gid > 0 {
                pc.info.has_passed_lord_gym(gid)
            } else {
                !pc.info.challenge.passed_lord_gyms.is_empty()
            }
        }
        "FinishRogueInstDiffRole" | "FinishRogueInstPopularEntrie" => ctx
            .progress_comp
            .is_some_and(|pc| (pc.info.challenge.finished_rogue_insts.len() as i32) >= cond.NeedNum.max(1)),
        "FirstGetRole" => ctx
            .role_comp
            .is_some_and(|rc| (rc.role_map.len() as i32) >= cond.NeedNum.max(1)),
        "FirstRoleBreakthrough" => {
            let req = get_param_int(lp, "Level").unwrap_or(1);
            eval_role_threshold(ctx, lp, RoleThresholdField::Breach, req)
        }
        "SkillLevelAdd" => {
            let req = get_param_int(lp, "Level").unwrap_or(1);
            ctx.role_comp.is_some_and(|rc| {
                rc.role_map
                    .values()
                    .any(|role| role.skills.iter().any(|&(_, lv)| lv >= req))
            })
        }
        "RoleSkillAllNodeMax" => ctx.role_comp.is_some_and(|rc| {
            rc.role_map.values().any(|role| {
                !role.skill_node_state.is_empty()
                    && role.skill_node_state.iter().all(|st| st.active)
            })
        }),
        "WeaponLevelAdd" | "WeaponBreakthrough" => {
            let Some(wc) = ctx.weapon_comp else {
                return false;
            };
            let is_breach = cond.Type == "WeaponBreakthrough";
            let req = if is_breach {
                get_param_int(lp, "BreachLevel")
            } else {
                get_param_int(lp, "Level")
            }
            .unwrap_or(1);
            wc.weapon_map.values().any(|w| {
                let val = if is_breach { w.breach } else { w.level };
                val >= req
            })
        }
        "CapturePhantom"
        | "NewCapturePhantom"
        | "PhantomGetCounter"
        | "StrengthenPhantomMax"
        | "ActivatePhantomFettles"
        | "PhantomIdentify" => {
            let need = cond.NeedNum.max(get_param_int(lp, "Count").unwrap_or(1));
            (ctx.echo_comp.echo_map.len() as i32) >= need
        }
        "CostItemBagType" => ctx
            .progress_comp
            .is_some_and(|pc| pc.info.get_key_int("cond_cnt:CostItemBagType") >= i64::from(cond.NeedNum)),
        "CheckActivityOpen" => {
            let Some(aid) = get_param_int(lp, "ActivityId") else {
                return false;
            };
            let want_open = get_param_int(lp, "IsOpen").unwrap_or(1) != 0;
            is_activity_open(ctx, aid) == want_open
        }
        "SomeQuestComplete" | "SomeTypeQuestComplete" => {
            (ctx.quest_comp.info.finished_quests.len() as i32) >= cond.NeedNum.max(1)
        }
        "RecallQuestState" => {
            let Some(qid) = get_param_int(lp, "RecallQuestId") else {
                return false;
            };
            let st = get_param_int(lp, "State").unwrap_or(3);
            eval_quest_state(ctx, qid, st)
        }
        "SceneGamePlayStatus" => {
            let Some(qid) = get_param_int(lp, "NewQuestId") else {
                return false;
            };
            let Some(nid) = get_param_int(lp, "QuestNodeId") else {
                return false;
            };
            let st = get_param_int(lp, "State").unwrap_or(3);
            eval_quest_node_status(ctx, qid, nid, st)
        }
        "CheckExploreProgress" | "CheckMulExploreProgress" => ctx.progress_comp.is_some_and(|pc| {
            pc.info.get_key_int("cond_cnt:CheckExploreProgress") >= i64::from(cond.NeedNum)
                || pc.info.count_claims_prefix("explore_") >= cond.NeedNum
        }),
        "CheckDungeonFinished" | "NewCheckInstanceState" => {
            if let Some(inst_id) = get_param_int(lp, "InstanceId")
                && inst_id > 0
            {
                let want_finish = get_param_int(lp, "Finish").unwrap_or(1) != 0;
                let finished = ctx.quest_comp.info.completed_dungeons.contains(&inst_id);
                return if want_finish { finished } else { !finished };
            }
            ctx.progress_comp
                .is_some_and(|pc| pc.info.get_key_int("cond_cnt:CheckDungeonFinished") >= i64::from(cond.NeedNum))
        }
        "CheckAccountSettingOpen" => {
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let sid = get_param_int(lp, "Id").unwrap_or(0);
            if sid > 0 && pc.info.get_key_int(&format!("acct_setting:{sid}")) != 0 {
                return true;
            }
            pc.info.get_key_int("cond_cnt:CheckAccountSettingOpen") >= i64::from(cond.NeedNum.max(1))
        }
        "BabelTowerLevelFinishStar" => {
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let lid = get_param_int(lp, "Id").unwrap_or(0);
            if lid > 0 && pc.info.get_key_int(&format!("babel:star:{lid}")) >= i64::from(cond.NeedNum) {
                return true;
            }
            pc.info.get_key_int("babel:star") >= i64::from(cond.NeedNum)
        }
        "RoleTowerPassStage" => {
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let sid = get_param_int(lp, "Id").unwrap_or(0);
            if sid > 0 && pc.info.has_claim(&format!("role_tower:stage:{sid}")) {
                return true;
            }
            pc.info.get_key_int("role_tower:stage") >= i64::from(sid.max(cond.NeedNum))
        }
        "AbyssLittleRoleLevel" => {
            let req = get_param_int(lp, "Level").unwrap_or(1).max(cond.NeedNum);
            ctx.progress_comp
                .is_some_and(|pc| pc.info.get_key_int("abyss:lrole_max_lv") >= i64::from(req))
        }
        "BossPilingLevelBossHpNum" => {
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let lid = get_param_int(lp, "Id").unwrap_or(0);
            if lid > 0 && pc.info.get_key_int(&format!("bp:hp:{lid}")) >= i64::from(cond.NeedNum) {
                return true;
            }
            pc.info.get_key_int("bp:max_hp") >= i64::from(cond.NeedNum)
        }
        "TeamParkOurLevelCheck" => {
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let lid = get_param_int(lp, "Id").unwrap_or(0);
            if lid > 0 && pc.info.get_key_int(&format!("parkour:score:{lid}")) >= i64::from(cond.NeedNum) {
                return true;
            }
            pc.info.get_key_int("parkour:best") >= i64::from(cond.NeedNum)
        }
        "CiacconaChapterResult" => {
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let res_id = get_param_int(lp, "ChapterResult").unwrap_or(0);
            if res_id > 0
                && (pc.info.has_claim(&format!("ciaccona:res:{res_id}"))
                    || pc.info.get_key_int("ciaccona:res") == i64::from(res_id))
            {
                return true;
            }
            pc.info.get_key_int("cond_cnt:CiacconaChapterResult") >= i64::from(cond.NeedNum.max(1))
        }
        "KurotatoFinishLevel" => {
            let lv_id = get_param_int(lp, "Level").unwrap_or(0);
            if lv_id > 0 && ctx.quest_comp.info.completed_dungeons.contains(&lv_id) {
                return true;
            }
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            if lv_id > 0 && pc.info.has_claim(&format!("kurotato:done:{lv_id}")) {
                return true;
            }
            pc.info.get_key_int("kurotato:finish_cnt") >= i64::from(cond.NeedNum.max(1))
        }
        "KurotatoUnlockInfo" => {
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let info_id = get_param_int(lp, "Id").unwrap_or(0);
            if cond.NeedNum <= 1 && info_id > 0 && pc.info.has_claim(&format!("kurotato:info:{info_id}")) {
                return true;
            }
            let cnt = pc
                .info
                .get_key_int("kurotato:unlock_cnt")
                .max(i64::from(pc.info.count_claims_prefix("kurotato:info:")));
            cnt >= i64::from(cond.NeedNum.max(1))
        }
        other => {
            if let Some(spec) = lookup_server_cond_spec(other) {
                return eval_server_spec_condition(ctx, cond, lp, spec);
            }
            false
        }
    }
}

struct ServerCondSpec {
    prog_key: &'static str,
    claim_prefix: &'static str,
    param_id_key: &'static str,
}

impl ServerCondSpec {
    const fn new(prog_key: &'static str) -> Self {
        Self {
            prog_key,
            claim_prefix: "",
            param_id_key: "",
        }
    }

    const fn with_claim(prog_key: &'static str, claim_prefix: &'static str) -> Self {
        Self {
            prog_key,
            claim_prefix,
            param_id_key: "",
        }
    }

    const fn with_param(prog_key: &'static str, param_id_key: &'static str) -> Self {
        Self {
            prog_key,
            claim_prefix: "",
            param_id_key,
        }
    }

    const fn full(
        prog_key: &'static str,
        claim_prefix: &'static str,
        param_id_key: &'static str,
    ) -> Self {
        Self {
            prog_key,
            claim_prefix,
            param_id_key,
        }
    }
}

fn get_ope_str<'a>(ope_opt: Option<&'a Value>, field: &str) -> Option<&'a str> {
    ope_opt.and_then(Value::as_object).and_then(|m| get_param_str(m, field))
}

fn eval_server_spec_condition(
    ctx: &EvalContext<'_>,
    cond: &Condition,
    lp: &Map<String, Value>,
    spec: ServerCondSpec,
) -> bool {
    let Some(pc) = ctx.progress_comp else {
        return false;
    };
    let target = cond.NeedNum.max(
        get_param_int(lp, "Count")
            .or_else(|| get_param_int(lp, "Num"))
            .or_else(|| get_param_int(lp, "Number"))
            .or_else(|| get_param_int(lp, "Level"))
            .unwrap_or(1),
    );

    if !spec.param_id_key.is_empty()
        && let Some(pid) = get_param_int(lp, spec.param_id_key)
        && pid > 0
    {
        if !spec.claim_prefix.is_empty() && pc.info.has_claim(&format!("{}{}", spec.claim_prefix, pid)) {
            return true;
        }
        let val = pc
            .info
            .get_key_int(&format!("{}:{}", spec.prog_key, pid))
            .clamp(0, i64::from(i32::MAX)) as i32;
        return val >= target;
    }

    if !spec.claim_prefix.is_empty() && pc.info.count_claims_prefix(spec.claim_prefix) >= target {
        return true;
    }

    let direct_val = pc
        .info
        .get_key_int(spec.prog_key)
        .clamp(0, i64::from(i32::MAX)) as i32;
    let cond_val = pc
        .info
        .get_key_int(&format!("cond_cnt:{}", cond.Type))
        .clamp(0, i64::from(i32::MAX)) as i32;
    direct_val.max(cond_val) >= target
}

fn lookup_server_cond_spec(cond_type: &str) -> Option<ServerCondSpec> {
    Some(match cond_type {
        "AbyssPass" => ServerCondSpec::full("abyss:settle_inst", "abyss:pass:", "Id"),
        "AddFriends" => ServerCondSpec::new("social:friends_added"),
        "AtmosphereLevelUpdate" => ServerCondSpec::with_claim("diy_atm", "diy_atm:"),
        "BabelTowerLevelFinish" => ServerCondSpec::full("babel:finish", "babel:done:", "Id"),
        "BabelTowerLevelStar" => ServerCondSpec::with_claim("babel:star", "babel_rw:"),
        "BossLinkInstHistoryMaxScore" => ServerCondSpec::with_param("rogue:boss_link_score", "InstanceId"),
        "BossRushScore" => ServerCondSpec::with_claim("bossrush:score", "bossrush:"),
        "BrochureCollectNum" => ServerCondSpec::with_claim("brochure:cnt", "brochure:"),
        "CdkGiftCdkReceived" => ServerCondSpec::new("cdk:received"),
        "CheckDailyLevelPlayAndInstCounter" => ServerCondSpec::new("daily:lp_inst_cnt"),
        "CheckGachaTenTimesItemCount" => ServerCondSpec::new("gacha:ten_times"),
        "CheckGachaTypeConsecutive" => ServerCondSpec::new("gacha:consecutive"),
        "CheckGachaTypeItemQuality" => ServerCondSpec::new("gacha:quality_cnt"),
        "CheckMoonBuilding" => ServerCondSpec::with_claim("trackmoon:bld", "tm_ent:"),
        "CheckMoonBuildingUp" => ServerCondSpec::with_claim("trackmoon:bld_lv", "tm_ent:"),
        "CheckMoonEntrust" => ServerCondSpec::with_claim("trackmoon:entrust", "tm_ent:"),
        "CheckMoonRoleAttribute" => ServerCondSpec::new("trackmoon:role_attr"),
        "CheckMoonRoleLikeLevel" => ServerCondSpec::new("trackmoon:role_like"),
        "CheckOnTreasureBoxOpen" => ServerCondSpec::new("explore:treasure_opened"),
        "CheckPeopleLikeCount" => ServerCondSpec::new("bld:like"),
        "CheckPopularityLevel" => ServerCondSpec::new("bld:pop_lv"),
        "CheckReviveByBuff" => ServerCondSpec::new("combat:revive_buff"),
        "CheckShop" => ServerCondSpec::with_param("shop:buy", "ShopItemId"),
        "CheckTetrisFinish" => ServerCondSpec::with_param("tetris:best", "Id"),
        "CiacconaActivityResult" => ServerCondSpec::new("ciaccona:res"),
        "CompareActorVar" => ServerCondSpec::new("actor_var:done"),
        "CookingLevelUp" => ServerCondSpec::new("cook:level"),
        "CorniceMeetingFinish" => ServerCondSpec::with_claim("cornice:finish", "cornice_rw:"),
        "CreateAdvice" => ServerCondSpec::new("advice:created"),
        "CrossSceneSyncKillMonster" => ServerCondSpec::new("simple_combat:kills"),
        "DailyAdventureCheck" => ServerCondSpec::with_claim("dadv:check", "dadv_"),
        "DangoBrawlComposeQuality" => ServerCondSpec::new("db_uid"),
        "DangoBrawlPassAllBaseFilled" => ServerCondSpec::new("db_fight_cnt"),
        "DangoBrawlPassChallenge" => ServerCondSpec::full("db_fight_cnt", "db_task:", "Id"),
        "DangoBrawlPassItemCountLimit" => ServerCondSpec::new("db_uid"),
        "DangoBrawlPassWithQualityItems" => ServerCondSpec::new("db_uid"),
        "DangoBrawlRegularWinCount" => ServerCondSpec::new("db_round"),
        "DangoMonopolyBoardCompleted" => ServerCondSpec::full("dmono_board", "dmono_board_rw:", "BoardId"),
        "Detection" => ServerCondSpec::new("explore:detection"),
        "DetectionSkillUseCount" => ServerCondSpec::new("explore:detection_skill"),
        "DrawCardCompleteTimes" => ServerCondSpec::new("gacha:total_pulls"),
        "DropCatchActivityScore" => ServerCondSpec::with_param("dc:score", "Id"),
        "EncircleChallengeComplete" => ServerCondSpec::full("encircle:done", "encircle:done:", "Id"),
        "EndlessTetrisScore" => ServerCondSpec::new("tetris:best"),
        "EntityDie" | "EntityDieByTemplate" | "EntityDieByType" => ServerCondSpec::new("simple_combat:kills"),
        "EntityInteractById" | "EntityInteractByType" => ServerCondSpec::new("explore:interact"),
        "ExploreProgressCompleted" => ServerCondSpec::with_claim("explore:prog_done", "explore_prog:"),
        "ExploreProgressCounter" => ServerCondSpec::new("explore:prog_cnt"),
        "ExploreProgressStepRewardTime" => ServerCondSpec::with_claim("explore:step_rw", "explore_prog:"),
        "FarmGoldLevelScore" => ServerCondSpec::full("fg:score", "fg_lp:", "Id"),
        "FinishInst" => ServerCondSpec::new("inst:finish_cnt"),
        "FinishFlipper" => ServerCondSpec::new("flipper:fin"),
        "FinishKuroSimpleCombatKillMonster" => ServerCondSpec::new("simple_combat:kills"),
        "FinishKurotato" => ServerCondSpec::new("kurotato:wave"),
        "FinishRoleTower" => ServerCondSpec::new("role_tower:stage"),
        "FinishRollBlock" => ServerCondSpec::new("rb_blk:fin"),
        "FinishSurvivorsRouge" => ServerCondSpec::new("surv:wave"),
        "FinishTetris" => ServerCondSpec::new("tetris:best"),
        "FinishTrapDefense" => ServerCondSpec::new("trap:pass_wave"),
        "NewHandInItems" => ServerCondSpec::full("handin_pm:cnt", "handin_pm:", "EntityId"),
        "ParallaxAlign" => ServerCondSpec::new("parallax:done"),
        "PlaybackShowTimeline" => ServerCondSpec::new("timeline:done"),
        "TakePicturesWithTimeScale" => ServerCondSpec::new("explore:photo_cnt"),
        "WalkingPattern" => ServerCondSpec::new("walk_pat:done"),
        "FishingEntrust" => ServerCondSpec::with_claim("fish:entrust_done", "fish_ent:"),
        "FishingFish" => ServerCondSpec::new("fish:catch_cnt"),
        "FishingIllustrated" => ServerCondSpec::with_claim("fish:ill_cnt", "fish_ill:"),
        "FishingQuickSell" => ServerCondSpec::new("fish:sell_cnt"),
        "FishingReputationLevel" => ServerCondSpec::new("fish:rep_lv"),
        "FishingSkillUse" => ServerCondSpec::with_param("fish:skill_use", "Id"),
        "FishingTargetEntrustCount" => ServerCondSpec::with_param("fish:target_entrust", "Id"),
        "FishingTechLevel" => ServerCondSpec::with_param("fish:tech_lv", "Id"),
        "FlagChallengePassLevel" => ServerCondSpec::full("flag:pass_lv", "flag_rw:", "Id"),
        "FlagChallengePerLevel" => ServerCondSpec::with_claim("flag:per_lv", "flag_rw:"),
        "FloroRanchCounter" => ServerCondSpec::with_claim("fr:day", "fr_task:"),
        "FoodProcess" => ServerCondSpec::new("cook:process_cnt"),
        "GachaRuleGroupFinishCount" => ServerCondSpec::new("gacha:rule_finish"),
        "GiantTotalStableStage" => ServerCondSpec::with_claim("giant:stage", "giant_sv:"),
        "GivebackScoreReach" => ServerCondSpec::new("giveback:score"),
        "GolemCrackFinish" => ServerCondSpec::with_param("golem:finish", "Id"),
        "HarvestScore" => ServerCondSpec::with_claim("harv:score", "harv_lv:"),
        "HonamiStoryActiveTalent" => ServerCondSpec::full("honami:talent_cnt", "honami:talent:", "Id"),
        "HonamiStoryCheckTowerPass" => ServerCondSpec::new("honami:tower_floor"),
        "HonamiStoryItemCollection" => ServerCondSpec::with_claim("honami:col_cnt", "honami_col:"),
        "HonamiStoryItemSold" => ServerCondSpec::new("honami:sold_val"),
        "HonamiStoryMascotUnlock" => ServerCondSpec::with_claim("honami:mascot_cnt", "honami_mascot:"),
        "HonamiStorySafeLeave" => ServerCondSpec::new("honami:safe_leave"),
        "HonamiStorySettleValue" => ServerCondSpec::new("honami:settle_val"),
        "HonamiStoryTotalRevenue" => ServerCondSpec::new("honami:total_rev"),
        "HonamiStoryWareHouseNoFull" => ServerCondSpec::new("honami:wh_ok"),
        "InfrLevelUp" => ServerCondSpec::with_claim("infr:lv", "infr_lv:"),
        "InfrRoadComplete" => ServerCondSpec::full("infr:road", "infr_road:", "InfrRoadId"),
        "InfrV2LevelUp" => ServerCondSpec::with_claim("infr2:lv", "infr2_lv:"),
        "InstACE" => ServerCondSpec::new("inst:ace_cnt"),
        "KillWithBossRushBuff" => ServerCondSpec::with_param("bossrush:buff_kill", "Id"),
        "KurotatoRoleFinishWave" => ServerCondSpec::new("kurotato:wave"),
        "LongShanRwdCount" => ServerCondSpec::with_claim("ls:rwd_cnt", "longshan_"),
        "MapTravelLevel" => ServerCondSpec::new("map_travel:lv"),
        "MatchThreePass" => ServerCondSpec::with_param("m3:pass", "Id"),
        "MotorFightBestScore" => ServerCondSpec::with_param("motor_fight:best_score", "Id"),
        "MotorFightLevelKillMonster" => ServerCondSpec::with_param("motor_fight:kills", "Id"),
        "MotorFightPassLevel" => ServerCondSpec::with_param("motor_fight:pass", "Id"),
        "MotorGainLiveness" => ServerCondSpec::new("motor:liveness"),
        "MotorGainTp" => ServerCondSpec::new("motor:tp"),
        "MotorLevelUp" => ServerCondSpec::new("motor:lv"),
        "MowTowerLevelScore" => ServerCondSpec::full("mow:score", "mow_rw:", "Id"),
        "NewTowerAllTeamScore" => ServerCondSpec::new("new_tower:all_score"),
        "NewTowerCycleHistoryScore" => ServerCondSpec::new("new_tower:cycle_score"),
        "NewTowerDamage" => ServerCondSpec::new("new_tower:damage"),
        "NewTowerDamageInSeason" => ServerCondSpec::new("new_tower:season_damage"),
        "NewTowerGetScoreNoFiveStarRole" => ServerCondSpec::new("new_tower:no5_score"),
        "NewTowerGetScoreNoInjury" => ServerCondSpec::new("new_tower:noinj_score"),
        "NewTowerGetScoreWithRole" => ServerCondSpec::new("new_tower:role_score"),
        "NewTowerKillBoss" => ServerCondSpec::new("new_tower:kill_boss"),
        "NewTowerKillBossInSeason" => ServerCondSpec::new("new_tower:season_boss"),
        "NewTowerKillWave" => ServerCondSpec::new("new_tower:kill_wave"),
        "NewTowerKillWaveInSeason" => ServerCondSpec::new("new_tower:season_wave"),
        "NewTowerLimitCondPass" => ServerCondSpec::new("new_tower:limit_pass"),
        "NewTowerLimitCondPassWave" => ServerCondSpec::new("new_tower:limit_wave"),
        "NewTowerRecord" => ServerCondSpec::new("new_tower:record"),
        "NewTowerSeasonAwardFinish" => ServerCondSpec::with_claim("new_tower:season_rw", "nt_rw:"),
        "NewTowerTeamChallengingFinish" => ServerCondSpec::full("new_tower:team_finish", "nt_role:", "RoleId"),
        "OnDeathCalculate" => ServerCondSpec::new("combat:death_calc"),
        "OnEnterOrExitBattle" => ServerCondSpec::new("combat:battle_toggle"),
        "OnHonamiStoryLifeSupportChange" => ServerCondSpec::new("honami:ls_lv"),
        "OnPickUpHonamiStoryItem" => ServerCondSpec::new("honami:pickup"),
        "OnPlayerUseSkill" => ServerCondSpec::new("combat:skill_use"),
        "OnSceneItemDurabilityEmpty" => ServerCondSpec::new("explore:durability_empty"),
        "OnSkillEnable" => ServerCondSpec::new("combat:skill_enable"),
        "OnlineKillBoss" => ServerCondSpec::new("coop:kill_boss"),
        "OnlineMotorJoin" => ServerCondSpec::with_claim("motor:online_join", "online_motor:"),
        "OnlineWithFriend" => ServerCondSpec::new("coop:with_friend"),
        "OpenTreasureBox" | "OpenTreasureBoxNew" => ServerCondSpec::new("explore:chest_cnt"),
        "ParkourActivityComplete" => ServerCondSpec::with_claim("parkour:done", "parkour:"),
        "PassSlashAndTowerScore" => ServerCondSpec::with_claim("slash_tower:score", "slash_rw:"),
        "PassTowerDefence" => ServerCondSpec::full("td:pass", "td_inst:", "Id"),
        "PassTowerStarCount" => ServerCondSpec::new("tower:total_stars"),
        "PhantomBattleCardLookoutUp" => ServerCondSpec::with_claim("pb_card_cnt", "pb_card:"),
        "PhantomBattleCollectBadgeNum" => ServerCondSpec::with_claim("pb_badge_cnt", "pb_badge:"),
        "PhantomBattleCollectCardNum" => ServerCondSpec::with_claim("pb_card_cnt", "pb_card:"),
        "PhantomBattleCompleteChallenge" => ServerCondSpec::full("pb:challenge", "pb:challenge:", "ChallengeId"),
        "PhantomBattleIntegrationMaxNum" => ServerCondSpec::new("pb:score"),
        "PhantomBattleLv" => ServerCondSpec::new("pb:lv"),
        "PhantomBattleSkillConditionGroupCardTypeCount" => ServerCondSpec::new("pb:cond_type_cnt"),
        "PhantomBattleSkillConditionGroupCostCount" => ServerCondSpec::new("pb:cond_cost_cnt"),
        "PhantomBattleSkillConditionGroupElementCount" => ServerCondSpec::new("pb:cond_elem_cnt"),
        "Photograph" => ServerCondSpec::new("explore:photo_cnt"),
        "PinballCowScore" => ServerCondSpec::new("pinball:score"),
        "PinballInGameAction" => ServerCondSpec::new("pinball:action_cnt"),
        "PinballLevelPass" => ServerCondSpec::with_claim("pinball:pass", "pinball_task:"),
        "PinballLevelStar" => ServerCondSpec::new("pinball:star"),
        "PinballLimitTaskFinish" => ServerCondSpec::with_claim("pinball:task_cnt", "pinball_task:"),
        "PinballRoleLevel" => ServerCondSpec::new("pinball:role_lv"),
        "PinballWeaponCal" => ServerCondSpec::new("pinball:wp_cal"),
        "RacingBetsTotalOddsFund" => ServerCondSpec::new("rb:odds_fund"),
        "RacingBetsTotalOddsTimes" => ServerCondSpec::new("rb:odds_times"),
        "RealmBetweenLevel" => ServerCondSpec::new("realm:lv"),
        "RecallGetItem" => ServerCondSpec::new("recall:item_cnt"),
        "RhythmDifficultyRankCount" => ServerCondSpec::new("rhythm:rank_cnt"),
        "RhythmRankReach" => ServerCondSpec::with_param("rhythm:best", "Id"),
        "RhythmRoleComplete" => ServerCondSpec::new("rhythm:role_done"),
        "RideSharingInvite" => ServerCondSpec::new("vehicle:share_invite"),
        "RiskHarvestScore" => ServerCondSpec::with_claim("rh:score", "rh_score:"),
        "RoadBookLevel" => ServerCondSpec::new("roadbook:lv"),
        "RogueBlackFlowerReceive" => ServerCondSpec::with_claim("rogue:bf_cnt", "rogue:bf:"),
        "RogueFinishAllTimeLimitTask" => ServerCondSpec::with_claim("rogue:limit_task_cnt", "rogue:season_rw:"),
        "RogueLinkCount" => ServerCondSpec::new("rogue:link_cnt"),
        "RoguePassRoom" => ServerCondSpec::new("rogue:layer"),
        "RogueResCostMood" => ServerCondSpec::new("rogue_res:mood_cost"),
        "RogueResFinishDiffEnd" => ServerCondSpec::with_claim("rogue_res:end_cnt", "rogue_res:end:"),
        "RogueResFinishEvent" => ServerCondSpec::full("rogue_res:evt", "rogue_res:evt:", "Id"),
        "RogueResFinishEventStep" => ServerCondSpec::full("rogue_res:evt_step", "rogue_res:evt_step:", "Id"),
        "RogueResPassInst" => ServerCondSpec::with_param("rogue_res:pass", "Id"),
        "RogueResPassInstWithBond" => ServerCondSpec::new("rogue_res:bond_pass"),
        "RogueResRoleRecruit" => ServerCondSpec::new("rogue_res:recruit"),
        "RogueResTriggerLinkWithBond" => ServerCondSpec::new("rogue_res:bond_link"),
        "RogueRoleConcerto" => ServerCondSpec::new("rogue:concerto"),
        "RogueTalentTreeMax" => ServerCondSpec::new("rogue:talent_max"),
        "RogueTowerTrialFinishLevel" => ServerCondSpec::with_param("rogue_trial:inst", "Id"),
        "RogueUseDiffPhantom" => ServerCondSpec::new("rogue:diff_phantom"),
        "RogueUseDiffRolePassBossLink" => ServerCondSpec::new("rogue:diff_role_boss"),
        "RoleTowerUseDiffRole" => ServerCondSpec::new("role_tower:diff_role"),
        "RougeWeeklyScore" => ServerCondSpec::new("rogue_weekly:score"),
        "RoverRogueBlessingGet" => ServerCondSpec::with_claim("rover_rogue:bless_cnt", "rover_rogue:gain:"),
        "RoverRogueBlessingTypeGet" => ServerCondSpec::new("rover_rogue:bless_type_cnt"),
        "RoverRogueBossPass" => ServerCondSpec::new("rover_rogue:boss_pass"),
        "RoverRogueCondition" => ServerCondSpec::with_param("rover_rogue:cond", "InsId"),
        "RoverRogueElitePass" => ServerCondSpec::new("rover_rogue:elite_pass"),
        "RoverRogueForgeGet" => ServerCondSpec::new("rover_rogue:forge_cnt"),
        "RoverRogueItemGet" => ServerCondSpec::new("rover_rogue:item_cnt"),
        "RoverRogueLootGet" => ServerCondSpec::with_claim("rover_rogue:loot_cnt", "rover_rogue:loot:"),
        "RoverRogueReviveTimes" => ServerCondSpec::new("rover_rogue:revive_cnt"),
        "RoverRogueStageClearCheckHp" => ServerCondSpec::new("rover_rogue:hp_clear"),
        "RoverRogueTalentUnlock" => ServerCondSpec::full("rover_rogue:talent", "rover_rogue:talent:", "TalentId"),
        "ScratchCardAllReward" => ServerCondSpec::new("scratch:all_rw"),
        "SlashTowerEndlessMaxScoreCount" => ServerCondSpec::new("slash_tower:endless_cnt"),
        "SlashTowerFullSeasonReward" => ServerCondSpec::with_claim("slash_tower:full_rw", "slash_rw:"),
        "SomeMonsterKill" => ServerCondSpec::new("simple_combat:kills"),
        "SpringSignInviteNum" => ServerCondSpec::new("ssign:draw_idx"),
        "SurvivorsLevelFinish" => ServerCondSpec::with_param("surv:wave", "Level"),
        "SurvivorsUnlockItem" => ServerCondSpec::new("surv:item_cnt"),
        "SurvivorsUnlockWeapon" => ServerCondSpec::new("surv:wp_cnt"),
        "SurvivorsWeaponEvolve" => ServerCondSpec::new("surv:wp_evo"),
        "Synthesis" => ServerCondSpec::new("craft:synthesis_cnt"),
        "SynthesisLevelUp" => ServerCondSpec::new("craft:synthesis_lv"),
        "SynthesisOrCookingCounter" => ServerCondSpec::new("craft:total_cnt"),
        "TakeChallenge" => ServerCondSpec::with_param("challenge:take", "Id"),
        "TakeLordGymChallenge" => ServerCondSpec::new("challenge:lord_gym"),
        "TakeSceneItem" => ServerCondSpec::new("explore:pickup_cnt"),
        "TotalFinishInstTimesWithMulti" => ServerCondSpec::new("inst:multi_cnt"),
        "TowerDefenceLevelScore" => ServerCondSpec::with_param("td:level_score", "Id"),
        "TowerDefenceScore" => ServerCondSpec::with_claim("td:total_score", "td_score:"),
        "TowerLimitedPass" => ServerCondSpec::new("tower:limited_pass"),
        "TowerNoHarmPass" => ServerCondSpec::new("tower:no_harm_pass"),
        "TowerPassMaxDifficultCount" => ServerCondSpec::new("tower:max_diff_cnt"),
        "TowerPerfectPass" => ServerCondSpec::new("tower:perfect_cnt"),
        "TrapDefenseAuxiliaryUnlockCount" => ServerCondSpec::new("trap:aux_cnt"),
        "TrapDefenseBuildingUnlock" => ServerCondSpec::new("trap:bld_unlock"),
        "TrapDefenseBuildingUnlockCount" => ServerCondSpec::new("trap:bld_cnt"),
        "TrapDefenseChallengeStar" => ServerCondSpec::with_param("trap:star", "ChallengeId"),
        "TrapDefenseConsecutiveKill" => ServerCondSpec::new("trap:streak_kill"),
        "TrapDefenseEnvironmentalKill" => ServerCondSpec::new("trap:env_kill"),
        "TrapDefenseGainWaveBuff" => ServerCondSpec::new("trap:wave_buff"),
        "TrapDefensePassFullStar" => ServerCondSpec::with_param("trap:full_star", "ChallengeId"),
        "TrapDefensePassWave" => ServerCondSpec::with_param("trap:pass_wave", "ChallengeId"),
        "TrapDefenseTotalBuildingBranch" => ServerCondSpec::new("trap:bld_branch_cnt"),
        "TrapDefenseTotalStar" => ServerCondSpec::new("trap:total_star"),
        "TrapDefenseUnlockTech" => ServerCondSpec::full("trap:tech", "trap:tech:", "Id"),
        "TrapDefenseUnlockTechCount" => ServerCondSpec::with_claim("trap:tech_cnt", "trap:tech:"),
        "TriggerInterative" => ServerCondSpec::new("explore:interactive"),
        "TurnTableBigRewardCount" => ServerCondSpec::new("turntable:spin"),
        "UseSkill" => ServerCondSpec::with_param("combat:skill_use", "SkillId"),
        "WeekInstanceCount" => ServerCondSpec::new("inst:week_cnt"),
        "WeeklyFrameTotalScore" => ServerCondSpec::new("adv:weekly_score"),
        "WuWuKujiRewardCount" => ServerCondSpec::with_claim("wuwu:spin", "wuwu_"),
        "WuWuWeekCheckInDay" => ServerCondSpec::new("wuwu_week:checkin"),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
enum RoleThresholdField {
    Breach,
    Level,
    Favor,
    Resonance,
}

fn eval_role_threshold(
    ctx: &EvalContext<'_>,
    lp: &Map<String, Value>,
    field: RoleThresholdField,
    req: i32,
) -> bool {
    let param_role = get_param_int(lp, "RoleId").unwrap_or(0);
    let role_id = if param_role != 0 {
        param_role
    } else {
        ctx.target_role_id
    };
    let Some(rc) = ctx.role_comp else {
        return false;
    };
    if role_id == 0 {
        return rc
            .role_map
            .values()
            .any(|role| role_meets_threshold(role, field, req));
    }
    rc.role_map
        .get(&role_id)
        .is_some_and(|role| role_meets_threshold(role, field, req))
}

fn role_meets_threshold(role: &RoleInfo, field: RoleThresholdField, req: i32) -> bool {
    match field {
        RoleThresholdField::Breach => role.breakthrough >= req,
        RoleThresholdField::Level => role.level >= req,
        RoleThresholdField::Favor => role.favor >= req,
        RoleThresholdField::Resonance => {
            let active_reson = role.reson.iter().filter(|r| r.open).count() as i32;
            role.resonant_chain_group_index.max(active_reson) >= req
        }
    }
}

pub fn eval_quest_provide_conditions(ctx: &EvalContext<'_>, provide_val: Option<&Value>) -> bool {
    let Some(pv) = provide_val.and_then(Value::as_object) else {
        return true;
    };
    let Some(conds_val) = pv.get("Conditions").and_then(Value::as_array) else {
        return true;
    };
    if conds_val.is_empty() {
        return true;
    }

    let is_or = json_int(pv.get("Type")).unwrap_or(0) == 1;
    for cond_item in conds_val {
        let passed = eval_single_provide_condition(ctx, cond_item);
        if is_or && passed {
            return true;
        }
        if !is_or && !passed {
            return false;
        }
    }
    !is_or
}

fn eval_system_state_provide_condition(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(cfg) = obj.get("Config").and_then(Value::as_object) else {
        return false;
    };
    let st = cfg.get("Type").and_then(Value::as_str).unwrap_or("");
    let Some(pc) = ctx.progress_comp else {
        return false;
    };

    match st {
        "TrackMoonBuilding" => {
            let Some(bid) = json_int(cfg.get("BuildingId")) else {
                return false;
            };
            let want_built = cfg.get("IsBuilt").and_then(Value::as_bool).unwrap_or(true);
            let is_built = pc.info.get_key_int(&format!("trackmoon:bld:{bid}")) > 0
                || pc.info.get_key_int(&format!("track_moon:bld:{bid}")) > 0;
            is_built == want_built
        }
        "RoadBuildState" => {
            let Some(rid) = json_int(cfg.get("RoadId")) else {
                return false;
            };
            let want_built = cfg.get("IsBuilt").and_then(Value::as_bool).unwrap_or(true);
            let k1 = format!("infr:road:{rid}");
            let k2 = format!("road_build:{rid}");
            let is_built = pc.info.has_claim(&k1)
                || pc.info.get_key_int(&k1) > 0
                || pc.info.has_claim(&k2)
                || pc.info.get_key_int(&k2) > 0;
            is_built == want_built
        }
        "TrackMoonPopularity" => {
            let Some(req) = json_int(cfg.get("Popularity")) else {
                return false;
            };
            let cmp = cfg.get("Compare").and_then(Value::as_str).unwrap_or("Ge");
            let raw = pc
                .info
                .get_key_int("track_moon:pop")
                .max(pc.info.get_key_int("trackmoon:popular"));
            let cur = raw.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            compare_ints(cur, req, cmp)
        }
        "InfrActivityExp" => {
            let Some(req) = json_int(cfg.get("Exp")) else {
                return false;
            };
            let cmp = cfg.get("Compare").and_then(Value::as_str).unwrap_or("Ge");
            let cur = pc
                .info
                .get_key_int("infr:exp")
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            compare_ints(cur, req, cmp)
        }
        "InfrastructureStage" => {
            let Some(req) = json_int(cfg.get("InfrastructureStage")) else {
                return false;
            };
            pc.info.get_key_int("infr:stage") >= i64::from(req)
        }
        "SpringFestivalAtmosphereLevel" => {
            let Some(req) = json_int(cfg.get("AtmosphereLevel")) else {
                return false;
            };
            pc.info
                .get_key_int("spring:atmosphere")
                .max(pc.info.get_key_int("spring_manor:atmosphere"))
                >= i64::from(req)
        }
        "VillageInfrLevel" => {
            let Some(req) = json_int(cfg.get("Level")) else {
                return false;
            };
            pc.info
                .get_key_int("village:level")
                .max(pc.info.get_key_int("infr2:lvl"))
                >= i64::from(req)
        }
        "VillageInfrTreeState" => {
            let Some(tid) = json_int(cfg.get("Id")) else {
                return false;
            };
            let want_built = cfg.get("IsBuilt").and_then(Value::as_bool).unwrap_or(true);
            let k1 = format!("infr2:tree:{tid}");
            let k2 = format!("village:tree:{tid}");
            let is_built = pc.info.has_claim(&k1)
                || pc.info.get_key_int(&k1) > 0
                || pc.info.get_key_int(&k2) > 0;
            is_built == want_built
        }
        "DropCatchGameplayCompleteLevel" => {
            let Some(lid) = json_int(cfg.get("LevelId")) else {
                return false;
            };
            let k1 = format!("drop_catch:done:{lid}");
            let k2 = format!("dc:score:{lid}");
            pc.info.has_claim(&k1) || pc.info.get_key_int(&k1) > 0 || pc.info.get_key_int(&k2) > 0
        }
        "RhythmSpaceshipReachLevelRank" => {
            let Some(lid) = json_int(cfg.get("RhythmSpaceshipLevelId")) else {
                return false;
            };
            let req_rank = json_int(cfg.get("Rank")).unwrap_or(1);
            let cur_rank = pc.info.get_key_int(&format!("rhythm:rank:{lid}"));
            cur_rank > 0 && cur_rank <= i64::from(req_rank)
        }
        "SpringFestivalGuessJokerFirstComplete" => {
            let Some(lid) = json_int(cfg.get("LevelId")) else {
                return false;
            };
            let k1 = format!("guess_joker:done:{lid}");
            let k2 = format!("joker_rw:{lid}");
            pc.info.has_claim(&k1) || pc.info.get_key_int(&k1) > 0 || pc.info.has_claim(&k2)
        }
        "DangoBrawlChallengeComplete" => {
            let Some(cid) = json_int(cfg.get("ChallengeId")) else {
                return false;
            };
            let k1 = format!("dango:challenge:{cid}");
            let k2 = format!("db_task:{cid}");
            pc.info.has_claim(&k1) || pc.info.get_key_int(&k1) > 0 || pc.info.has_claim(&k2)
        }
        "CollectionShopFull" => {
            let shop_type = cfg.get("ShopType").and_then(Value::as_str).unwrap_or("");
            if shop_type.is_empty() {
                return false;
            }
            let key = format!("coll_shop:full:{shop_type}");
            pc.info.has_claim(&key) || pc.info.get_key_int(&key) > 0
        }
        "EncircleLevelComplete" => {
            let Some(eid) = json_int(cfg.get("EncircleLevelId")) else {
                return false;
            };
            let want_done = cfg.get("IsCompleted").and_then(Value::as_bool).unwrap_or(true);
            let key = format!("encircle:done:{eid}");
            let is_done = pc.info.has_claim(&key) || pc.info.get_key_int(&key) > 0;
            is_done == want_done
        }
        _ => false,
    }
}

fn eval_single_provide_condition(ctx: &EvalContext<'_>, cond_item: &Value) -> bool {
    let Some(obj) = cond_item.as_object() else {
        return true;
    };
    let Some(t_val) = obj.get("Type") else {
        return true;
    };
    if t_val.is_i64() || t_val.is_u64() {
        return eval_inline_condition_block(ctx, cond_item);
    }
    let Some(ctype) = t_val.as_str() else {
        return true;
    };
    match ctype {
        "PreQuest" | "PreChildQuest" => eval_single_inline_condition(ctx, cond_item),
        "ExploreLevel" => {
            let req = json_int(obj.get("ExploreLevel")).unwrap_or(1);
            ctx.basic_comp.info.attributes.level >= req
        }
        "PreLevelPlay" => {
            let Some(lpid) = json_int(obj.get("PreLevelPlay")) else {
                return false;
            };
            ctx.quest_comp.info.completed_level_plays.contains(&lpid)
        }
        "PreDungeon" => {
            let Some(sub) = obj.get("Dungeon").and_then(Value::as_object) else {
                return false;
            };
            let Some(did) = json_int(sub.get("DungeonId")) else {
                return false;
            };
            ctx.quest_comp.info.completed_dungeons.contains(&did)
        }
        "Item" => {
            let Some(items_val) = obj.get("Items").and_then(Value::as_array) else {
                return false;
            };
            for it in items_val {
                let Some(it_obj) = it.as_object() else {
                    continue;
                };
                let Some(iid) = json_int(it_obj.get("ItemId")) else {
                    continue;
                };
                let cnt = json_int(it_obj.get("Count")).unwrap_or(1);
                if item_count(ctx, iid) < cnt {
                    return false;
                }
            }
            true
        }
        "Gender" => eval_player_gender(ctx, obj.get("Gender")),
        "SystemState" => eval_system_state_provide_condition(ctx, obj),
        "OpenDate" => eval_open_date_condition(ctx, obj),
        _ => eval_single_inline_condition(ctx, cond_item),
    }
}

pub fn eval_inline_condition_block(ctx: &EvalContext<'_>, block_val: &Value) -> bool {
    let Some(obj) = block_val.as_object() else {
        return true;
    };
    let Some(conds_val) = obj.get("Conditions").and_then(Value::as_array) else {
        return true;
    };
    if conds_val.is_empty() {
        return true;
    }

    // Type 0 = AND, Type 1 = OR
    let is_or = json_int(obj.get("Type")).unwrap_or(0) == 1;
    for c in conds_val {
        let passed = eval_single_inline_condition(ctx, c);
        if is_or && passed {
            return true;
        }
        if !is_or && !passed {
            return false;
        }
    }
    !is_or
}

fn resolve_current_entity_state_cow<'a>(
    ctx: &'a EvalContext<'_>,
    eid: i32,
) -> std::borrow::Cow<'a, str> {
    if let Some(saved) = ctx.quest_comp.info.get_entity_state(i64::from(eid)) {
        return std::borrow::Cow::Borrowed(saved);
    }
    if let Some(assets) = ctx.assets {
        let inst_id = ctx.scene.map_or(8, |s| s.instance_id);
        if let Some(lcfg) = assets.find_level_entity(inst_id, i64::from(eid)) {
            let mut comps = lcfg.ComponentsData.clone();
            if let Some(tpl) = assets.find_template_config(&lcfg.BlueprintType) {
                tpl.ComponentsData.merge_into(&mut comps);
            }
            if let Some(esc) = comps.EntityStateComponent
                && let Some(st) = esc.State
                && !st.is_empty()
            {
                return std::borrow::Cow::Owned(st);
            }
        }
    }
    std::borrow::Cow::Borrowed("关卡.Common.状态.常态")
}

fn eval_entity_state_compare(ctx: &EvalContext<'_>, eid: i32, obj: &Map<String, Value>) -> bool {
    let Some(want_state) = obj.get("State").and_then(Value::as_str) else {
        return false;
    };
    let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
    let cur = resolve_current_entity_state_cow(ctx, eid);
    let eq = cur.as_ref() == want_state;
    if is_neg_compare(cmp) { !eq } else { eq }
}

pub fn eval_single_inline_condition(ctx: &EvalContext<'_>, c_val: &Value) -> bool {
    let Some(obj) = c_val.as_object() else {
        return true;
    };
    let Some(t_val) = obj.get("Type") else {
        return true;
    };
    if t_val.is_i64() || t_val.is_u64() {
        return eval_inline_condition_block(ctx, c_val);
    }
    eval_single_inline_condition_obj(ctx, obj)
}

pub fn eval_single_inline_condition_obj(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(ct) = obj.get("Type").and_then(Value::as_str) else {
        return true;
    };

    match ct {
        "CompareVar" => eval_compare_var_condition(ctx, obj),
        "CheckChildQuestFinished" => {
            let Some(qid) = json_int(obj.get("QuestId")) else {
                return false;
            };
            let Some(cid) = json_int(obj.get("ChildQuestId")) else {
                return false;
            };
            ctx.quest_comp.info.has_finished_child_quest(qid, cid)
        }
        "CheckQuestFinished" => {
            let Some(qid) = json_int(obj.get("QuestId")) else {
                return false;
            };
            ctx.quest_comp.info.has_finished_quest(qid)
        }
        "CheckItems" => {
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Ge");
            let Some(items) = obj.get("Items").and_then(Value::as_array) else {
                return false;
            };
            if items.is_empty() {
                return true;
            }
            for it in items {
                let Some(it_obj) = it.as_object() else {
                    continue;
                };
                let Some(iid) = json_int(it_obj.get("ItemId")) else {
                    continue;
                };
                let cnt = json_int(it_obj.get("Count")).unwrap_or(1);
                if !compare_ints(item_count(ctx, iid), cnt, cmp) {
                    return false;
                }
            }
            true
        }
        "CompareEntityState" => {
            let Some(eid) = json_int(obj.get("EntityId")) else {
                return false;
            };
            eval_entity_state_compare(ctx, eid, obj)
        }
        "CompareEntitySelfState" => {
            if ctx.self_entity_config_id <= 0 {
                return false;
            }
            eval_entity_state_compare(ctx, ctx.self_entity_config_id, obj)
        }
        "CheckLevelPlay" => {
            if let Some(lpid) = json_int(obj.get("LevelPlayId")) {
                return ctx.quest_comp.info.completed_level_plays.contains(&lpid);
            }
            if let Some(ids_val) = obj.get("LevelPlayIds").and_then(Value::as_array) {
                let req = json_int(obj.get("CompleteNumber")).unwrap_or(1).max(1);
                let done = ids_val
                    .iter()
                    .filter_map(|v| json_int(Some(v)))
                    .filter(|lpid| ctx.quest_comp.info.completed_level_plays.contains(lpid))
                    .count() as i32;
                return done >= req;
            }
            false
        }
        "CheckDungeon" => {
            let Some(did) = json_int(obj.get("DungeonId")) else {
                return false;
            };
            ctx.quest_comp.info.completed_dungeons.contains(&did)
        }
        "CheckActivityState" => {
            let Some(aid) = json_int(obj.get("ActivityId")) else {
                return false;
            };
            let want = json_int(obj.get("ActivityState")).unwrap_or(2);
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            let cur = if is_activity_open(ctx, aid) { 2 } else { 0 };
            compare_ints(cur, want, cmp)
        }
        "CheckPlayerGender" => eval_player_gender(ctx, obj.get("Gender")),
        "OpenDate" => eval_open_date_condition(ctx, obj),
        "CheckPlayerMoraleLevelRange" => {
            let Some(min_level) = json_int(obj.get("MinMoraleLevel")) else {
                return false;
            };
            let Some(max_level) = json_int(obj.get("MaxMoraleLevel")) else {
                return false;
            };
            if min_level > max_level {
                return false;
            }
            let current_level = morale::resolve_current_keep_level(
                ctx.assets,
                ctx.inventory_comp,
                ctx.progress_comp,
            );
            current_level >= min_level && current_level <= max_level
        }
        "CheckCurrentRole" => {
            let Some(role_id) = json_int(obj.get("RoleId")) else {
                return false;
            };
            let Some(scene) = ctx.scene else {
                return false;
            };
            let Ok(formation_index) = usize::try_from(scene.formation_info.cur_formation) else {
                return false;
            };
            let Some(formation) = scene.formation_info.formations.get(formation_index) else {
                return false;
            };
            let current_role_id = formation.cur_role;
            if current_role_id <= 0 || role_id <= 0 {
                return false;
            }
            let current_role_exists = formation
                .roles
                .iter()
                .flatten()
                .any(|role| role.role_id == current_role_id);
            if !current_role_exists {
                return false;
            }
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            compare_ints(current_role_id, role_id, cmp)
        }
        "CompareFishingPrestigeLevel" => {
            let Some(required) = json_int(obj.get("PrestigeLevel")) else {
                return false;
            };
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Ge");
            let Some(assets) = ctx.assets else {
                return false;
            };
            compare_ints(
                fishing_prestige_level(assets, item_count(ctx, 35)),
                required,
                cmp,
            )
        }
        "CheckDangoCultivationProgress" => {
            let Some(check_type) = obj.get("CheckType").and_then(Value::as_object) else {
                return false;
            };
            if !json_has_type(check_type, "CountDangoOverTargetLevel") {
                return false;
            }
            let Some(target_level) = json_int(check_type.get("TargetLevel")) else {
                return false;
            };
            let Some(target_number) = json_int(check_type.get("TargetNumber")) else {
                return false;
            };
            if target_level < 0 || target_number < 0 {
                return false;
            }
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let Some(assets) = ctx.assets else {
                return false;
            };
            count_dango_above_level(assets, &progress.info, target_level) >= target_number
        }
        "CompareAlertValue" => {
            let Some(area_id) = json_int(obj.get("AreaId")) else {
                return false;
            };
            let Some(compare_type) = obj.get("CompareType").and_then(Value::as_str) else {
                return false;
            };
            let Some(compare_value) = obj.get("CompareValue").and_then(Value::as_object) else {
                return false;
            };
            let Some(value_type) = json_int(compare_value.get("Type")) else {
                return false;
            };
            if value_type != 0 && value_type != 1 {
                return false;
            }
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let key = format!("alert:area:{area_id}");
            let cur = i32::from(progress.info.get_key_int(&key) != 0);
            compare_ints(cur, value_type, compare_type)
        }
        "CheckTeleControlState" => {
            let Some(config_id) = json_int(obj.get("EntityId")) else {
                return false;
            };
            let Some(wanted) = obj.get("State").and_then(Value::as_str) else {
                return false;
            };
            let Some(scene) = ctx.scene else {
                return false;
            };
            if scene.find_entity_net_id_by_config_id(config_id).is_none() {
                return false;
            }
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let state = progress.info.get_key_int(&format!("item_bb:{config_id}:0"));
            match wanted {
                "Hold" => state == 4,
                "LetGo" => state == 11,
                "Throwing" => matches!(state, 6..=9),
                _ => false,
            }
        }
        "ListenEntityThroughPortal" => {
            let Some(portal_id) = json_int(obj.get("PortalEntityId")) else {
                return false;
            };
            let Some(target) = obj.get("CheckTarget").and_then(Value::as_object) else {
                return false;
            };
            if !json_has_type(target, "Player") {
                return false;
            }
            ctx.scene.is_some_and(|s| s.last_passed_portal_id == portal_id)
        }
        "CheckIsTargetHoldElementalBox" => {
            let Some(target) = obj.get("Target").and_then(Value::as_object) else {
                return false;
            };
            if !json_has_type(target, "Player") {
                return false;
            }
            let Some(want_holding) = obj.get("IsHolding").and_then(Value::as_bool) else {
                return false;
            };
            let Some(scene) = ctx.scene else {
                return false;
            };
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let Some(assets) = ctx.assets else {
                return false;
            };
            let wanted_box = json_int(obj.get("ElementalBoxId"));
            let wanted_type = get_param_str(obj, "TargetElementalType");
            let mut is_holding_matching_box = false;
            for ent in &scene.entities {
                let cid = ent.config.config_id;
                if progress.info.get_key_int(&format!("item_bb:{cid}:0")) != 4 {
                    continue;
                }
                let Some(bx) = assets.tables.elemental_box_define.get_data_by_id(&cid) else {
                    continue;
                };
                if wanted_box.is_some_and(|box_id| box_id != cid) {
                    continue;
                }
                if wanted_type.is_some_and(|et| bx.ElementalType != et) {
                    continue;
                }
                is_holding_matching_box = true;
                break;
            }
            is_holding_matching_box == want_holding
        }
        "CheckGiantStableValue" => {
            let Some(option) = obj.get("StableValueOption").and_then(Value::as_object) else {
                return false;
            };
            if !json_has_type(option, "Region") {
                return false;
            }
            let eco_id = match get_param_str(option, "EcoType") {
                Some("Fire") => 0,
                Some("Water") => 1,
                Some("Wood") => 2,
                _ => return false,
            };
            let Some(range) = option.get("Range").and_then(Value::as_object) else {
                return false;
            };
            let Some(min_obj) = range.get("MinValue").and_then(Value::as_object) else {
                return false;
            };
            let Some(max_obj) = range.get("MaxValue").and_then(Value::as_object) else {
                return false;
            };
            let Some(min_value) = json_int(min_obj.get("Value")) else {
                return false;
            };
            let Some(max_value) = json_int(max_obj.get("Value")) else {
                return false;
            };
            let Some(min_boundary) = get_param_str(min_obj, "Boundary") else {
                return false;
            };
            let Some(max_boundary) = get_param_str(max_obj, "Boundary") else {
                return false;
            };
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let stable = progress
                .info
                .get_key_int(&format!("giant:stable:{eco_id}"))
                .clamp(0, i64::from(i32::MAX)) as i32;
            let min_ok = if min_boundary == "Closed" {
                stable >= min_value
            } else {
                stable > min_value
            };
            let max_ok = if max_boundary == "Closed" {
                stable <= max_value
            } else {
                stable < max_value
            };
            min_ok && max_ok
        }
        "CheckFormationRoleInfo" => eval_formation_role_info(ctx, obj),
        "CheckEntityGravityDirection" => eval_entity_gravity_direction(ctx, obj),
        "CheckEntityLocked" => {
            let Some(scene) = ctx.scene else {
                return false;
            };
            let Some(entities_val) = obj.get("Entities").and_then(Value::as_array) else {
                return false;
            };
            if entities_val.is_empty() {
                return false;
            }
            let Some(want_locked) = obj.get("IsLocked").and_then(Value::as_bool) else {
                return false;
            };
            let Some(assets) = ctx.assets else {
                return false;
            };
            let Some(locked_tag_id) =
                gameplay_tags::id_from_name(&assets.tables.gameplay_tag_remap, "关卡.Common.属性.锁定")
            else {
                return false;
            };
            for entity_val in entities_val {
                let Some(entity_id) = json_int(Some(entity_val)) else {
                    return false;
                };
                let Some(ent) = scene
                    .entities
                    .iter()
                    .find(|e| e.config.config_id == entity_id)
                else {
                    return false;
                };
                let is_locked = ent
                    .tag
                    .as_ref()
                    .is_some_and(|tag| tag.has_tag(&assets.tables.gameplay_tag_parent, locked_tag_id));
                if is_locked != want_locked {
                    return false;
                }
            }
            true
        }
        "CompareTimePeriod" | "TimePeriod" => {
            let Some(period) = get_param_str(obj, "TimePeriod") else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            eval_time_period_match(ctx, period, cmp)
        }
        "PreQuest" => {
            let Some(pre_id) = json_int(obj.get("PreQuestId")).or_else(|| json_int(obj.get("PreQuest"))) else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let done = ctx.quest_comp.info.has_finished_quest(pre_id);
            if is_neg_compare(cmp) { !done } else { done }
        }
        "PreChildQuest" => {
            let Some(sub) = obj.get("PreChildQuest").and_then(Value::as_object) else {
                return false;
            };
            let Some(qid) = json_int(sub.get("QuestId")) else {
                return false;
            };
            let Some(cid) = json_int(sub.get("ChildQuestId")) else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let done = ctx.quest_comp.info.has_finished_child_quest(qid, cid);
            if is_neg_compare(cmp) { !done } else { done }
        }
        "Clock" => {
            let (hour, _) = current_map_hour_minute(ctx);
            let start_h = json_int(obj.get("Start")).unwrap_or(0);
            let end_h = json_int(obj.get("End")).unwrap_or(24);
            let in_span = if start_h == end_h {
                hour == start_h
            } else if start_h < end_h {
                hour >= start_h && hour < end_h
            } else {
                hour >= start_h || hour < end_h
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            if is_neg_compare(cmp) { !in_span } else { in_span }
        }
        "CheckConditionGroup" | "CheckClientConditionGroup" => {
            if let Some(sub) = obj.get("Condition") {
                return eval_inline_condition_block(ctx, sub);
            }
            if let Some(gid) = json_int(obj.get("GroupId")) {
                return eval_condition_group(ctx, gid);
            }
            true
        }
        "ReadMail" => json_int(obj.get("MailId"))
            .is_some_and(|mid| ctx.quest_comp.info.progress.read_mails.contains(&mid)),
        "ReadPhoneMessage" => json_int(obj.get("PhoneMessageId"))
            .is_some_and(|pmid| ctx.quest_comp.info.progress.read_phone_messages.contains(&pmid)),
        "ReceiveTelecom" => json_int(obj.get("TelecomId"))
            .is_some_and(|tid| ctx.quest_comp.info.progress.received_telecoms.contains(&tid)),
        "FinishBvbChallenge" => json_int(obj.get("BvbChallengeId"))
            .is_some_and(|bid| ctx.quest_comp.info.progress.completed_bvb.contains(&bid)),
        "CompareQuestState" | "CheckQuestState" => {
            let Some(qid) = json_int(obj.get("QuestId")) else {
                return false;
            };
            let want_st = json_int(obj.get("State")).unwrap_or(3);
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            compare_ints(quest_state_value(ctx, qid), want_st, cmp)
        }
        "CheckNodeStatus" | "CheckChildQuestStatus" => {
            let Some(nid) = json_int(obj.get("NodeId")) else {
                return false;
            };
            let want_st = json_int(obj.get("Status")).unwrap_or(4);
            let fallback_qid = if ctx.owner_quest_id > 0 {
                ctx.owner_quest_id
            } else {
                ctx.quest_comp.info.tracked_quest_id
            };
            let qid = json_int(obj.get("QuestId")).unwrap_or(fallback_qid);
            eval_inline_node_status(ctx, qid, nid, want_st)
        }
        "CheckIsTrackingCurrentQuest" => {
            if let Some(qid) = json_int(obj.get("QuestId")) {
                ctx.quest_comp.info.tracked_quest_id == qid
            } else {
                ctx.quest_comp.info.tracked_quest_id > 0
            }
        }
        "CheckIsTrackingQuest" => {
            let Some(qid) = json_int(obj.get("QuestId")) else {
                return false;
            };
            let want_track = obj.get("IsTracking").and_then(Value::as_bool).unwrap_or(true);
            (ctx.quest_comp.info.tracked_quest_id == qid) == want_track
        }
        "Directly" => {
            let Some(eid) = json_int(obj.get("EntityId")) else {
                return false;
            };
            let Some(want_state) = obj.get("State").and_then(Value::as_str) else {
                return false;
            };
            let cur = ctx
                .quest_comp
                .info
                .get_entity_state(i64::from(eid))
                .unwrap_or("关卡.Common.状态.常态");
            cur == want_state
        }
        "HourToHour" => {
            let start_obj = obj.get("Start").and_then(Value::as_object);
            let end_obj = obj.get("End").and_then(Value::as_object);
            let (hour, minute) = current_map_hour_minute(ctx);
            let cur_total = hour * 60 + minute;
            let s_h = start_obj.and_then(|so| json_int(so.get("Hour"))).unwrap_or(0);
            let s_m = start_obj.and_then(|so| json_int(so.get("Min"))).unwrap_or(0);
            let e_h = end_obj.and_then(|eo| json_int(eo.get("Hour"))).unwrap_or(24);
            let e_m = end_obj.and_then(|eo| json_int(eo.get("Min"))).unwrap_or(0);
            let start_total = s_h * 60 + s_m;
            let end_total = e_h * 60 + e_m;
            let in_span = if start_total <= end_total {
                cur_total >= start_total && cur_total <= end_total
            } else {
                cur_total >= start_total || cur_total <= end_total
            };
            let is_ne = match obj.get("Compare") {
                Some(Value::String(s)) => is_neg_compare(s),
                Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
                _ => false,
            };
            if is_ne { !in_span } else { in_span }
        }
        "CheckLevel" => {
            let req = json_int(obj.get("Level")).unwrap_or(1);
            ctx.basic_comp.info.attributes.level >= req
        }
        "CompareExploreLevel" => {
            let req = json_int(obj.get("Level")).unwrap_or(1);
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Ge");
            compare_ints(ctx.basic_comp.info.attributes.level, req, cmp)
        }
        "CheckOriginWorldLevel" => {
            let req = json_int(obj.get("Level")).unwrap_or(0);
            ctx.basic_comp.info.attributes.origin_world_level >= req
        }
        "CheckCurWorldLevel" => {
            let req = json_int(obj.get("Level")).unwrap_or(0);
            ctx.basic_comp.info.attributes.cur_world_level >= req
        }
        "CompareDungeonId" => {
            let Some(did) = json_int(obj.get("DungeonId")) else {
                return false;
            };
            let Some(scene) = ctx.scene else {
                return false;
            };
            if scene.instance_id <= 0 || did <= 0 {
                return false;
            }
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            let eq = scene.instance_id == did;
            if is_neg_compare(cmp) { !eq } else { eq }
        }
        "CheckDungeonFinish" => {
            let Some(did) = json_int(obj.get("DungeonId")) else {
                return false;
            };
            let want_finished = obj.get("IsFinished").and_then(Value::as_bool).unwrap_or(true);
            ctx.quest_comp.info.completed_dungeons.contains(&did) == want_finished
        }
        "CheckInRange" | "CheckInRangeProperly" => {
            let want_in = obj.get("InRange").and_then(Value::as_bool).unwrap_or(true);
            eval_check_in_range(ctx, obj) == want_in
        }
        "CheckSystemState" => eval_system_state_provide_condition(ctx, obj),
        "CheckVehicleCondition" => obj
            .get("Condition")
            .is_some_and(|sub| eval_single_inline_condition(ctx, sub)),
        "CheckInCombat" => {
            let Some(want_in_combat) = obj.get("InCombat").and_then(Value::as_bool) else {
                return false;
            };
            ctx.scene
                .is_some_and(|scene| scene.player_in_battle == want_in_combat)
        }
        "ComparePlayerMotionState" | "ComparePlayerMotionState2" => {
            let motion = obj.get("MotionState").and_then(Value::as_str).unwrap_or("");
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            let is_ne = cmp == "Ne";
            if cmp != "Eq" && !is_ne {
                return false;
            }
            let Some(matched) = eval_player_motion_state(ctx, motion) else {
                return false;
            };
            if is_ne { !matched } else { matched }
        }
        "UsingVehicle" => {
            let want_used = obj
                .get("CheckIsBeingUsed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut target_vid = i64::from(json_int(obj.get("TargetVehicle")).unwrap_or(0));
            if target_vid == 0
                && let Some(tv) = obj.get("TargetVehicle").and_then(Value::as_object)
            {
                target_vid = i64::from(
                    json_int(tv.get("VehicleId"))
                        .or_else(|| json_int(tv.get("EntityId")))
                        .unwrap_or(0),
                );
            }
            let want_seat = json_int(obj.get("Seat")).unwrap_or(999);
            let cur_vid = ctx.progress_comp.map_or(0, |pc| pc.info.get_key_int("vehicle:cur"));
            let mut is_used = if target_vid > 0 {
                cur_vid == target_vid
            } else {
                cur_vid > 0
            };
            if is_used
                && (0..999).contains(&want_seat)
                && let Some(pc) = ctx.progress_comp
            {
                let cur_seat = pc.info.get_key_int("vehicle:seat");
                if cur_seat >= 0 && cur_seat != i64::from(want_seat) {
                    is_used = false;
                }
            }
            if !is_used && let Some(sc) = ctx.scene {
                for ent in &sc.entities {
                    if let Some(veh) = &ent.vehicle
                        && veh.occupied
                        && (target_vid <= 0 || i64::from(ent.config.config_id) == target_vid)
                        && (want_seat < 0
                            || want_seat == 999
                            || cur_vid != i64::from(ent.config.config_id))
                    {
                        is_used = true;
                        break;
                    }
                }
            }
            is_used == want_used
        }
        "PlayerInVehicle" => {
            let want_in = obj.get("CheckType").and_then(Value::as_bool).unwrap_or(true);
            let want_vtype = get_param_str(obj, "VehicleType").unwrap_or("");
            let cur_vid = ctx.progress_comp.map_or(0, |pc| pc.info.get_key_int("vehicle:cur"));
            let mut in_veh = cur_vid > 0 && matches_vehicle_type(ctx, cur_vid, want_vtype);
            if !in_veh && let Some(sc) = ctx.scene {
                for ent in &sc.entities {
                    if let Some(veh) = &ent.vehicle
                        && veh.occupied
                        && matches_vehicle_type(ctx, i64::from(ent.config.config_id), want_vtype)
                    {
                        in_veh = true;
                        break;
                    }
                }
            }
            if !in_veh
                && !want_vtype.is_empty()
                && let Some(pc) = ctx.progress_comp
                && ((want_vtype == "FishingBoat" && pc.info.get_key_int("fish:boat_active") != 0)
                    || (want_vtype == "Motorcycle" && pc.info.get_key_int("motor:riding") != 0))
            {
                in_veh = true;
            }
            in_veh == want_in
        }
        "CheckDataLayer" => {
            let Some(dl_id) = json_int(obj.get("DataLayerId")) else {
                return false;
            };
            let want_load = obj.get("IsLoad").and_then(Value::as_bool).unwrap_or(true);
            let is_loaded = ctx.quest_comp.info.world.loaded_datalayers.contains(&dl_id)
                && !ctx.quest_comp.info.world.unloaded_datalayers.contains(&dl_id);
            is_loaded == want_load
        }
        "CompareLift" => {
            let eid = json_int(obj.get("EntityId")).unwrap_or_else(|| {
                if obj.get("IsSelf").and_then(Value::as_bool) == Some(true) {
                    ctx.self_entity_config_id
                } else {
                    0
                }
            });
            if eid <= 0 {
                return false;
            }
            let Some(want_loc) = json_int(obj.get("Location")) else {
                return false;
            };
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            let cur_loc = resolve_compare_lift_location(ctx, eid);
            compare_ints(cur_loc, want_loc, cmp)
        }
        "CheckTreasureBeenClaimed" => json_int(obj.get("EntityId"))
            .is_some_and(|eid| ctx.quest_comp.info.has_opened_chest(eid)),
        "CheckSystemFunction" => {
            let Some(sid) = json_int(obj.get("SystemId")) else {
                return false;
            };
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            let is_ne = cmp == "Ne" || cmp == "NotEq";
            let mut open = ctx.quest_comp.info.has_unlocked_system(sid);
            if !open
                && let Some(a) = ctx.assets
                && let Some(fc) = a
                    .tables
                    .function_condition
                    .items
                    .iter()
                    .find(|fc| fc.FunctionId == sid)
            {
                let action_gated = a.indexes.is_action_opened_function(fc.FunctionId);
                open = fc.FunctionId == 10053
                    || (fc.OpenConditionId == 0 && !action_gated)
                    || (fc.OpenConditionId != 0 && eval_condition_group(ctx, fc.OpenConditionId));
            }
            if is_ne { !open } else { open }
        }
        "CompareWeather" | "Weather" => {
            let Some(wid) = json_int(obj.get("WeatherId")) else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            compare_ints(ctx.quest_comp.info.env.weather_id, wid, cmp)
        }
        "CheckLevelPlayState" => {
            let Some(lpid) = json_int(obj.get("LevelId")).or_else(|| json_int(obj.get("LevelPlayId"))) else {
                return false;
            };
            let want_st = json_int(obj.get("State")).unwrap_or(2);
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            let cur_st = if ctx.quest_comp.info.completed_level_plays.contains(&lpid) {
                3
            } else if let Some(pc) = ctx.progress_comp {
                let v = pc.info.get_key_int(&format!("lp_state:{lpid}"));
                if v >= 0 { v.clamp(0, 3) as i32 } else { 1 }
            } else {
                1
            };
            compare_ints(cur_st, want_st, cmp)
        }
        "CheckLevelPlayCompleteNumber" => {
            let Some(level_id) = json_int(obj.get("LevelId")) else {
                return false;
            };
            let Some(required) = json_int(obj.get("Number")) else {
                return false;
            };
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Eq");
            if required > 1 {
                return false;
            }
            let count = i32::from(ctx.quest_comp.info.completed_level_plays.contains(&level_id));
            compare_ints(count, required, cmp)
        }
        "CheckEntitesExist" => {
            let want_exist = obj.get("IsExist").and_then(Value::as_bool).unwrap_or(true);
            eval_check_entities_exist(ctx, obj) == want_exist
        }
        "CheckTargetSunSpiritCollected" => {
            let Some(eid) = json_int(obj.get("SunSpiritEntityId")) else {
                return false;
            };
            let want_col = obj.get("IsCollected").and_then(Value::as_bool).unwrap_or(true);
            is_sun_spirit_collected(ctx, i64::from(eid)) == want_col
        }
        "CheckCollectedSunSpiritCount" | "CheckAvailableSunSpiritCount" => {
            let dungeon_id = json_int(obj.get("DungeonId")).unwrap_or(0);
            let area_id = json_int(obj.get("AreaId")).unwrap_or(0);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let req_count = json_int(obj.get("Count")).unwrap_or(0);
            let want_collected = ct == "CheckCollectedSunSpiritCount";
            let count = count_sun_spirits_filtered(ctx, dungeon_id, area_id, want_collected);
            compare_ints(count, req_count, cmp)
        }
        "CheckIsSupportedWalking" => {
            let Some(want_supported) = obj.get("IsSupported").and_then(Value::as_bool) else {
                return false;
            };
            let is_supported = ctx
                .progress_comp
                .is_some_and(|pc| pc.info.get_key_int("npc:supported_walking") > 0);
            is_supported == want_supported
        }
        "CompareGameDifficulty" => {
            let cmp = get_param_str(obj, "CompareType").unwrap_or("Eq");
            let Some(req_diff) = json_int(obj.get("Difficulty")) else {
                return false;
            };
            let cur_diff = ctx
                .progress_comp
                .map(|pc| pc.info.get_key_int("game:difficulty"))
                .filter(|&v| v > 0)
                .map_or(5, |v| v as i32);
            compare_ints(cur_diff, req_diff, cmp)
        }
        "CheckPlayerSkillReady" => eval_player_skill_ready(ctx, obj),
        "CheckEntityHasBuff" | "HasBuff" => {
            let Some(buff_id) = json_int(obj.get("BuffId")) else {
                return false;
            };
            let want_has_buff = match obj.get("HasBuff") {
                Some(Value::Bool(b)) => *b,
                Some(_) => return false,
                None => true,
            };
            let cmp_opt = get_param_str(obj, "Compare");
            let cmp = cmp_opt.unwrap_or("Eq");
            let Some(scene) = ctx.scene else {
                return false;
            };
            let entity_id = if ct == "CheckEntityHasBuff" {
                let Some(id) = resolve_target_net_id(scene, obj.get("Target")) else {
                    return false;
                };
                id
            } else {
                scene.current_role_entity_id()
            };
            let Some(&entity_index) = scene.net_id_map.get(&entity_id) else {
                return false;
            };
            let has_buff = scene
                .entities
                .get(entity_index)
                .and_then(|e| e.buffs.as_ref())
                .is_some_and(|buffs| buffs.get_by_buff_id(i64::from(buff_id)).is_some());
            if ct != "CheckEntityHasBuff" || cmp_opt.is_some() {
                compare_ints(i32::from(has_buff), i32::from(want_has_buff), cmp)
            } else {
                has_buff == want_has_buff
            }
        }
        "CheckGameplayTag" => {
            let Some(tag_name) = get_param_str(obj, "GameplayTag") else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            if cmp != "Eq" && cmp != "Ne" {
                return false;
            }
            let Some(scene) = ctx.scene else {
                return false;
            };
            let entity_id = if obj.contains_key("Target") {
                let Some(id) = resolve_target_net_id(scene, obj.get("Target")) else {
                    return false;
                };
                id
            } else {
                scene.current_role_entity_id()
            };
            let Some(&entity_index) = scene.net_id_map.get(&entity_id) else {
                return false;
            };
            let Some(assets) = ctx.assets else {
                return false;
            };
            let Some(tag_id) =
                gameplay_tags::id_from_name(&assets.tables.gameplay_tag_remap, tag_name)
            else {
                return false;
            };
            let has_tag = scene
                .entities
                .get(entity_index)
                .and_then(|e| e.tag.as_ref())
                .is_some_and(|tc| tc.has_tag(&assets.tables.gameplay_tag_parent, tag_id));
            if cmp == "Eq" { has_tag } else { !has_tag }
        }
        "CheckCertainFishingItemCount" => {
            let Some(item_id) = json_int(obj.get("Id")) else {
                return false;
            };
            let Some(count) = json_int(obj.get("Count")) else {
                return false;
            };
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Ge");
            compare_ints(item_count(ctx, item_id), count, cmp)
        }
        "CompareFishingTechLevel" => {
            let Some(tech_id) = json_int(obj.get("TechId")) else {
                return false;
            };
            let Some(required_level) = json_int(obj.get("TechLevel")) else {
                return false;
            };
            let cmp = obj.get("Compare").and_then(Value::as_str).unwrap_or("Ge");
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let current_level =
                ProgressInfo::get_id_count(&progress.info.fishing.tech_levels, tech_id);
            compare_ints(current_level, required_level, cmp)
        }
        "CompleteCertainFishingEntrust" => {
            let Some(entrust_id) = json_int(obj.get("Id")) else {
                return false;
            };
            let Some(required_count) = json_int(obj.get("Count")) else {
                return false;
            };
            if required_count <= 0 {
                return true;
            }
            if required_count > 1 {
                return false;
            }
            ctx.progress_comp
                .is_some_and(|pc| pc.info.has_claim(&format!("fish:entrust_done:{entrust_id}")))
        }
        "CompareFishingBoatState" => {
            let want_in_port = match obj.get("IsStop") {
                Some(Value::Bool(b)) => *b,
                Some(_) => return false,
                None => true,
            };
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let is_in_port = progress.info.get_key_int("fish:sailing") == 0;
            want_in_port == is_in_port
        }
        "DungeonSubType" => {
            let Some(subtype) = json_int(obj.get("DungeonSubType")) else {
                return false;
            };
            let Some(scene) = ctx.scene else {
                return false;
            };
            let Some(assets) = ctx.assets else {
                return false;
            };
            assets
                .tables
                .instance_dungeon
                .get_data_by_id(&scene.instance_id)
                .is_some_and(|instance| instance.InstSubType == subtype)
        }
        "CheckHookLockPoint" => {
            let Some(ids) = obj.get("EntityIds").and_then(Value::as_array) else {
                return false;
            };
            if ids.is_empty() {
                return false;
            }
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            ids.iter()
                .filter_map(|v| json_int(Some(v)))
                .any(|eid| progress.info.get_key_int(&format!("hook_point:active:{eid}")) != 0)
        }
        "CheckIsCharacterHoldingHands" => {
            let Some(target) = obj.get("Target").and_then(Value::as_object) else {
                return false;
            };
            if target.get("Type").and_then(Value::as_str) != Some("Player") {
                return false;
            }
            let Some(want_holding) = obj.get("IsHoldingHands").and_then(Value::as_bool) else {
                return false;
            };
            let Some(scene) = ctx.scene else {
                return false;
            };
            let role_id = scene.current_role_entity_id();
            if role_id <= 0 {
                return false;
            }
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            want_holding == (progress.info.get_key_int(&format!("hold_hand:{role_id}")) != 0)
        }
        "CheckHonamiStoryActivateTalent" => {
            let Some(talent_id) = json_int(obj.get("TalentId")) else {
                return false;
            };
            let Some(want_active) = obj.get("IsActivated").and_then(Value::as_bool) else {
                return false;
            };
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            want_active == progress.info.has_claim(&format!("honami:talent:{talent_id}"))
        }
        "CheckHonamiStoryLifeSupportLevel" => {
            let Some(required) = json_int(obj.get("Level")) else {
                return false;
            };
            let Some(cmp) = obj.get("Compare").and_then(Value::as_str) else {
                return false;
            };
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let current = progress.info.get_key_int("honami:ls_lv").max(1) as i32;
            compare_ints(current, required, cmp)
        }
        "CheckHonamiStoryUnlockSlot" => {
            let Some(role_slot) = json_int(obj.get("RoleSlot")) else {
                return false;
            };
            let Some(plugin_slot) = json_int(obj.get("PluginSlot")) else {
                return false;
            };
            let Some(want_unlocked) = obj.get("IsUnlocked").and_then(Value::as_bool) else {
                return false;
            };
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            want_unlocked == progress.info.has_claim(&format!("honami:slot:{role_slot}:{plugin_slot}"))
        }
        "CheckTargetAttribute" => {
            let Some(option) = obj.get("Option").and_then(Value::as_object) else {
                return false;
            };
            let Some(target_type) = get_param_str(option, "Type") else {
                return false;
            };
            let Some(attributes) = option.get("AttributeTypes").and_then(Value::as_array) else {
                return false;
            };
            if attributes.is_empty() {
                return false;
            }
            let Some(scene) = ctx.scene else {
                return false;
            };

            if target_type == "Player" {
                let target_option = get_param_str(option, "Option").unwrap_or("AnyRole");
                if target_option == "CurRole" || target_option == "Team" {
                    let role_eid = scene.current_role_entity_id();
                    let Some(&entity_idx) = scene.net_id_map.get(&role_eid) else {
                        return false;
                    };
                    return eval_entity_attribute_condition_list(ctx, scene, entity_idx, attributes);
                }
                let Ok(formation_idx) = usize::try_from(scene.formation_info.cur_formation) else {
                    return false;
                };
                let Some(formation) = scene.formation_info.formations.get(formation_idx) else {
                    return false;
                };
                if target_option == "SpecifyRole" {
                    let Ok(slot_idx) = usize::try_from(json_int(option.get("Index")).unwrap_or(0)) else {
                        return false;
                    };
                    let Some(Some(role)) = formation.roles.get(slot_idx) else {
                        return false;
                    };
                    if role.entity_id <= 0 {
                        return false;
                    }
                    let Some(&entity_idx) = scene.net_id_map.get(&role.entity_id) else {
                        return false;
                    };
                    return eval_entity_attribute_condition_list(ctx, scene, entity_idx, attributes);
                }
                if target_option != "AnyRole" {
                    return false;
                }
                return formation.roles.iter().flatten().any(|role| {
                    role.entity_id > 0
                        && scene
                            .net_id_map
                            .get(&role.entity_id)
                            .is_some_and(|&idx| eval_entity_attribute_condition_list(ctx, scene, idx, attributes))
                });
            }
            let Some(net_id) = resolve_target_net_id_val(scene, option) else {
                return false;
            };
            let Some(&entity_idx) = scene.net_id_map.get(&net_id) else {
                return false;
            };
            eval_entity_attribute_condition_list(ctx, scene, entity_idx, attributes)
        }
        "CheckBvbChallengeState" => {
            let Some(challenge_id) = json_int(obj.get("ChallengeId")) else {
                return false;
            };
            let Some(state) = json_int(obj.get("ChallengeState")) else {
                return false;
            };
            let succeeded = ctx
                .quest_comp
                .info
                .progress
                .completed_bvb
                .contains(&challenge_id);
            if state == 2 {
                return succeeded;
            }
            let failed = ctx
                .progress_comp
                .is_some_and(|pc| pc.info.get_key_int(&format!("bvb:state:{challenge_id}")) == 1);
            match state {
                1 => !succeeded && failed,
                0 => !succeeded && !failed,
                _ => false,
            }
        }
        "RangeSphere" => {
            let Some(center) = obj.get("Center").and_then(Value::as_object) else {
                return false;
            };
            let Some(radius) = json_float_val(obj.get("Radius")) else {
                return false;
            };
            if radius <= 0.0 {
                return false;
            }
            let Some(cx) = json_float_val(center.get("X")) else {
                return false;
            };
            let Some(cy) = json_float_val(center.get("Y")) else {
                return false;
            };
            let cz = json_float_val(center.get("Z")).unwrap_or(0.0);
            let Some(p) = current_player_pos_3d(ctx) else {
                return false;
            };
            let dx = p[0] - cx;
            let dy = p[1] - cy;
            let dz = p[2] - cz;
            dx * dx + dy * dy + dz * dz <= radius * radius
        }
        "CheckDirection" => {
            let Some(scene) = ctx.scene else {
                return false;
            };
            let Some(dir_val) = obj.get("Direction").and_then(Value::as_object) else {
                return false;
            };
            let target_yaw = if let Some(zv) = dir_val.get("Z") {
                let Some(v) = json_float_val(Some(zv)) else {
                    return false;
                };
                v
            } else {
                0.0
            };
            let interval = json_float_val(obj.get("AngleInterval")).unwrap_or(60.0);
            if interval <= 0.0 {
                return false;
            }
            if interval >= 180.0 {
                return true;
            }
            let Some(first_player) = scene.instance.players.first() else {
                return false;
            };
            let mut cur_yaw = first_player.rotation[2];
            if let Some(target_val) = obj.get("Target") {
                let Some(net_id) = resolve_target_net_id(scene, Some(target_val)) else {
                    return false;
                };
                let Some(&idx) = scene.net_id_map.get(&net_id) else {
                    return false;
                };
                if let Some(pos) = scene.entities.get(idx).and_then(|e| e.position) {
                    cur_yaw = pos.rotation[2];
                }
            }
            let diff = ((cur_yaw - target_yaw + 180.0).rem_euclid(360.0) - 180.0).abs();
            diff <= interval
        }
        "CompareCalabashLevel" => {
            let Some(a) = ctx.assets else {
                return false;
            };
            let req = json_int(obj.get("CalabashLevel"))
                .or_else(|| json_int(obj.get("Level")))
                .unwrap_or(1);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Ge");
            compare_ints(current_calabash_level(a, ctx.basic_comp, ctx.echo_comp), req, cmp)
        }
        "CheckLordGymFinish" => {
            let Some(gym_id) = json_int(obj.get("LordGymId")) else {
                return false;
            };
            ctx.progress_comp
                .is_some_and(|pc| pc.info.has_passed_lord_gym(gym_id))
        }
        "CheckCalabashDevelopReward" => {
            let Some(monster_id) = json_int(obj.get("MonsterId")) else {
                return false;
            };
            let need_dev = json_int(obj.get("Develop")).unwrap_or(1);
            if ctx.echo_comp.calabash_info.highest_quality_for_monster(monster_id) >= need_dev {
                return true;
            }
            ctx.progress_comp
                .is_some_and(|pc| pc.info.get_key_int(&format!("calabash:dev:{monster_id}")) >= i64::from(need_dev))
        }
        "CheckSubLevelState" => {
            let Some(check_list) = obj.get("CheckList").and_then(Value::as_array) else {
                return false;
            };
            if check_list.is_empty() {
                return true;
            }
            let req_count = json_int(obj.get("ConditionCount")).unwrap_or(0);
            let mut matched = 0i32;
            for entry in check_list {
                let Some(entry_obj) = entry.as_object() else {
                    continue;
                };
                let Some(sname) = get_param_str(entry_obj, "SubLevelName") else {
                    continue;
                };
                let want_state = get_param_str(entry_obj, "SubLevelState").unwrap_or("Enable");
                let is_loaded = ctx
                    .quest_comp
                    .info
                    .world
                    .loaded_sublevels
                    .iter()
                    .any(|lv| lv == sname);
                if is_loaded == (want_state == "Enable") {
                    matched += 1;
                }
            }
            if req_count <= 0 {
                matched == (check_list.len() as i32)
            } else {
                matched >= req_count
            }
        }
        "CheckPlayerIsInMotorcycleCruise" => {
            let want = obj
                .get("IsInMotorcycleCruise")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let Some(pc) = ctx.progress_comp else {
                return !want;
            };
            (pc.info.get_key_int("motor:cruise") > 0) == want
        }
        "CheckSyncLevelPlayCompleteNumber" => {
            let Some(sync_id) = json_int(obj.get("SyncId")) else {
                return false;
            };
            let num = json_int(obj.get("Number")).unwrap_or(0);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let cur = ctx.progress_comp.map_or(0, |pc| {
                pc.info
                    .get_key_int(&format!("sync_lp:{sync_id}"))
                    .clamp(0, i64::from(i32::MAX)) as i32
            });
            compare_ints(cur, num, cmp)
        }
        "CheckCrossSceneSyncKillMonsterFinished" => {
            let Some(sync_id) = json_int(obj.get("SyncId")) else {
                return false;
            };
            let bundle_id = json_int(obj.get("EntityBundleId")).unwrap_or(0);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let key = format!("cross_kill:{sync_id}:{bundle_id}");
            let cur = i32::from(
                ctx.progress_comp
                    .is_some_and(|pc| pc.info.has_claim(&key) || pc.info.get_key_int(&key) > 0),
            );
            compare_ints(cur, 1, cmp)
        }
        "CheckGiantRegionCultivationTriggeredUpgradeLevel" => {
            let eco_id = match get_param_str(obj, "EcoType") {
                Some("Fire") => 0,
                Some("Water") => 1,
                Some("Wood") => 2,
                _ => return false,
            };
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let cur = pc
                .info
                .get_key_int(&format!("giant:upgrade:{eco_id}"))
                .clamp(0, i64::from(i32::MAX)) as i32;
            let Some(levels) = obj.get("Levels").and_then(Value::as_array) else {
                return cur > 0;
            };
            levels
                .iter()
                .any(|lv| json_int(Some(lv)).unwrap_or(-1) == cur)
        }
        "CheckTetrisEndlessScore" => {
            let did = json_int(obj.get("DungeonId")).unwrap_or(0);
            let target = json_int(obj.get("TargetScore")).unwrap_or(0);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Ge");
            let score = ctx.progress_comp.map_or(0, |pc| {
                pc.info
                    .get_key_int(&format!("tetris:score:{did}"))
                    .clamp(0, i64::from(i32::MAX)) as i32
            });
            compare_ints(score, target, cmp)
        }
        "CheckDollGrabShowcaseItemComboAchieved" => {
            let eid = json_int(obj.get("ShowcaseEntityId")).unwrap_or(0);
            let count = json_int(obj.get("Count")).unwrap_or(1);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Ge");
            let cur = ctx.progress_comp.map_or(0, |pc| {
                pc.info
                    .get_key_int(&format!("doll:combo:{eid}"))
                    .clamp(0, i64::from(i32::MAX)) as i32
            });
            compare_ints(cur, count, cmp)
        }
        "CompareLevelPlayRewardState" => {
            let cfg_id = json_int(obj.get("EntityId")).unwrap_or(ctx.self_entity_config_id);
            let want_state = json_int(obj.get("State")).unwrap_or(3);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let mut cur_state = 0;
            if cfg_id > 0 && ctx.quest_comp.info.has_opened_chest(cfg_id) {
                cur_state = 3;
            } else if cfg_id > 0
                && let Some(assets) = ctx.assets
            {
                let map_id = ctx.scene.map_or(8, |sc| sc.instance_id);
                let scene_map = assets.resolve_map_config_id(map_id);
                for &lp_idx in assets.indexes.level_play_indices_for_reward_entity(cfg_id) {
                    let lp = &assets.tables.level_play_data.items[lp_idx as usize];
                    let lp_map = assets.resolve_map_config_id(if lp.Data.InstanceId != 0 {
                        lp.Data.InstanceId
                    } else {
                        lp.Data.LevelId
                    });
                    if lp.Data.InstanceId != map_id
                        && lp.Data.LevelId != map_id
                        && lp_map != scene_map
                    {
                        continue;
                    }
                    cur_state = if ctx.quest_comp.info.completed_level_plays.contains(&lp.LevelPlayId) {
                        3
                    } else if ctx
                        .quest_comp
                        .info
                        .progress
                        .entered_level_plays
                        .contains(&lp.LevelPlayId)
                    {
                        2
                    } else {
                        1
                    };
                    break;
                }
            }
            compare_ints(cur_state, want_state, cmp)
        }
        "TargetEntityInvisible" => {
            let Some(eid) = json_int(obj.get("EntityId")) else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let invisible = is_entity_target_invisible(ctx, eid);
            let want_inv = obj.get("Invisible").and_then(Value::as_bool).unwrap_or(true);
            let matches = invisible == want_inv;
            if is_neg_compare(cmp) { !matches } else { matches }
        }
        "CompareEntityGroupState" => {
            let Some(gc) = obj.get("GroupCondition").and_then(Value::as_object) else {
                return false;
            };
            let Some(conds) = gc.get("Conditions").and_then(Value::as_array) else {
                return false;
            };
            let mut matched = 0i32;
            for entry in conds {
                let Some(eobj) = entry.as_object() else {
                    continue;
                };
                let Some(eid) = json_int(eobj.get("EntityId")) else {
                    continue;
                };
                let Some(want_state) = get_param_str(eobj, "State") else {
                    continue;
                };
                let cur = ctx
                    .quest_comp
                    .info
                    .get_entity_state(i64::from(eid))
                    .unwrap_or("关卡.Common.状态.常态");
                if cur == want_state {
                    matched += 1;
                }
            }
            let need = json_int(gc.get("Count")).unwrap_or(conds.len() as i32);
            let cmp = get_param_str(gc, "Compare").unwrap_or("Ge");
            compare_ints(matched, need, cmp)
        }
        "CheckEntityPosition" => {
            let eid = json_int(obj.get("EntityId")).unwrap_or(ctx.self_entity_config_id);
            let Some(ent_pos) = resolve_entity_world_pos(ctx, i64::from(eid)) else {
                return false;
            };
            let Some(range_val) = obj.get("Range").and_then(Value::as_object) else {
                return false;
            };
            let Some(center) = parse_json_vector_pos(range_val.get("Pos")) else {
                return false;
            };
            let radius = json_float_val(range_val.get("Radius")).unwrap_or(0.0);
            let dx = ent_pos[0] - center[0];
            let dy = ent_pos[1] - center[1];
            let dz = ent_pos[2] - center[2];
            let in_range = (dx * dx + dy * dy + dz * dz) <= (radius * radius);
            let want_on = obj.get("IsOnRange").and_then(Value::as_bool).unwrap_or(true);
            in_range == want_on
        }
        "CheckPlayerRelativeDistance" => {
            let eid = json_int(obj.get("TargetEntityId"))
                .or_else(|| json_int(obj.get("EntityId")))
                .unwrap_or(ctx.self_entity_config_id);
            let Some(ent_pos) = resolve_entity_world_pos(ctx, i64::from(eid)) else {
                return false;
            };
            let Some(player_pos) = current_player_pos_3d(ctx) else {
                return false;
            };
            let dx = player_pos[0] - ent_pos[0];
            let dy = player_pos[1] - ent_pos[1];
            let dz = player_pos[2] - ent_pos[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let Some(ranges) = obj.get("ValidDistanceRanges").and_then(Value::as_array) else {
                return false;
            };
            if ranges.is_empty() {
                return false;
            }
            ranges.iter().any(|r| {
                r.as_object().is_some_and(|robj| {
                    let min_d = json_float_val(robj.get("Min")).unwrap_or(0.0);
                    let max_d = json_float_val(robj.get("Max")).unwrap_or(f32::MAX);
                    dist >= min_d && dist <= max_d
                })
            })
        }
        "CheckHonamiStoryPlayerCorruptedLevel" => {
            let cur = ctx.progress_comp.map_or(0, |pc| {
                pc.info
                    .get_key_int("honami:corrupted_lv")
                    .clamp(0, i64::from(i32::MAX)) as i32
            });
            let target = json_int(obj.get("Level")).unwrap_or(0);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            compare_ints(cur, target, cmp)
        }
        "CheckHonamiStorySuccessfulEvacuation" => {
            let want = obj.get("IsSuccessful").and_then(Value::as_bool).unwrap_or(true);
            let cur = ctx
                .progress_comp
                .is_some_and(|pc| pc.info.get_key_int("honami:safe_leave") > 0);
            cur == want
        }
        "CheckHonamiStoryGetItem" | "CheckHonamiStoryEquipItem" => {
            let Some(iid) = json_int(obj.get("ItemId")) else {
                return false;
            };
            let need = json_int(obj.get("Count")).unwrap_or(1);
            ctx.progress_comp
                .is_some_and(|pc| pc.info.get_key_int(&format!("honami:item:{iid}")) >= i64::from(need))
        }
        "CheckHonamiStoryChestOpenTime" => {
            let cur = ctx.progress_comp.map_or(0, |pc| {
                pc.info
                    .get_key_int("honami:corrupted_chest_cnt")
                    .clamp(0, i64::from(i32::MAX)) as i32
            });
            let target = json_int(obj.get("Time")).unwrap_or(1);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Ge");
            compare_ints(cur, target, cmp)
        }
        "CheckBvbActivityRank" => {
            let cur = ctx
                .progress_comp
                .map_or(1, |pc| pc.info.get_key_int("bvb:rank").max(1) as i32);
            let target = json_int(obj.get("Rank")).unwrap_or(1);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Ge");
            compare_ints(cur, target, cmp)
        }
        "CheckLevelPlayChildQuestFinished" => {
            let Some(nid) = json_int(obj.get("ChildQuestId")) else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let done = is_level_play_node_marked_finished(ctx, nid);
            compare_ints(i32::from(done), 1, cmp)
        }
        "CheckEntityHasSceneItemAttributeTag" => {
            let raw_eid = json_int(obj.get("EntityId")).unwrap_or(0);
            let eid = if raw_eid > 0 {
                raw_eid
            } else {
                ctx.self_entity_config_id
            };
            if eid <= 0 {
                return false;
            }
            let check_ty = json_int(obj.get("CheckType")).unwrap_or(0);
            let Some(tags_val) = obj.get("Tags").and_then(Value::as_array) else {
                return false;
            };
            if tags_val.is_empty() {
                return check_ty != 0;
            }
            let mut has_any = false;
            let mut has_all = true;
            for tv in tags_val {
                let Some(tid) = json_int(Some(tv)) else {
                    continue;
                };
                if entity_has_runtime_tag(ctx, eid, tid) {
                    has_any = true;
                } else {
                    has_all = false;
                }
            }
            match check_ty {
                0 => has_all,
                1 => !has_any,
                _ => has_any,
            }
        }
        "CheckPlayerStateRestriction" => {
            let Some(rid) = json_int(obj.get("RestrictionId")) else {
                return true;
            };
            eval_player_state_restriction(ctx, rid)
        }
        "ListenEntitySelfEvent" => {
            let Some(ev_key) = get_param_str(obj, "EventKey") else {
                return false;
            };
            let raw_eid = json_int(obj.get("EntityId")).unwrap_or(0);
            let eid = if raw_eid > 0 {
                raw_eid
            } else {
                ctx.self_entity_config_id
            };
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let key = format!("ent_self_ev:{eid}:{ev_key}");
            pc.info.get_key_int(&key) > 0 || pc.info.has_claim(&key)
        }
        "CheckAiState" => {
            let Some(st_type) = get_param_str(obj, "StateType") else {
                return false;
            };
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let raw_eid = json_int(obj.get("EntityId")).unwrap_or(0);
            let eid = if raw_eid > 0 {
                raw_eid
            } else {
                ctx.self_entity_config_id
            };
            let actual = ctx
                .progress_comp
                .map_or("", |pc| pc.info.get_key_str(&format!("ai_state:{eid}")));
            let eq = !actual.is_empty() && actual == st_type;
            if is_neg_compare(cmp) { !eq } else { eq }
        }
        "CheckDollGrabMachineFinished" => {
            let mut eid = ctx.self_entity_config_id;
            if let Some(tv) = obj.get("Target").and_then(Value::as_object)
                && let Some(tid) = json_int(tv.get("EntityId"))
                && tid > 0
            {
                eid = tid;
            }
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let key = format!("doll:fin:{eid}");
            pc.info.has_claim(&key) || pc.info.get_key_int(&key) > 0
        }
        "CheckFishingPointHasFish" => {
            let want = obj.get("HasFish").and_then(Value::as_bool).unwrap_or(true);
            let is_empty = ctx.self_entity_config_id > 0
                && ctx.progress_comp.is_some_and(|pc| {
                    pc.info
                        .get_key_int(&format!("fish:empty:{}", ctx.self_entity_config_id))
                        > 0
                });
            (!is_empty) == want
        }
        "CheckFishingCageFillingRatio" => {
            let ratio = json_int(obj.get("Ratio")).unwrap_or(0);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Le");
            let cur = ctx
                .progress_comp
                .map_or(0, |pc| pc.info.get_key_int("fish:cage_ratio").clamp(0, 100) as i32);
            compare_ints(cur, ratio, cmp)
        }
        "CheckAlertAreaEnabled" => {
            let Some(area_id) = json_int(obj.get("AreaId")) else {
                return false;
            };
            ctx.progress_comp
                .is_some_and(|pc| pc.info.get_key_int(&format!("alert_enable:{area_id}")) != 0)
        }
        "CheckFollowShooterActive" => {
            let want = obj.get("Active").and_then(Value::as_bool).unwrap_or(true);
            let id = json_int(obj.get("Id")).unwrap_or(0);
            let Some(pc) = ctx.progress_comp else {
                return !want;
            };
            let cur = id > 0 && pc.info.get_key_int("trial:follow_shooter") == i64::from(id);
            cur == want
        }
        "CheckDungeonHasSaveConfig" => {
            let Some(did) = json_int(obj.get("DungeonId")) else {
                return false;
            };
            let want = obj
                .get("IsHasSaveConfig")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let has_save = ctx.progress_comp.is_some_and(|pc| {
                let k = format!("inst:save_cfg:{did}");
                pc.info.get_key_int(&k) > 0 || pc.info.has_claim(&k)
            });
            has_save == want
        }
        "CheckVehicleSpeed" => {
            let target_speed = json_int(obj.get("Speed")).unwrap_or(0);
            let cmp = get_param_str(obj, "CompareType")
                .or_else(|| get_param_str(obj, "Compare"))
                .unwrap_or("Gt");
            let cur_speed = ctx.progress_comp.map_or(0, |pc| {
                pc.info
                    .get_key_int("vehicle:speed")
                    .clamp(0, i64::from(i32::MAX)) as i32
            });
            compare_ints(cur_speed, target_speed, cmp)
        }
        "CheckIsGlobalTimeScaleEnabled" => {
            let want = obj.get("Enabled").and_then(Value::as_bool).unwrap_or(true);
            let cur = ctx
                .progress_comp
                .is_some_and(|pc| pc.info.get_key_int("time_scale:global") > 0);
            cur == want
        }
        "CheckIsEntityTimeScaleEnabled" => {
            let want = obj.get("Enabled").and_then(Value::as_bool).unwrap_or(true);
            let mut eid = ctx.self_entity_config_id;
            if let Some(tv) = obj.get("Target").and_then(Value::as_object)
                && let Some(tid) = json_int(tv.get("EntityId"))
                && tid > 0
            {
                eid = tid;
            }
            let cur = ctx
                .progress_comp
                .is_some_and(|pc| pc.info.get_key_int(&format!("time_scale:ent:{eid}")) > 0);
            cur == want
        }
        "CheckElementalEffectActive" => {
            let want = obj.get("IsActive").and_then(Value::as_bool).unwrap_or(true);
            let mut active = false;
            if ctx.self_entity_config_id > 0 {
                if ctx.progress_comp.is_some_and(|pc| {
                    pc.info
                        .get_key_int(&format!("elem_ability:{}", ctx.self_entity_config_id))
                        > 0
                }) {
                    active = true;
                }
                if !active {
                    let st = resolve_current_entity_state_cow(ctx, ctx.self_entity_config_id);
                    if st.contains("激活") {
                        active = true;
                    }
                }
            }
            active == want
        }
        "CheckClientEvent" => {
            let Some(ev_name) = get_param_str(obj, "EventName") else {
                return false;
            };
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            let key = format!("client_ev:{ev_name}");
            pc.info.has_claim(&key) || pc.info.get_key_int(&key) > 0
        }
        "CheckCollectAnimalParts" => {
            let check_ty = json_int(obj.get("CheckType")).unwrap_or(0);
            let Some(slots) = obj.get("Slots").and_then(Value::as_array) else {
                return false;
            };
            let Some(pc) = ctx.progress_comp else {
                return false;
            };
            for sv in slots {
                let Some(slot_id) = json_int(Some(sv)) else {
                    continue;
                };
                let key = format!("animal_part:{}:{slot_id}", ctx.self_entity_config_id);
                let collected = pc.info.has_claim(&key) || pc.info.get_key_int(&key) > 0;
                if check_ty == 0 && collected {
                    return true;
                }
                if check_ty != 0 && !collected {
                    return false;
                }
            }
            check_ty != 0
        }
        "CheckPlayerIsInDeadEyeProcess" => {
            let want = obj
                .get("IsInDeadEyeProcess")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let active = ctx
                .progress_comp
                .is_some_and(|pc| pc.info.get_key_int("deadeye:active") > 0);
            active == want
        }
        "CheckSceneItemDirection" => eval_check_scene_item_direction(ctx, obj),
        "CheckPlayerRelativeAngle" | "CheckPlayerEnterClientTriggerDir" => {
            eval_check_player_relative_angle(ctx, obj)
        }
        "CheckPlayerPosition" => eval_check_player_position(ctx, obj),
        "SaveConfigEnterMethod" => {
            let expected = get_param_str(obj, "EnterMethod").unwrap_or("FirstTime");
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let current = if ctx
                .scene
                .is_some_and(|s| s.last_server_rollback_quest_id == ctx.owner_quest_id && ctx.owner_quest_id > 0)
            {
                "FailureRollBack"
            } else {
                "FirstTime"
            };
            let matches = current == expected;
            if cmp == "Ne" { !matches } else { matches }
        }
        "CheckTrafficBlocked" => {
            let is_blocked = obj.get("IsBlocked").and_then(Value::as_bool).unwrap_or(true);
            let Some(player_pos) = current_player_pos_3d(ctx) else {
                return !is_blocked;
            };
            let Some(ent_pos) = resolve_entity_world_pos(ctx, i64::from(ctx.self_entity_config_id)) else {
                return !is_blocked;
            };
            let dx = player_pos[0] - ent_pos[0];
            let dy = player_pos[1] - ent_pos[1];
            let dz = player_pos[2] - ent_pos[2];
            let in_zone = (dx * dx + dy * dy + dz * dz) <= 300.0 * 300.0;
            in_zone == is_blocked
        }
        _ => eval_extra_inline_condition(ctx, ct, obj).unwrap_or(false),
    }
}

fn eval_extra_inline_condition(
    ctx: &EvalContext<'_>,
    ct: &str,
    obj: &Map<String, Value>,
) -> Option<bool> {
    let pc = ctx.progress_comp;
    let res = match ct {
        "CheckFinishLoading" => ctx.scene.is_some(),
        "CheckElementalBoxState" => {
            let eid = obj
                .get("Target")
                .and_then(Value::as_object)
                .and_then(|t| json_int(t.get("EntityId")))
                .or_else(|| json_int(obj.get("EntityId")))?;
            let want = obj
                .get("Target")
                .and_then(Value::as_object)
                .and_then(|t| get_param_str(t, "State"))
                .or_else(|| get_param_str(obj, "State"))
                .unwrap_or("关卡.Common.状态.完成");
            let cur = ctx
                .quest_comp
                .info
                .get_entity_state(i64::from(eid))
                .unwrap_or("关卡.Common.状态.常态");
            cur == want
        }
        "CheckEntityBuffStack" => {
            let scene = ctx.scene?;
            let buff_id = json_int(obj.get("BuffId"))?;
            let want_stack = json_int(obj.get("Stack")).unwrap_or(1);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Ge");
            let target_nid = resolve_target_net_id(scene, obj.get("Target")).or_else(|| {
                (ctx.self_entity_config_id > 0)
                    .then(|| scene.find_entity_net_id_by_config_id(ctx.self_entity_config_id))
                    .flatten()
            })?;
            let idx = *scene.net_id_map.get(&target_nid)?;
            let cur_stack = scene
                .entities
                .get(idx)
                .and_then(|e| e.buffs.as_ref())
                .map_or(0, |b| b.stack_count_of(i64::from(buff_id)));
            compare_ints(cur_stack, want_stack, cmp)
        }
        "CheckEntitySelfDirection" => {
            let scene = ctx.scene?;
            let dir = obj.get("Direction").and_then(Value::as_object)?;
            let want_yaw = dir.get("Z").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let tol = obj
                .get("Tolerance")
                .and_then(Value::as_f64)
                .map_or(25.0, |v| if v > 0.0 { v as f32 } else { 25.0 });
            let nid = if ctx.self_entity_config_id > 0 {
                scene.find_entity_net_id_by_config_id(ctx.self_entity_config_id)?
            } else {
                scene.current_role_entity_id()
            };
            let pos = scene.entity(nid)?.position?;
            let mut diff = (pos.rotation[2] - want_yaw).abs() % 360.0;
            if diff > 180.0 {
                diff = 360.0 - diff;
            }
            diff <= tol
        }
        "CheckIsInDragActorPlay" => {
            let want = obj.get("IsIn").and_then(Value::as_bool).unwrap_or(false);
            let in_play = pc.is_some_and(|p| p.info.get_key_int("drag_actor:in_play") != 0);
            in_play == want
        }
        "CompareTeammateDie" => {
            let want_die = json_int(obj.get("DieCount")).unwrap_or(0);
            let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
            let dead_count = ctx.scene.map_or(0, |scene| {
                let idx = usize::try_from(scene.formation_info.cur_formation).unwrap_or(0);
                scene.formation_info.formations.get(idx).map_or(0, |f| {
                    f.roles
                        .iter()
                        .flatten()
                        .filter(|fr| fr.entity_id > 0)
                        .filter(|fr| {
                            scene.entity(fr.entity_id).is_some_and(|st| {
                                st.attribute.as_ref().is_some_and(|attr_comp| {
                                    let life_idx = sonoro_proto::PbEnum::to_i32(
                                        sonoro_proto::enums::EAttributeType::Life,
                                    ) as usize;
                                    let max_idx = sonoro_proto::PbEnum::to_i32(
                                        sonoro_proto::enums::EAttributeType::LifeMax,
                                    ) as usize;
                                    let life_max =
                                        attr_comp.attributes.get(max_idx).map_or(0, |a| a.current);
                                    let life =
                                        attr_comp.attributes.get(life_idx).map_or(0, |a| a.current);
                                    life_max > 0 && life <= 0
                                })
                            })
                        })
                        .count() as i32
                })
            });
            compare_ints(dead_count, want_die, cmp)
        }
        "CompareCost" => {
            let want_cost = json_int(obj.get("Cost")).unwrap_or(0);
            let cmp = obj
                .get("CompareType")
                .or_else(|| obj.get("Compare"))
                .and_then(Value::as_str)
                .unwrap_or("Eq");
            let cur_cost = pc.map_or(0, |p| p.info.get_key_int("bvb:last_cost") as i32);
            compare_ints(cur_cost, want_cost, cmp)
        }
        "IsCapableToEvolve" => {
            let want_cost = json_int(obj.get("Cost")).unwrap_or(0);
            pc.is_some_and(|p| {
                p.info.get_key_int("bvb:can_evolve") != 0
                    || (want_cost > 0 && p.info.has_claim(&format!("bvb:evolve_cost:{want_cost}")))
            })
        }
        "IsCore" => {
            let want = obj.get("IsCore").and_then(Value::as_bool).unwrap_or(true);
            let is_core = pc.is_some_and(|p| p.info.get_key_int("bvb:is_core") != 0);
            is_core == want
        }
        "ActiveSkillTag" => {
            let tag = json_int(obj.get("ActiveSkillTag")).unwrap_or(0);
            pc.is_some_and(|p| {
                p.info.has_claim(&format!("skill:active_tag:{tag}"))
                    || p.info.get_key_int("skill:active_tag") == i64::from(tag)
            })
        }
        "HasOptimalFactor" => {
            resolve_var_value(ctx, obj.get("HandCard")) > 0
                || pc.is_some_and(|p| p.info.get_key_int("bvb:optimal_factor") != 0)
        }
        "CheckBVBEvent" => {
            let ev = get_param_str(obj, "EventName").unwrap_or("");
            !ev.is_empty()
                && pc.is_some_and(|p| {
                    p.info.has_claim(&format!("bvb:event:{ev}"))
                        || p.info.get_key_str("bvb:last_event") == ev
                })
        }
        "CheckChessWinner" => {
            let cid = json_int(obj.get("ChessboardId")).unwrap_or(0);
            let want = get_param_str(obj, "Winner").unwrap_or("Player");
            let cur = pc
                .map(|p| p.info.get_key_str(&format!("chess:winner:{cid}")))
                .unwrap_or("");
            cur == want
                || (cur.is_empty()
                    && want == "Player"
                    && pc.is_some_and(|p| p.info.has_claim(&format!("chess:done:{cid}"))))
        }
        "CheckHasFollowShooter" => {
            let want = obj.get("Has").and_then(Value::as_bool).unwrap_or(true);
            let sid = json_int(obj.get("Id")).unwrap_or(0);
            let has = pc.is_some_and(|p| {
                p.info.has_claim(&format!("follow_shooter:{sid}"))
                    || (sid > 0 && p.info.get_key_int("follow_shooter:active") == i64::from(sid))
            });
            has == want
        }
        "CheckIsGramophonePlayingMusic" => {
            let gid = json_int(obj.get("TargetGramophone")).unwrap_or(0);
            let cur_music = pc.map_or(0, |p| {
                let g_val = p.info.get_key_int(&format!("gramophone:music:{gid}"));
                if g_val > 0 {
                    g_val
                } else {
                    p.info.get_key_int("phantom_music:cur")
                }
            });
            let playlist: Vec<i64> = obj
                .get("CheckCondition")
                .and_then(Value::as_object)
                .and_then(|cc| cc.get("PlayList"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| json_int(Some(v)).map(i64::from))
                        .collect()
                })
                .unwrap_or_default();
            if playlist.is_empty() {
                cur_music > 0
            } else {
                playlist.contains(&cur_music)
            }
        }
        "CheckPhotographCameraParam" => {
            let params = obj
                .get("CameraParam")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if params.is_empty() {
                pc.is_some_and(|p| p.info.has_claim("photo:submitted"))
            } else {
                pc.is_some_and(|p| {
                    p.info.has_claim("photo:submitted")
                        || params.iter().any(|cp| {
                            cp.as_object()
                                .and_then(|co| json_int(co.get("TemplateId")))
                                .is_some_and(|tid| p.info.has_claim(&format!("photo:param:{tid}")))
                        })
                })
            }
        }
        "CheckPlayerUseSkill" => {
            let skills = obj
                .get("CheckSkills")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            pc.is_some_and(|p| {
                skills.iter().any(|sk| {
                    sk.as_object()
                        .and_then(|so| json_int(so.get("SkillId")))
                        .is_some_and(|sid| p.info.has_claim(&format!("skill:used:{sid}")))
                })
            })
        }
        "CheckRoverRogueAbilitySelect" => {
            let want_recv = obj
                .get("IsReceived")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let st = get_param_str(obj, "SelectType").unwrap_or("");
            let selected = pc.is_some_and(|p| match st {
                "BlessGroup" => {
                    p.info.get_key_int("rover_rogue:bless_group") > 0
                        || p.info.has_claim("rover_rogue:select:BlessGroup")
                }
                _ => p.info.has_claim(&format!("rover_rogue:select:{st}")),
            });
            selected == want_recv
        }
        _ => return None,
    };
    Some(res)
}

fn eval_check_player_position(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let want_on = obj.get("IsOnRange").and_then(Value::as_bool).unwrap_or(true);
    let Some(range_obj) = obj.get("Range").and_then(Value::as_object) else {
        return !want_on;
    };
    let Some(base_pos) = parse_json_vector_pos(range_obj.get("Pos")) else {
        return !want_on;
    };
    let Some(player_pos) = current_player_pos_3d(ctx) else {
        return !want_on;
    };
    let mut shape = serde_json::from_value::<ShapeConfig>(Value::Object(range_obj.clone()))
        .unwrap_or_default();
    if shape.Type.as_deref() == Some("BoxRange") {
        shape.Type = Some("Box".to_string());
    } else if shape.Type.as_deref() == Some("CircleRange") {
        shape.Type = Some("Sphere".to_string());
    }
    let inside = is_pos_inside_range_shape(
        player_pos,
        base_pos,
        [0.0, 0.0, 0.0],
        &shape,
        500.0,
        1.0,
    );
    inside == want_on
}

fn quest_state_value(ctx: &EvalContext<'_>, qid: i32) -> i32 {
    if ctx.quest_comp.info.has_finished_quest(qid) {
        return 3;
    }
    if ctx.quest_comp.info.has_active_quest(qid) {
        return 2;
    }
    let depth = QUEST_STATE_EVAL_DEPTH.with(Cell::get);
    if depth < 4
        && let Some(a) = ctx.assets
        && let Some(q) = a.tables.quest_data.get_data_by_id(&qid)
    {
        QUEST_STATE_EVAL_DEPTH.with(|d| d.set(depth + 1));
        let _guard = QuestStateDepthGuard;
        if eval_quest_provide_conditions(ctx, q.Data.ProvideType.as_ref()) {
            return 1;
        }
    }
    0
}

fn eval_inline_node_status(ctx: &EvalContext<'_>, qid: i32, nid: i32, want_st: i32) -> bool {
    match want_st {
        4 => {
            if qid > 0 {
                return ctx.quest_comp.info.has_finished_child_quest(qid, nid);
            }
            ctx.quest_comp
                .info
                .finished_child_quests
                .iter()
                .any(|rec| rec.node_id == nid)
                || ctx
                    .quest_comp
                    .info
                    .active_quests
                    .iter()
                    .any(|aq| aq.is_node_finished(nid))
        }
        1 | 2 => {
            if qid > 0 {
                return ctx
                    .quest_comp
                    .info
                    .find_active_quest(qid)
                    .is_some_and(|aq| aq.is_node_active(nid));
            }
            ctx.quest_comp
                .info
                .active_quests
                .iter()
                .any(|aq| aq.is_node_active(nid))
        }
        0 => {
            if qid > 0 {
                if ctx.quest_comp.info.has_finished_child_quest(qid, nid) {
                    return false;
                }
                if let Some(aq) = ctx.quest_comp.info.find_active_quest(qid) {
                    return !aq.is_node_active(nid) && !aq.is_node_finished(nid);
                }
                return true;
            }
            if ctx
                .quest_comp
                .info
                .finished_child_quests
                .iter()
                .any(|rec| rec.node_id == nid)
            {
                return false;
            }
            !ctx.quest_comp
                .info
                .active_quests
                .iter()
                .any(|aq| aq.is_node_active(nid) || aq.is_node_finished(nid))
        }
        _ => false,
    }
}

fn eval_check_entities_exist(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(scene) = ctx.scene else {
        return false;
    };
    let Some(ids_val) = obj.get("EntityIds").and_then(Value::as_array) else {
        return false;
    };
    if ids_val.is_empty() {
        return false;
    }
    for id_val in ids_val {
        let Some(eid) = json_int(Some(id_val)) else {
            continue;
        };
        let found = scene
            .entities
            .iter()
            .any(|ent| ent.config.config_id == eid && ent.visible);
        if !found {
            return false;
        }
    }
    true
}

pub fn eval_player_motion_state(ctx: &EvalContext<'_>, motion: &str) -> Option<bool> {
    let scene = ctx.scene?;
    let role_entity_id = scene.current_role_entity_id();
    if role_entity_id <= 0 {
        return None;
    }
    let &entity_index = scene.net_id_map.get(&role_entity_id)?;
    let entity = scene.entities.get(entity_index)?;
    let logic_state = entity.logic_state.as_ref()?;

    let ps = logic_state.position_state;
    let ms = logic_state.move_state;
    let pss = logic_state.position_sub_state;
    let matched = match motion {
        "Ground" => ps == 0 && pss == 0,
        "Air" => ps == 2,
        "Climb" => ps == 1,
        "OnClimbing" => ps == 1 && (ms == 0 || ms == 14 || ms == 15),
        "Water" => ps == 3,
        "WalkOnWater" => ps == 0 && pss == 1,
        "GlideInAir" => ps == 2 && ms == 16,
        "FallInAir" => ps == 2 && ms == 0,
        "Soaring" => ps == 2 && ms == 28,
        "Gongduola" => ps == 5 && ms == 31,
        "SwitchSkill" | "UltimateDodge" | "UltimateSkill" => {
            let tag_name = match motion {
                "SwitchSkill" => "角色.Common.技能通用标识.QTE",
                "UltimateDodge" => "角色.Common.技能通用标识.极限闪避",
                _ => "角色.Common.技能通用标识.大招",
            };
            let assets = ctx.assets?;
            let tag_id = gameplay_tags::id_from_name(&assets.tables.gameplay_tag_remap, tag_name)?;
            let tag_comp = entity.tag.as_ref()?;
            return Some(tag_comp.has_tag(&assets.tables.gameplay_tag_parent, tag_id));
        }
        _ => return None,
    };
    Some(matched)
}

fn json_float_val(v_opt: Option<&Value>) -> Option<f32> {
    let v = v_opt?;
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        _ => None,
    }
}

pub fn current_player_pos_3d(ctx: &EvalContext<'_>) -> Option<[f32; 3]> {
    let scene = ctx.scene?;
    if let Some(first) = scene.instance.players.first() {
        return Some(first.location);
    }
    Some(scene.last_spatial_check_loc)
}

fn rotate_inverse_euler(v: [f32; 3], rot_deg: [f32; 3]) -> [f32; 3] {
    if rot_deg[0] == 0.0 && rot_deg[1] == 0.0 && rot_deg[2] == 0.0 {
        return v;
    }
    let deg2rad = std::f32::consts::PI / 180.0;
    let (sy, cy) = (rot_deg[2] * deg2rad).sin_cos();
    let x1 = v[0] * cy + v[1] * sy;
    let y1 = -v[0] * sy + v[1] * cy;
    let z1 = v[2];

    let (sp, cp) = (rot_deg[1] * deg2rad).sin_cos();
    let x2 = x1 * cp + z1 * sp;
    let y2 = y1;
    let z2 = -x1 * sp + z1 * cp;

    let (sr, cr) = (rot_deg[0] * deg2rad).sin_cos();
    [x2, y2 * cr - z2 * sr, y2 * sr + z2 * cr]
}

/// Evaluates oriented 3D containment for `RangeComponent` shapes (`Sphere`, `HollowSphere`,
/// `Cylinder`, `HollowCylinder`, `Box`, `Combination`, and fallback spherical/box shapes).
pub fn is_pos_inside_range_shape(
    pos: [f32; 3],
    base_pos: [f32; 3],
    entity_rot: [f32; 3],
    shape: &ShapeConfig,
    default_radius: f32,
    min_sphere_radius: f32,
) -> bool {
    is_pos_inside_range_shape_depth(
        pos,
        base_pos,
        entity_rot,
        shape,
        default_radius,
        min_sphere_radius,
        0,
    )
}

fn is_pos_inside_range_shape_depth(
    pos: [f32; 3],
    base_pos: [f32; 3],
    entity_rot: [f32; 3],
    shape: &ShapeConfig,
    default_radius: f32,
    min_sphere_radius: f32,
    depth: u8,
) -> bool {
    let stype = shape.Type.as_deref().unwrap_or("");
    if stype == "Combination" || (shape.CombinationShapes.is_some() && stype.is_empty()) {
        if depth >= 4 {
            return false;
        }
        let Some(subs) = shape.CombinationShapes.as_deref() else {
            return false;
        };
        return subs.iter().any(|sub| {
            is_pos_inside_range_shape_depth(
                pos,
                base_pos,
                entity_rot,
                sub,
                default_radius,
                min_sphere_radius,
                depth + 1,
            )
        });
    }

    let center_off = shape.Center.as_ref().map_or([0.0, 0.0, 0.0], |c| {
        [
            c.X.unwrap_or(0.0),
            c.Y.unwrap_or(0.0),
            c.Z.unwrap_or(0.0),
        ]
    });

    let rel = [
        pos[0] - base_pos[0],
        pos[1] - base_pos[1],
        pos[2] - base_pos[2],
    ];

    let shape_rot = shape.Rotator.as_ref().map_or([0.0, 0.0, 0.0], |sr| {
        [
            sr.X.unwrap_or(0.0),
            sr.Y.unwrap_or(0.0),
            sr.Z.unwrap_or(0.0),
        ]
    });

    let ent_local = rotate_inverse_euler(rel, entity_rot);
    let shifted = [
        ent_local[0] - center_off[0],
        ent_local[1] - center_off[1],
        ent_local[2] - center_off[2],
    ];
    let local = rotate_inverse_euler(shifted, shape_rot);
    let (dx, dy, dz) = (local[0], local[1], local[2]);

    let has_box_size = shape.Size.as_ref().is_some_and(|sz| {
        sz.X.unwrap_or(0.0) > 0.0 || sz.Y.unwrap_or(0.0) > 0.0 || sz.Z.unwrap_or(0.0) > 0.0
    });
    let is_box_like = stype == "Box"
        || stype == "TransportBox"
        || ((stype.is_empty()
            || stype == "Volume"
            || stype == "ActorCollision"
            || stype == "ActorRefVolume")
            && has_box_size);
    if is_box_like {
        let Some(sz) = &shape.Size else {
            return false;
        };
        let sx = sz.X.filter(|&v| v > 0.0).unwrap_or(100.0);
        let sy = sz.Y.filter(|&v| v > 0.0).unwrap_or(100.0);
        let sz_v = sz.Z.filter(|&v| v > 0.0).unwrap_or(100.0);
        return dx.abs() <= sx && dy.abs() <= sy && dz.abs() <= sz_v;
    }

    let r = shape.Radius.unwrap_or(default_radius).max(min_sphere_radius);
    if stype == "Cone" {
        let dist_sq = dx * dx + dy * dy + dz * dz;
        let max_r = shape.Height.filter(|&h| h > 0.0).map_or(r, |h| r.max(h));
        if dist_sq > max_r * max_r {
            return false;
        }
        if let Some(ang_deg) = shape.Angle {
            let dist = dist_sq.sqrt();
            if dist > 0.001 {
                let cos_half = ((ang_deg * 0.5).min(180.0) * (std::f32::consts::PI / 180.0)).cos();
                if (dx / dist) < cos_half {
                    return false;
                }
            }
        }
        return true;
    }

    if stype == "Cylinder"
        || stype == "HollowCylinder"
        || ((stype == "Volume" || stype == "ActorCollision" || stype == "ActorRefVolume")
            && shape.Height.is_some())
    {
        let xy_sq = dx * dx + dy * dy;
        let half_h = shape
            .Height
            .filter(|&h| h > 0.0)
            .map_or_else(|| r.max(1500.0), |h| h * 0.5);
        if xy_sq > r * r || dz.abs() > half_h {
            return false;
        }
        if stype == "HollowCylinder" {
            let inner_r = shape.InnerRadius.unwrap_or(0.0);
            if inner_r > 0.0 && xy_sq < inner_r * inner_r {
                return false;
            }
        }
        return true;
    }

    let dist_sq = dx * dx + dy * dy + dz * dz;
    if dist_sq > r * r {
        return false;
    }
    if stype == "HollowSphere" {
        let inner_r = shape.InnerRadius.unwrap_or(0.0);
        if inner_r > 0.0 && dist_sq < inner_r * inner_r {
            return false;
        }
    }
    true
}

fn eval_check_in_range(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(scene) = ctx.scene else {
        return false;
    };
    let Some(range_entities) = obj.get("RangeEntities").and_then(Value::as_array) else {
        return false;
    };
    if range_entities.is_empty() {
        return false;
    }
    let Some(player_pos) = current_player_pos_3d(ctx) else {
        return false;
    };

    for ent_val in range_entities {
        let Some(eid) = json_int(Some(ent_val)) else {
            continue;
        };
        let mut ent_pos: Option<[f32; 3]> = None;
        let mut ent_rot = [0.0f32; 3];
        if let Some(ent) = scene.entities.iter().find(|e| e.config.config_id == eid)
            && let Some(p) = ent.position
        {
            ent_pos = Some(p.location);
            ent_rot = p.rotation;
        }
        let mut shape = ShapeConfig::default();
        if let Some(a) = ctx.assets
            && let Some(lcfg) = a.find_level_entity(scene.instance_id, i64::from(eid))
        {
            if ent_pos.is_none() {
                ent_pos = asset_indexes::entity_world_pos(&lcfg);
                ent_rot = asset_indexes::entity_world_rot(&lcfg);
            }
            if let Some(rc) = &lcfg.ComponentsData.RangeComponent
                && let Some(sh) = &rc.Shape
            {
                shape = sh.clone();
            } else if let Some(tpl) = a.find_template_config(&lcfg.BlueprintType)
                && let Some(rc) = &tpl.ComponentsData.RangeComponent
                && let Some(sh) = &rc.Shape
            {
                shape = sh.clone();
            }
        }
        let Some(pos) = ent_pos else {
            continue;
        };
        if is_pos_inside_range_shape(player_pos, pos, ent_rot, &shape, 1500.0, 0.0) {
            return true;
        }
    }
    false
}

fn eval_quest_state(ctx: &EvalContext<'_>, qid: i32, st: i32) -> bool {
    quest_state_value(ctx, qid) == st
}

fn eval_quest_node_status(ctx: &EvalContext<'_>, qid: i32, nid: i32, st: i32) -> bool {
    let aq = ctx.quest_comp.info.find_active_quest(qid);
    match st {
        0 => {
            if let Some(q) = aq {
                !q.is_node_active(nid) && !q.is_node_finished(nid)
            } else {
                !ctx.quest_comp.info.has_finished_child_quest(qid, nid)
            }
        }
        1 | 2 => aq.is_some_and(|q| q.is_node_active(nid)),
        3 | 4 => {
            ctx.quest_comp.info.has_finished_child_quest(qid, nid)
                || aq.is_some_and(|q| q.is_node_finished(nid))
        }
        _ => false,
    }
}

pub fn is_activity_open(ctx: &EvalContext<'_>, activity_id: i32) -> bool {
    if ctx
        .quest_comp
        .info
        .progress
        .opened_activities
        .contains(&activity_id)
    {
        return true;
    }
    ctx.assets
        .is_some_and(|a| a.tables.activity.get_data_by_id(&activity_id).is_some())
}

fn scoped_var_name(ctx: &EvalContext<'_>, var_obj: &Map<String, Value>, name: &str) -> String {
    let src = get_param_str(var_obj, "Source").unwrap_or("Constant");
    if src == "Other" {
        let ref_type = get_param_str(var_obj, "RefType").unwrap_or("");
        let ref_id = json_int(var_obj.get("RefId")).unwrap_or(0);
        if !ref_type.is_empty() && ref_id != 0 {
            return format!("{ref_type}:{ref_id}:{name}");
        }
    } else if src == "Self" {
        if ctx.owner_quest_id > 0 {
            let k = format!("Self:{}:{name}", ctx.owner_quest_id);
            if ctx.quest_comp.info.has_var(&k) {
                return k;
            }
        }
        if let Some(scene) = ctx.scene {
            if scene.active_flow_owner_entity_id != 0 {
                let k = format!("EntSelf:{}:{name}", scene.active_flow_owner_entity_id);
                if ctx.quest_comp.info.has_var(&k) {
                    return k;
                }
            }
            if scene.active_flow_id > 0 {
                let k = format!("FlowSelf:{}:{name}", scene.active_flow_id);
                if ctx.quest_comp.info.has_var(&k) {
                    return k;
                }
            }
        }
    }
    name.to_string()
}

pub(crate) fn lookup_entity_config_var(
    ctx: &EvalContext<'_>,
    obj: &Map<String, Value>,
    name: &str,
) -> Option<Value> {
    if get_param_str(obj, "Source")? != "Other" {
        return None;
    }
    if get_param_str(obj, "RefType")? != "Entity" {
        return None;
    }
    let ref_id = match obj.get("RefId")? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64))?,
        _ => return None,
    };
    let assets = ctx.assets?;
    let map_id = ctx
        .scene
        .map_or(8, |scene| assets.resolve_map_config_id(scene.instance_id));
    let cfg = assets.find_level_entity(map_id, ref_id)?;
    let vc = cfg.ComponentsData.VarComponent.as_ref()?;
    if vc.Disabled.unwrap_or(false) {
        return None;
    }
    vc.Vars
        .iter()
        .find(|v| v.Name == name)
        .map(|v| v.Value.clone())
}

fn resolve_var_str(ctx: &EvalContext<'_>, var_opt: Option<&Value>) -> Option<String> {
    let v = var_opt.and_then(Value::as_object)?;
    let src = get_param_str(v, "Source").unwrap_or("Constant");
    if src == "Constant" {
        let Some(val) = v.get("Value") else {
            return Some(String::new());
        };
        return val.as_str().map(str::to_string);
    }
    let name = get_param_str(v, "Name")?;
    let scoped = scoped_var_name(ctx, v, name);
    let val = ctx.quest_comp.info.get_var_str(&scoped);
    if !val.is_empty() {
        return Some(val.to_string());
    }
    if let Some(init_val) = lookup_entity_config_var(ctx, v, name)
        && let Some(s) = init_val.as_str()
    {
        return Some(s.to_string());
    }
    if scoped == name {
        return Some(val.to_string());
    }
    Some(ctx.quest_comp.info.get_var_str(name).to_string())
}

fn resolve_var_float(ctx: &EvalContext<'_>, var_opt: Option<&Value>) -> f64 {
    let Some(v) = var_opt.and_then(Value::as_object) else {
        return 0.0;
    };
    let src = get_param_str(v, "Source").unwrap_or("Constant");
    if src == "Constant" {
        return match v.get("Value") {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Value::Bool(true)) => 1.0,
            Some(Value::Bool(false)) => 0.0,
            _ => 0.0,
        };
    }
    let Some(name) = get_param_str(v, "Name") else {
        return 0.0;
    };
    let scoped = scoped_var_name(ctx, v, name);
    let s = ctx.quest_comp.info.get_var_str(&scoped);
    if !s.is_empty()
        && let Ok(f) = s.parse::<f64>()
    {
        return f;
    }
    let iv = ctx.quest_comp.info.get_var_int(&scoped);
    if iv != 0 {
        return f64::from(iv);
    }
    if let Some(init_val) = lookup_entity_config_var(ctx, v, name) {
        return match init_val {
            Value::Number(n) => n.as_f64().unwrap_or(0.0),
            Value::Bool(true) => 1.0,
            Value::Bool(false) => 0.0,
            _ => 0.0,
        };
    }
    if scoped == name {
        0.0
    } else {
        f64::from(ctx.quest_comp.info.get_var_int(name))
    }
}

/// Integer value of a level-data variable reference (`{"Type","Source","Name"|"Value"...}`).
pub fn resolve_int_var(ctx: &EvalContext<'_>, var_opt: Option<&Value>) -> i32 {
    resolve_var_value(ctx, var_opt)
}

fn resolve_var_value(ctx: &EvalContext<'_>, var_opt: Option<&Value>) -> i32 {
    let Some(v) = var_opt.and_then(Value::as_object) else {
        return 0;
    };
    let src = v
        .get("Source")
        .and_then(Value::as_str)
        .unwrap_or("Constant");
    if src == "Constant" {
        return match v.get("Value") {
            Some(Value::Number(n)) => n
                .as_i64()
                .and_then(|i| i32::try_from(i).ok())
                .unwrap_or(0),
            Some(Value::Bool(b)) => i32::from(*b),
            _ => 0,
        };
    }
    let Some(name) = v.get("Name").and_then(Value::as_str) else {
        return 0;
    };
    let scoped = scoped_var_name(ctx, v, name);
    if let Some(vt) = get_param_str(v, "Type")
        && (vt == "Boolean" || vt == "Bool")
        && (ctx.quest_comp.info.get_var_bool(&scoped)
            || (scoped != name && ctx.quest_comp.info.get_var_bool(name)))
    {
        return 1;
    }
    let val = ctx.quest_comp.info.get_var_int(&scoped);
    if val != 0 {
        return val;
    }
    if let Some(init_val) = lookup_entity_config_var(ctx, v, name) {
        return match init_val {
            Value::Number(n) => n
                .as_i64()
                .and_then(|i| i32::try_from(i).ok())
                .unwrap_or(0),
            Value::Bool(b) => i32::from(b),
            _ => 0,
        };
    }
    if scoped == name {
        val
    } else {
        ctx.quest_comp.info.get_var_int(name)
    }
}

pub fn eval_compare_var_condition(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let cmp = get_param_str(obj, "Compare").unwrap_or("Eq");
    let v1_opt = obj.get("Var1");
    let v2_opt = obj.get("Var2");
    let t1 = v1_opt
        .and_then(Value::as_object)
        .and_then(|o| get_param_str(o, "Type"));
    let t2 = v2_opt
        .and_then(Value::as_object)
        .and_then(|o| get_param_str(o, "Type"));
    if t1 == Some("String") || t2 == Some("String") {
        let Some(s1) = resolve_var_str(ctx, v1_opt) else {
            return false;
        };
        let Some(s2) = resolve_var_str(ctx, v2_opt) else {
            return false;
        };
        let eq = s1 == s2;
        if is_neg_compare(cmp) {
            return !eq;
        }
        if cmp == "Eq" || cmp == "==" {
            return eq;
        }
        return false;
    }
    if t1 == Some("Float") || t2 == Some("Float") {
        return compare_floats(
            resolve_var_float(ctx, v1_opt),
            resolve_var_float(ctx, v2_opt),
            cmp,
        );
    }
    compare_ints(
        resolve_var_value(ctx, v1_opt),
        resolve_var_value(ctx, v2_opt),
        cmp,
    )
}

fn compare_ints(a: i32, b: i32, cmp: &str) -> bool {
    match cmp {
        "Eq" | "==" => a == b,
        "Ne" | "NotEq" | "!=" => a != b,
        "Gt" | "Great" | ">" => a > b,
        "Ge" | "GtEq" | "GreatEq" | ">=" => a >= b,
        "Lt" | "Less" | "<" => a < b,
        "Le" | "LtEq" | "LessEq" | "<=" => a <= b,
        _ => a == b,
    }
}

fn compare_floats(a: f64, b: f64, cmp: &str) -> bool {
    match cmp {
        "Eq" | "==" => a == b,
        "Ne" | "NotEq" | "!=" => a != b,
        "Gt" | "Great" | ">" => a > b,
        "Ge" | "GtEq" | "GreatEq" | ">=" => a >= b,
        "Lt" | "Less" | "<" => a < b,
        "Le" | "LtEq" | "LessEq" | "<=" => a <= b,
        _ => false,
    }
}

pub fn current_calabash_level(
    assets: &Assets,
    basic_comp: &PlayerBasicComponent,
    echo_comp: &PlayerEchoComponent,
) -> i32 {
    calabash::current_level(assets, basic_comp, echo_comp)
}

fn item_count(ctx: &EvalContext<'_>, item_id: i32) -> i32 {
    if item_id == 2 {
        return ctx.basic_comp.info.attributes.coin;
    }
    if item_id == 3 {
        return ctx
            .basic_comp
            .info
            .attributes
            .rare_coin
            .max(ctx.inventory_comp.info.normal_item_count(3));
    }
    ctx.inventory_comp.info.normal_item_count(item_id)
}

fn fishing_prestige_level(assets: &Assets, exp: i32) -> i32 {
    let mut level = 1;
    for row in &assets.tables.fishing_reputation.items {
        if exp >= row.Exp {
            level = level.max(row.Level);
        }
    }
    level
}

fn count_dango_above_level(assets: &Assets, info: &ProgressInfo, target_level: i32) -> i32 {
    let prefix = "abyss:lrole:";
    let mut count = 0;
    for entry in &info.dynamic.ints {
        let Some(id_text) = entry.key.strip_prefix(prefix) else {
            continue;
        };
        let Ok(dango_id) = id_text.parse::<i32>() else {
            continue;
        };
        if assets
            .tables
            .abyss_little_role
            .get_data_by_id(&dango_id)
            .is_none()
        {
            continue;
        }
        if entry.val > i64::from(target_level) {
            count += 1;
        }
    }
    count
}

fn json_has_type(obj: &Map<String, Value>, expected: &str) -> bool {
    obj.get("Type").and_then(Value::as_str) == Some(expected)
}

fn current_formation_role_id(ctx: &EvalContext<'_>, index: i32) -> Option<i32> {
    let slot = usize::try_from(index).ok()?;
    let scene = ctx.scene?;
    let formation_index = usize::try_from(scene.formation_info.cur_formation).ok()?;
    let formation = scene.formation_info.formations.get(formation_index)?;
    formation.roles.get(slot)?.as_ref().map(|r| r.role_id)
}

fn eval_player_gender(ctx: &EvalContext<'_>, gval_opt: Option<&Value>) -> bool {
    let Some(g) = gval_opt.and_then(Value::as_str) else {
        return true;
    };
    match g {
        "男" | "Male" => ctx.basic_comp.info.attributes.sex == 1,
        "女" | "Female" => ctx.basic_comp.info.attributes.sex == 0,
        _ => false,
    }
}

fn eval_open_date_condition(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let date_id = json_int(obj.get("DateId")).unwrap_or(0);
    if date_id > 0
        && let Some(pc) = ctx.progress_comp
        && pc.info.get_key_int(&format!("open_date:{date_id}")) < 0
    {
        return false;
    }
    true
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn matches_vehicle_type(ctx: &EvalContext<'_>, cfg_id: i64, want_vtype: &str) -> bool {
    if want_vtype.is_empty() {
        return true;
    }
    if let Some(assets) = ctx.assets {
        let map_id = ctx.scene.map_or(8, |sc| sc.instance_id);
        if let Some(cfg) = assets.find_level_entity(map_id, cfg_id)
            && (contains_case_insensitive(&cfg.BlueprintType, want_vtype)
                || (want_vtype == "Motorcycle"
                    && contains_case_insensitive(&cfg.BlueprintType, "Motor"))
                || (want_vtype == "FishingBoat"
                    && (contains_case_insensitive(&cfg.BlueprintType, "Fishing")
                        || contains_case_insensitive(&cfg.BlueprintType, "Boat")))
                || (want_vtype == "Gongduola"
                    && contains_case_insensitive(&cfg.BlueprintType, "Gondola")))
        {
            return true;
        }
    }
    if want_vtype == "FishingBoat" {
        return ctx
            .progress_comp
            .is_some_and(|pc| pc.info.get_key_int("fish:boat_active") != 0);
    }
    if want_vtype == "Motorcycle" {
        return ctx.progress_comp.is_some_and(|pc| {
            pc.info.get_key_int("motor:riding") != 0 || pc.info.get_key_int("motor:summoned") != 0
        });
    }
    false
}

fn resolve_target_net_id(scene: &Scene, target_val_opt: Option<&Value>) -> Option<i64> {
    let target = target_val_opt?.as_object()?;
    resolve_target_net_id_val(scene, target)
}

fn resolve_target_net_id_val(scene: &Scene, target: &Map<String, Value>) -> Option<i64> {
    let target_type = get_param_str(target, "Type")?;
    match target_type {
        "Player" | "OnlinePlayer" => {
            let eid = scene.current_role_entity_id();
            (eid > 0).then_some(eid)
        }
        "Monster" => {
            let cfg_id = json_int(target.get("MonsterEntityId"))
                .or_else(|| json_int(target.get("EntityId")))?;
            scene.find_entity_net_id_by_config_id(cfg_id)
        }
        "Entity" | "Target" | "Npc" | "SceneItem" => {
            let cfg_id = json_int(target.get("EntityId"))
                .or_else(|| json_int(target.get("NpcEntityId")))?;
            scene.find_entity_net_id_by_config_id(cfg_id)
        }
        _ => None,
    }
}

fn eval_entity_attribute_condition_list(
    ctx: &EvalContext<'_>,
    scene: &Scene,
    entity_idx: usize,
    items: &[Value],
) -> bool {
    let Some(entity) = scene.entities.get(entity_idx) else {
        return false;
    };
    let Some(attr) = entity.attribute.as_ref() else {
        return false;
    };
    let buffs_opt = entity.buffs.as_ref();

    let life_idx = EAttributeType::Life.to_i32() as usize;
    let max_idx = EAttributeType::LifeMax.to_i32() as usize;
    let max_life = attr.attributes[max_idx].current;

    for attribute_condition in items {
        let Some(ac_obj) = attribute_condition.as_object() else {
            return false;
        };
        let Some(attribute_type) = get_param_str(ac_obj, "Type") else {
            return false;
        };
        let Some(cmp) = get_param_str(ac_obj, "Compare") else {
            return false;
        };
        let Some(target_value) = ac_obj.get("Value").and_then(Value::as_f64) else {
            return false;
        };

        let actual_val = match attribute_type {
            "Health" => {
                if max_life <= 0 {
                    return false;
                }
                f64::from(attr.attributes[life_idx].current) / f64::from(max_life) * 100.0
            }
            "Energy" => {
                let e_idx = EAttributeType::Energy.to_i32() as usize;
                let emax_idx = EAttributeType::EnergyMax.to_i32() as usize;
                let cur_e = attr.attributes[e_idx].current;
                let max_e = attr.attributes[emax_idx].current;
                if max_e > 0 {
                    f64::from(cur_e) / f64::from(max_e) * 100.0
                } else {
                    f64::from(cur_e)
                }
            }
            "Shield" => {
                let total_shield = buffs_opt.map_or(0, |b| b.total_shield());
                if max_life > 0 {
                    f64::from(total_shield) / f64::from(max_life) * 100.0
                } else {
                    f64::from(total_shield)
                }
            }
            "StabilityPoint" | "Tough" => {
                let t_idx = EAttributeType::Tough.to_i32() as usize;
                let tmax_idx = EAttributeType::ToughMax.to_i32() as usize;
                let cur_t = attr.attributes[t_idx].current;
                let max_t = attr.attributes[tmax_idx].current;
                if max_t > 0 {
                    f64::from(cur_t) / f64::from(max_t) * 100.0
                } else if let Some(pc) = ctx.progress_comp
                    && pc.info.get_key_int("team:stability") > 0
                {
                    pc.info.get_key_int("team:stability") as f64
                } else {
                    return false;
                }
            }
            _ => return false,
        };

        if !compare_floats(actual_val, target_value, cmp) {
            return false;
        }
    }
    true
}

fn eval_entity_gravity_direction(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(scene) = ctx.scene else {
        return false;
    };
    let entity_id = if let Some(target_val) = obj.get("Target") {
        let Some(target) = target_val.as_object() else {
            return false;
        };
        let Some(ty) = get_param_str(target, "Type") else {
            return false;
        };
        if ty == "OnlinePlayer" {
            let Some(opt) = target
                .get("OnlinePlayerConditionTargetOption")
                .and_then(Value::as_object)
            else {
                return false;
            };
            let opt_ty = get_param_str(opt, "Type").unwrap_or("");
            if opt_ty != "Host" && opt_ty != "Participator" {
                return false;
            }
            scene.current_role_entity_id()
        } else {
            let Some(id) = resolve_target_net_id_val(scene, target) else {
                return false;
            };
            id
        }
    } else {
        scene.current_role_entity_id()
    };
    if entity_id <= 0 {
        return false;
    }
    let Some(&entity_index) = scene.net_id_map.get(&entity_id) else {
        return false;
    };
    let Some(entity) = scene.entities.get(entity_index) else {
        return false;
    };

    let ent_rot = entity.position.map_or([0.0, 0.0, 0.0], |p| p.rotation);
    let Some(direction) = obj.get("GravityDirection").and_then(Value::as_object) else {
        return false;
    };
    let Some(dir_type) = get_param_str(direction, "Type") else {
        return false;
    };

    let expected = match dir_type {
        "WorldAxis" => {
            let Some(axis) = get_param_str(direction, "WorldAxis") else {
                return false;
            };
            let res = gravity::resolve_direction(
                &GravityDirection {
                    Type: Some("WorldAxis".to_string()),
                    WorldAxis: Some(axis.to_string()),
                    Direction: None,
                },
                ent_rot,
            );
            if !res.configured {
                return false;
            }
            res.dir
        }
        "SelfRotation" => {
            gravity::resolve_direction(
                &GravityDirection {
                    Type: Some("SelfRotation".to_string()),
                    WorldAxis: None,
                    Direction: None,
                },
                ent_rot,
            )
            .dir
        }
        "Vector" | "VectorInfo" | "Direction" => {
            let Some(vec_val) = direction
                .get("Direction")
                .or_else(|| direction.get("Vector"))
                .or_else(|| direction.get("VectorInfo"))
                .and_then(Value::as_object)
            else {
                return false;
            };
            let vx = json_float_val(vec_val.get("X")).unwrap_or(0.0);
            let vy = json_float_val(vec_val.get("Y")).unwrap_or(0.0);
            let vz = json_float_val(vec_val.get("Z")).unwrap_or(0.0);
            let len = (vx * vx + vy * vy + vz * vz).sqrt();
            if len <= 0.0001 {
                return false;
            }
            [vx / len, vy / len, vz / len]
        }
        _ => return false,
    };

    let actual = if let Some(grav) = entity.world_item.gravity {
        grav.location
    } else if let Some(assets) = ctx.assets {
        let cfg_id = entity.config.config_id;
        assets
            .find_level_entity(scene.instance_id, i64::from(cfg_id))
            .map(|lcfg| gravity::resolve_entity_gravity(&lcfg, ent_rot))
            .filter(|gres| gres.configured)
            .map_or([0.0, 0.0, -1.0], |gres| gres.dir)
    } else {
        [0.0, 0.0, -1.0]
    };

    let matches = actual
        .iter()
        .zip(expected.iter())
        .all(|(&a, &e)| ((a * 1000.0).round() / 1000.0 - (e * 1000.0).round() / 1000.0).abs() <= 0.001);
    let invert = match obj.get("IsNotInTargetDirection") {
        Some(Value::Bool(enabled)) => *enabled,
        Some(_) => return false,
        None => false,
    };
    if invert { !matches } else { matches }
}

fn eval_formation_role_info(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(option) = obj.get("Option").and_then(Value::as_object) else {
        return false;
    };
    let Some(option_type) = get_param_str(option, "Type") else {
        return false;
    };
    match option_type {
        "RoleLevel" | "WeaponLevel" => {
            let Some(target) = option.get("Option").and_then(Value::as_object) else {
                return false;
            };
            if !json_has_type(target, "SpecifyRole") {
                return false;
            }
            let Some(slot) = json_int(target.get("Index")) else {
                return false;
            };
            let Some(required) = json_int(target.get("Level")) else {
                return false;
            };
            let compare = get_param_str(target, "Compare").unwrap_or("Ge");
            let Some(role_id) = current_formation_role_id(ctx, slot) else {
                return false;
            };
            let Some(role_comp) = ctx.role_comp else {
                return false;
            };
            let Some(role) = role_comp.role_map.get(&role_id) else {
                return false;
            };
            if option_type == "RoleLevel" {
                return compare_ints(role.level, required, compare);
            }
            let Some(weapon_comp) = ctx.weapon_comp else {
                return false;
            };
            let Some(weapon) = weapon_comp.weapon_map.get(&role.weapon) else {
                return false;
            };
            compare_ints(weapon.level, required, compare)
        }
        "HasEquippedVision" => {
            if option.get("Option").and_then(Value::as_str) != Some("AnyRole") {
                return false;
            }
            let Some(scene) = ctx.scene else {
                return false;
            };
            let Ok(formation_index) = usize::try_from(scene.formation_info.cur_formation) else {
                return false;
            };
            let Some(formation) = scene.formation_info.formations.get(formation_index) else {
                return false;
            };
            formation.roles.iter().flatten().any(|role| {
                ctx.echo_comp
                    .echo_map
                    .values()
                    .any(|e| e.role_id == Some(role.role_id) && e.equipped_pos >= 0)
            })
        }
        "HasUpgradableVision" => {
            if option.get("Option").and_then(Value::as_str) != Some("AnyRole") {
                return false;
            }
            let Some(scene) = ctx.scene else {
                return false;
            };
            let Some(assets) = ctx.assets else {
                return false;
            };
            let Ok(formation_index) = usize::try_from(scene.formation_info.cur_formation) else {
                return false;
            };
            let Some(formation) = scene.formation_info.formations.get(formation_index) else {
                return false;
            };
            formation.roles.iter().flatten().any(|role| {
                ctx.echo_comp.echo_map.values().any(|echo| {
                    echo.role_id == Some(role.role_id)
                        && echo.equipped_pos >= 0
                        && assets
                            .tables
                            .phantom_item
                            .get_data_by_id(&echo.id)
                            .and_then(|item| {
                                assets
                                    .tables
                                    .phantom_quality
                                    .get_data_by_id(&item.QualityId)
                            })
                            .is_some_and(|q| echo.level < q.LevelLimit)
                })
            })
        }
        "FeatureCollection" => {
            let Some(pool_id) = json_int(option.get("Id")) else {
                return false;
            };
            let Some(required) = json_int(option.get("Level")) else {
                return false;
            };
            let compare = get_param_str(option, "Compare").unwrap_or("Ge");
            let Some(progress) = ctx.progress_comp else {
                return false;
            };
            let Some(assets) = ctx.assets else {
                return false;
            };
            let Some(pool) = assets.tables.dragon_pool.get_data_by_id(&pool_id) else {
                return false;
            };
            let level = progress
                .info
                .get_key_int(&format!("mingsu:lvl:{}", pool.Id))
                .clamp(0, i64::from(i32::MAX)) as i32;
            compare_ints(level, required, compare)
        }
        _ => false,
    }
}

fn is_sun_spirit_collected(ctx: &EvalContext<'_>, eid: i64) -> bool {
    if let Ok(eid32) = i32::try_from(eid)
        && ctx.quest_comp.info.has_opened_chest(eid32)
    {
        return true;
    }
    if let Some(pc) = ctx.progress_comp {
        let key = format!("sun_spirit:{eid}");
        if pc.info.has_claim(&key) || pc.info.get_key_int(&key) > 0 {
            return true;
        }
    }
    false
}

fn sun_spirit_row_matches_area(
    assets: &Assets,
    map_id: i32,
    entity_id: i64,
    row_area_id: i32,
    req_area_id: i32,
    collect_only: bool,
) -> bool {
    let Some(lcfg) = assets.find_level_entity(map_id, entity_id) else {
        return !collect_only && (req_area_id <= 0 || row_area_id == req_area_id);
    };
    if lcfg.BlueprintType.contains("PPV") {
        return false;
    }
    let mut comps = lcfg.ComponentsData.clone();
    if let Some(tpl) = assets.find_template_config(&lcfg.BlueprintType) {
        tpl.ComponentsData.merge_into(&mut comps);
    }
    let mut has_collect = false;
    let mut comp_area: Option<i64> = None;
    for entry in comps.extra.iter().chain(comps.template_extra.iter()) {
        match entry {
            ExtraEntry::SunSpiritCollectComponent(sc) => {
                has_collect = true;
                if let Some(aid) = sc.AreaId {
                    comp_area = Some(aid);
                }
            }
            ExtraEntry::SunSpiritLauncherComponent(sl) if !collect_only && comp_area.is_none() => {
                comp_area = sl.AreaId;
            }
            _ => {}
        }
    }
    if collect_only && !has_collect {
        return false;
    }
    if req_area_id <= 0 {
        return true;
    }
    row_area_id == req_area_id || comp_area == Some(i64::from(req_area_id))
}

fn count_sun_spirits_filtered(
    ctx: &EvalContext<'_>,
    dungeon_id: i32,
    area_id: i32,
    want_collected: bool,
) -> i32 {
    let Some(assets) = ctx.assets else {
        return 0;
    };
    let collect_only = !want_collected;
    let mut count = 0;
    if let Some(db) = assets.tables.assets_db() {
        let Ok(rows) = db.query_sun_spirit_entities() else {
            return 0;
        };
        for row in rows {
            if dungeon_id > 0 && row.map_id != dungeon_id {
                continue;
            }
            if !sun_spirit_row_matches_area(
                assets,
                row.map_id,
                row.entity_id,
                row.area_id,
                area_id,
                collect_only,
            ) {
                continue;
            }
            if is_sun_spirit_collected(ctx, row.entity_id) == want_collected {
                count += 1;
            }
        }
        return count;
    }
    for cfg in &assets.tables.level_entity_config.items {
        if dungeon_id > 0 && cfg.MapId != dungeon_id {
            continue;
        }
        if !cfg.BlueprintType.contains("RiLing") && !cfg.BlueprintType.contains("SunSpirit") {
            continue;
        }
        if !sun_spirit_row_matches_area(
            assets,
            cfg.MapId,
            cfg.EntityId,
            cfg.AreaId,
            area_id,
            collect_only,
        ) {
            continue;
        }
        if is_sun_spirit_collected(ctx, cfg.EntityId) == want_collected {
            count += 1;
        }
    }
    count
}

fn current_map_hour_minute(ctx: &EvalContext<'_>) -> (i32, i32) {
    if ctx.quest_comp.info.env.time_locked {
        return (
            ctx.quest_comp.info.env.time_lock_hour,
            ctx.quest_comp.info.env.time_lock_minute,
        );
    }
    if let Some(scene) = ctx.scene {
        return (scene.instance.map_time.hour, scene.instance.map_time.minute);
    }
    (
        ctx.quest_comp.info.env.time_lock_hour,
        ctx.quest_comp.info.env.time_lock_minute,
    )
}

fn parse_json_vector_pos(val_opt: Option<&Value>) -> Option<[f32; 3]> {
    let v = val_opt?.as_object()?;
    let x = json_float_val(v.get("X"))?;
    let y = json_float_val(v.get("Y"))?;
    let z = json_float_val(v.get("Z")).unwrap_or(0.0);
    Some([x, y, z])
}

fn resolve_entity_world_pos(ctx: &EvalContext<'_>, cfg_id_raw: i64) -> Option<[f32; 3]> {
    if cfg_id_raw <= 0 {
        return None;
    }
    if let Some(scene) = ctx.scene {
        for ent in &scene.entities {
            if i64::from(ent.config.config_id) == cfg_id_raw
                && let Some(p) = ent.position
            {
                return Some(p.location);
            }
        }
    }
    if let Some(assets) = ctx.assets {
        let map_id = ctx.scene.map_or(8, |sc| sc.instance_id);
        if let Some(lcfg) = assets.find_level_entity(map_id, cfg_id_raw) {
            return asset_indexes::entity_world_pos(&lcfg);
        }
    }
    None
}

fn is_entity_target_invisible(ctx: &EvalContext<'_>, eid: i32) -> bool {
    if eid <= 0 {
        return false;
    }
    if let Some(scene) = ctx.scene
        && let Some(nid) = scene.find_entity_net_id_by_config_id(eid)
        && let Some(&idx) = scene.net_id_map.get(&nid)
    {
        return !scene.entities[idx].visible;
    }
    if ctx.quest_comp.info.world.hidden_entities.contains(&eid) {
        return true;
    }
    if ctx.quest_comp.info.world.shown_entities.contains(&eid) {
        return false;
    }
    if let Some(assets) = ctx.assets {
        let map_id = ctx.scene.map_or(8, |sc| sc.instance_id);
        if let Some(lcfg) = assets.find_level_entity(map_id, i64::from(eid)) {
            return lcfg.IsHidden || lcfg.InSleep;
        }
    }
    true
}

fn is_level_play_node_marked_finished(ctx: &EvalContext<'_>, nid: i32) -> bool {
    if nid <= 0 {
        return false;
    }
    let Some(pc) = ctx.progress_comp else {
        return false;
    };
    let suffix = format!(":{nid}");
    pc.info
        .dynamic
        .claims
        .iter()
        .any(|c| (c.starts_with("lp_done:") || c.starts_with("lp_fin:")) && c.ends_with(&suffix))
        || pc
            .info
            .dynamic
            .ints
            .iter()
            .any(|e| e.key.starts_with("lp_node_fin:") && e.key.ends_with(&suffix) && e.val != 0)
}

pub fn eval_condition_group_list(ctx: &EvalContext<'_>, list_val: &Value) -> bool {
    let Some(items) = list_val.as_array() else {
        return true;
    };
    if items.is_empty() {
        return true;
    }
    items.iter().any(|grp| eval_inline_condition_block(ctx, grp))
}

pub fn eval_entity_visible_component(ctx: &EvalContext<'_>, components: &Components) -> bool {
    let extra_ev = components.extra.iter().find_map(|entry| match entry {
        ExtraEntry::EntityVisibleComponent(ev) => Some(ev.as_ref()),
        _ => None,
    });
    let tpl_ev = components.template_extra.iter().find_map(|entry| match entry {
        ExtraEntry::EntityVisibleComponent(ev) => Some(ev.as_ref()),
        _ => None,
    });
    let Some(ev) = extra_ev.or(tpl_ev) else {
        return true;
    };
    if ev.Disabled.unwrap_or(false) {
        return true;
    }
    match &ev.VisibleConditions {
        Some(vc) => eval_condition_group_list(ctx, vc),
        None => true,
    }
}

fn is_neg_compare(cmp: &str) -> bool {
    cmp == "Ne" || cmp == "NotEq" || cmp == "!="
}

fn eval_time_period_match(ctx: &EvalContext<'_>, period: &str, cmp: &str) -> bool {
    let (hour, minute) = current_map_hour_minute(ctx);
    let total_minute = hour * 60 + minute;
    let is_day = (360..1080).contains(&total_minute);
    let matches = match period {
        "DayTime" => is_day,
        "Night" => !is_day,
        _ => return false,
    };
    if cmp == "Eq" {
        matches
    } else if is_neg_compare(cmp) {
        !matches
    } else {
        false
    }
}

fn entity_has_runtime_tag(ctx: &EvalContext<'_>, eid: i32, tid: i32) -> bool {
    if tid == 0 {
        return false;
    }
    if let Some(scene) = ctx.scene
        && let Some(nid) = scene.find_entity_net_id_by_config_id(eid)
        && let Some(&idx) = scene.net_id_map.get(&nid)
        && let Some(ent) = scene.entities.get(idx)
    {
        if let Some(tag) = &ent.tag {
            if let Some(assets) = ctx.assets
                && tag.has_tag(&assets.tables.gameplay_tag_parent, tid)
            {
                return true;
            }
            if tag.entity_common_tags.contains(&tid) {
                return true;
            }
        }
        if let Some(st) = &ent.state_tag
            && st.state_tag_id == tid
        {
            return true;
        }
    }
    ctx.progress_comp
        .is_some_and(|pc| pc.info.get_key_int(&format!("tag_ent:{eid}:{tid}")) > 0)
}

fn player_role_has_tag_name(ctx: &EvalContext<'_>, tag_name: &str) -> bool {
    let Some(scene) = ctx.scene else {
        return false;
    };
    let Some(assets) = ctx.assets else {
        return false;
    };
    let role_nid = scene.current_role_entity_id();
    if role_nid <= 0 {
        return false;
    }
    let Some(&idx) = scene.net_id_map.get(&role_nid) else {
        return false;
    };
    let Some(tag_comp) = scene.entities.get(idx).and_then(|e| e.tag.as_ref()) else {
        return false;
    };
    let Some(tag_id) = gameplay_tags::id_from_name(&assets.tables.gameplay_tag_remap, tag_name) else {
        return false;
    };
    tag_comp.has_tag(&assets.tables.gameplay_tag_parent, tag_id)
}

fn eval_player_state_restriction(ctx: &EvalContext<'_>, restriction_id: i32) -> bool {
    let in_skill = player_role_has_tag_name(ctx, "行为状态.逻辑状态.技能中");
    let in_battle = ctx
        .scene
        .is_some_and(|s| s.player_in_battle || player_role_has_tag_name(ctx, "行为状态.逻辑状态.进入战斗"));
    match restriction_id {
        1 => {
            (eval_player_motion_state(ctx, "Water").unwrap_or(false)
                || eval_player_motion_state(ctx, "WalkOnWater").unwrap_or(false))
                && !player_role_has_tag_name(ctx, "战斗状态.行为限制.禁止攀爬")
        }
        2 => !in_skill,
        3 => !player_role_has_tag_name(ctx, "怪物.common.状态标识.霸体"),
        900001 => eval_player_motion_state(ctx, "Ground").unwrap_or(true) && !in_skill,
        750001 | 850001 => !in_battle,
        150001 => !in_skill,
        _ => !in_skill,
    }
}

fn resolve_compare_lift_location(ctx: &EvalContext<'_>, eid: i32) -> i32 {
    if let Some(pc) = ctx.progress_comp {
        let lift_target = pc.info.get_key_int(&format!("lift_target:{eid}"));
        if lift_target > 0 {
            return lift_target.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        }
        let elevator = pc.info.get_key_int(&format!("elevator:{eid}"));
        if elevator > 0 {
            return elevator.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        }
    }
    if let Some(scene) = ctx.scene
        && let Some(nid) = scene.find_entity_net_id_by_config_id(eid)
        && let Some(ent) = scene.entity(nid)
        && let Some(lift) = ent.world_item.lift
        && lift.location > 0
    {
        return lift.location;
    }
    if let Some(assets) = ctx.assets {
        let inst_id = ctx
            .scene
            .map(|s| s.instance_id)
            .filter(|&id| id > 0)
            .unwrap_or(8);
        if let Some(lcfg) = assets.find_level_entity(inst_id, i64::from(eid)) {
            let mut comps = lcfg.ComponentsData.clone();
            if let Some(tpl) = assets.find_template_config(&lcfg.BlueprintType) {
                tpl.ComponentsData.merge_into(&mut comps);
            }
            if let Some(lc) = comps.LiftComponent
                && !lc.Disabled.unwrap_or(false)
            {
                return lc.InitialFloor.unwrap_or(1);
            }
        }
    }
    0
}

fn resolve_entity_world_rot(ctx: &EvalContext<'_>, cfg_id_raw: i64) -> [f32; 3] {
    if cfg_id_raw > 0 {
        if let Some(scene) = ctx.scene {
            for ent in &scene.entities {
                if i64::from(ent.config.config_id) == cfg_id_raw
                    && let Some(p) = ent.position
                {
                    return p.rotation;
                }
            }
        }
        if let Some(assets) = ctx.assets {
            let map_id = ctx.scene.map_or(8, |sc| sc.instance_id);
            if let Some(lcfg) = assets.find_level_entity(map_id, cfg_id_raw) {
                return asset_indexes::entity_world_rot(&lcfg);
            }
        }
    }
    [0.0, 0.0, 0.0]
}

fn eval_check_scene_item_direction(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(target_dir) = parse_json_vector_pos(obj.get("Direction")) else {
        return false;
    };
    let tol = json_float_val(obj.get("AngleInterval")).unwrap_or(30.0);
    let eid = obj
        .get("Target")
        .and_then(Value::as_object)
        .and_then(|tv| json_int(tv.get("EntityId")))
        .unwrap_or(ctx.self_entity_config_id);
    let rot = resolve_entity_world_rot(ctx, i64::from(eid));
    let yaw_rad = rot[2] * (std::f32::consts::PI / 180.0);
    let cur_dir = [yaw_rad.cos(), yaw_rad.sin(), 0.0];
    let tlen = (target_dir[0] * target_dir[0]
        + target_dir[1] * target_dir[1]
        + target_dir[2] * target_dir[2])
        .sqrt();
    if tlen < 0.0001 {
        return true;
    }
    let dot = ((cur_dir[0] * target_dir[0] + cur_dir[1] * target_dir[1] + cur_dir[2] * target_dir[2])
        / tlen)
        .clamp(-1.0, 1.0);
    let ang_deg = dot.acos() * (180.0 / std::f32::consts::PI);
    ang_deg <= tol
}

fn eval_check_player_relative_angle(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(player_pos) = current_player_pos_3d(ctx) else {
        return false;
    };
    let eid = json_int(obj.get("TargetEntityId"))
        .or_else(|| json_int(obj.get("EntityId")))
        .unwrap_or(ctx.self_entity_config_id);
    let Some(ent_pos) = resolve_entity_world_pos(ctx, i64::from(eid)) else {
        return true;
    };
    let ent_rot = resolve_entity_world_rot(ctx, i64::from(eid));
    let rel_world = [
        player_pos[0] - ent_pos[0],
        player_pos[1] - ent_pos[1],
        player_pos[2] - ent_pos[2],
    ];
    let bspace = get_param_str(obj, "BaseVectorSpace").unwrap_or("World");
    let rel = if bspace == "Local" {
        rotate_inverse_euler(rel_world, ent_rot)
    } else {
        rel_world
    };
    let rlen = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
    if rlen < 0.001 {
        return true;
    }
    let bvec = parse_json_vector_pos(obj.get("BaseVector")).unwrap_or([1.0, 0.0, 0.0]);
    let blen = (bvec[0] * bvec[0] + bvec[1] * bvec[1] + bvec[2] * bvec[2]).sqrt();
    if blen < 0.001 {
        return true;
    }
    let dot = ((rel[0] * bvec[0] + rel[1] * bvec[1] + rel[2] * bvec[2]) / (rlen * blen))
        .clamp(-1.0, 1.0);
    let ang_deg = dot.acos() * (180.0 / std::f32::consts::PI);
    if let Some(ranges) = obj.get("ValidAngleRanges").and_then(Value::as_array)
        && !ranges.is_empty()
    {
        return ranges.iter().any(|r| {
            r.as_object().is_some_and(|robj| {
                let min_a = json_float_val(robj.get("Min")).unwrap_or(0.0);
                let max_a = json_float_val(robj.get("Max")).unwrap_or(180.0);
                ang_deg >= min_a && ang_deg <= max_a
            })
        });
    }
    true
}

fn eval_player_skill_ready(ctx: &EvalContext<'_>, obj: &Map<String, Value>) -> bool {
    let Some(opt) = obj.get("SkillOption").and_then(Value::as_object) else {
        return false;
    };
    let Some(skill_type) = get_param_str(opt, "Type") else {
        return false;
    };
    let Some(scene) = ctx.scene else {
        return false;
    };
    let role_net_id = scene.current_role_entity_id();
    let Some(&idx) = scene.net_id_map.get(&role_net_id) else {
        return false;
    };
    let entity = &scene.entities[idx];
    let life_idx = EAttributeType::Life.to_i32() as usize;
    match skill_type {
        "UltimateSkill" => {
            let Some(attr) = entity.attribute.as_ref() else {
                return false;
            };
            if attr.attributes[life_idx].current <= 0 {
                return false;
            }
            let energy_idx = EAttributeType::Energy.to_i32() as usize;
            let max_idx = EAttributeType::EnergyMax.to_i32() as usize;
            let cur = attr.attributes[energy_idx].current;
            let max_val = attr.attributes[max_idx].current;
            if max_val > 0 { cur >= max_val } else { cur > 0 }
        }
        "ESkill" => {
            if let Some(attr) = entity.attribute.as_ref()
                && attr.attributes[life_idx].current <= 0
            {
                return false;
            }
            if let Some(pc) = ctx.progress_comp
                && pc.info.get_key_int(&format!("skill_cd:e:{role_net_id}")) > 0
            {
                return false;
            }
            true
        }
        "VisionSkill" => {
            if let Some(attr) = entity.attribute.as_ref()
                && attr.attributes[life_idx].current <= 0
            {
                return false;
            }
            if let Some(vs) = entity.vision_skills.as_ref()
                && !vs.vision_skills.is_empty()
            {
                return true;
            }
            scene.explore_tools_info.active_explore_skill != 0 || !ctx.echo_comp.echo_map.is_empty()
        }
        _ => false,
    }
}

fn get_param_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key)?.as_str()
}

fn get_param_int(obj: &Map<String, Value>, key: &str) -> Option<i32> {
    get_param_str(obj, key)?.parse::<i32>().ok()
}

fn json_int(v_opt: Option<&Value>) -> Option<i32> {
    match v_opt? {
        Value::Number(n) => n
            .as_i64()
            .and_then(|i| i32::try_from(i).ok())
            .or_else(|| {
                n.as_f64().and_then(|f| {
                    if f.is_finite() && f >= f64::from(i32::MIN) && f <= f64::from(i32::MAX) {
                        Some(f as i32)
                    } else {
                        None
                    }
                })
            }),
        Value::String(s) => s.parse::<i32>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonoro_gamesv_fs::role_info::Reson;

    #[test]
    fn role_meets_threshold_checks_breach_level_favor_and_resonance() {
        let role = RoleInfo {
            level: 60,
            breakthrough: 3,
            favor: 4,
            resonant_chain_group_index: 2,
            reson: vec![
                Reson {
                    id: 1,
                    open: true,
                    increase: 1,
                },
                Reson {
                    id: 2,
                    open: true,
                    increase: 1,
                },
                Reson {
                    id: 3,
                    open: false,
                    increase: 0,
                },
            ],
            ..Default::default()
        };
        assert!(role_meets_threshold(&role, RoleThresholdField::Level, 60));
        assert!(!role_meets_threshold(&role, RoleThresholdField::Level, 70));
        assert!(role_meets_threshold(&role, RoleThresholdField::Breach, 3));
        assert!(role_meets_threshold(&role, RoleThresholdField::Favor, 4));
        assert!(!role_meets_threshold(&role, RoleThresholdField::Favor, 5));
        assert!(role_meets_threshold(&role, RoleThresholdField::Resonance, 2));
        assert!(!role_meets_threshold(&role, RoleThresholdField::Resonance, 3));
    }

    #[test]
    fn compare_ints_handles_symbol_and_keyword_comparison_operators_uniformly() {
        assert!(compare_ints(0, 2, "<"));
        assert!(!compare_ints(2, 2, "<"));
        assert!(compare_ints(2, 2, "<="));
        assert!(compare_ints(5, 3, ">"));
        assert!(compare_ints(3, 3, "GtEq"));
        assert!(compare_ints(1, 2, "!="));
        assert!(compare_ints(2, 2, "=="));
    }

    #[test]
    fn eval_condition_record_evaluates_direct_3_8_11_types_and_server_cond_specs() {
        let mut basic = PlayerBasicComponent {
            player_id: 1,
            ..Default::default()
        };
        basic.info.attributes.level = 55;
        basic.info.attributes.origin_world_level = 6;
        basic.info.attributes.cur_world_level = 5;

        let echo = PlayerEchoComponent::default();
        let inv = PlayerInventoryComponent::default();
        let map = PlayerMapComponent::default();
        let mut quest = PlayerQuestComponent::default();
        quest.info.set_var_str("route", "B");

        let mut prog = PlayerProgressComponent::default();
        prog.info.mark_claim("abyss:pass:101");
        prog.info.mark_claim("rover_rogue:talent:7");
        prog.info.set_key_int("pb:challenge:9", 1);

        let mut ctx = EvalContext::new(&basic, &echo, &inv, &map, &quest);
        ctx.progress_comp = Some(&prog);

        let p_lv: Value = serde_json::from_str(r#"{"Level":"50","Op":"Ge"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 1,
                Type: "CheckLevelOp".to_string(),
                LimitParams: Some(p_lv),
                ..Default::default()
            }
        ));

        let p_lv_fail: Value = serde_json::from_str(r#"{"Level":"60","Op":"Ge"}"#).unwrap();
        assert!(!eval_condition_record(
            &ctx,
            &Condition {
                Id: 2,
                Type: "CheckLevelOp".to_string(),
                LimitParams: Some(p_lv_fail),
                ..Default::default()
            }
        ));

        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 3,
                Type: "ReduceWorldLevel".to_string(),
                ..Default::default()
            }
        ));
        basic.info.attributes.cur_world_level = 6;
        let ctx = EvalContext {
            basic_comp: &basic,
            echo_comp: &echo,
            inventory_comp: &inv,
            map_comp: &map,
            quest_comp: &quest,
            progress_comp: Some(&prog),
            ..EvalContext::new(&basic, &echo, &inv, &map, &quest)
        };
        assert!(!eval_condition_record(
            &ctx,
            &Condition {
                Id: 4,
                Type: "ReduceWorldLevel".to_string(),
                ..Default::default()
            }
        ));

        let p_vd: Value = serde_json::from_str(r#"{"VDName":"route","VDValueString":"B"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 5,
                Type: "VarDefineString".to_string(),
                LimitParams: Some(p_vd),
                ..Default::default()
            }
        ));

        let p_abyss: Value = serde_json::from_str(r#"{"Id":"101"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 6,
                Type: "AbyssPass".to_string(),
                LimitParams: Some(p_abyss),
                ..Default::default()
            }
        ));

        let p_rr: Value = serde_json::from_str(r#"{"TalentId":"7"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 7,
                Type: "RoverRogueTalentUnlock".to_string(),
                LimitParams: Some(p_rr),
                ..Default::default()
            }
        ));

        let p_pb: Value = serde_json::from_str(r#"{"ChallengeId":"9"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 8,
                Type: "PhantomBattleCompleteChallenge".to_string(),
                LimitParams: Some(p_pb),
                ..Default::default()
            }
        ));

        let p_miss: Value = serde_json::from_str(r#"{"Id":"999"}"#).unwrap();
        assert!(!eval_condition_record(
            &ctx,
            &Condition {
                Id: 9,
                Type: "AbyssPass".to_string(),
                LimitParams: Some(p_miss),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn eval_condition_record_handles_15_level_play_condition_types() {
        let basic = PlayerBasicComponent::default();
        let echo = PlayerEchoComponent::default();
        let inv = PlayerInventoryComponent::default();
        let map = PlayerMapComponent::default();
        let quest = PlayerQuestComponent::default();

        let mut prog = PlayerProgressComponent::default();
        for (k, v) in [
            ("actor_var:done", 1),
            ("simple_combat:kills", 1),
            ("flipper:fin", 1),
            ("kurotato:wave", 1),
            ("role_tower:stage", 1),
            ("rb_blk:fin", 1),
            ("surv:wave", 1),
            ("tetris:best", 500),
            ("trap:pass_wave", 1),
            ("handin_pm:cnt", 1),
            ("parallax:done", 1),
            ("timeline:done", 1),
            ("explore:photo_cnt", 1),
            ("walk_pat:done", 1),
        ] {
            prog.info.set_key_int(k, v);
        }

        let mut ctx = EvalContext::new(&basic, &echo, &inv, &map, &quest);
        ctx.progress_comp = Some(&prog);

        for ctype in [
            "CompareActorVar",
            "CrossSceneSyncKillMonster",
            "FinishFlipper",
            "FinishKuroSimpleCombatKillMonster",
            "FinishKurotato",
            "FinishRoleTower",
            "FinishRollBlock",
            "FinishSurvivorsRouge",
            "FinishTetris",
            "FinishTrapDefense",
            "NewHandInItems",
            "ParallaxAlign",
            "PlaybackShowTimeline",
            "TakePicturesWithTimeScale",
            "WalkingPattern",
        ] {
            assert!(
                eval_condition_record(
                    &ctx,
                    &Condition {
                        Id: 1,
                        Type: ctype.to_string(),
                        NeedNum: 1,
                        ..Default::default()
                    }
                ),
                "expected condition {ctype} to pass"
            );
        }
    }

    #[test]
    fn eval_condition_record_handles_3_8_11_dungeon_tower_and_activity_conditions() {
        let basic = PlayerBasicComponent::default();
        let echo = PlayerEchoComponent::default();
        let inv = PlayerInventoryComponent::default();
        let map = PlayerMapComponent::default();
        let mut quest = PlayerQuestComponent::default();
        quest.info.mark_dungeon_completed(9101);

        let mut prog = PlayerProgressComponent::default();
        prog.info.mark_claim("role_tower:stage:3");
        prog.info.mark_claim("pb:challenge:12");
        prog.info.mark_claim("kurotato:done:4");
        prog.info.set_key_int("babel:star:2", 9);
        prog.info.set_key_int("cond_cnt:CiacconaChapterResult", 1);

        let mut ctx = EvalContext::new(&basic, &echo, &inv, &map, &quest);
        ctx.progress_comp = Some(&prog);

        let p_dung_ok: Value = serde_json::from_str(r#"{"InstanceId":"9101","Finish":"1"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 10,
                Type: "CheckDungeonFinished".to_string(),
                LimitParams: Some(p_dung_ok),
                ..Default::default()
            }
        ));

        let p_dung_fail: Value = serde_json::from_str(r#"{"InstanceId":"9102","Finish":"1"}"#).unwrap();
        assert!(!eval_condition_record(
            &ctx,
            &Condition {
                Id: 11,
                Type: "NewCheckInstanceState".to_string(),
                LimitParams: Some(p_dung_fail),
                ..Default::default()
            }
        ));

        let p_rt: Value = serde_json::from_str(r#"{"Id":"3"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 12,
                Type: "RoleTowerPassStage".to_string(),
                LimitParams: Some(p_rt),
                ..Default::default()
            }
        ));

        let p_babel: Value = serde_json::from_str(r#"{"Id":"2"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 13,
                Type: "BabelTowerLevelFinishStar".to_string(),
                LimitParams: Some(p_babel),
                NeedNum: 9,
                ..Default::default()
            }
        ));

        let p_kuro: Value = serde_json::from_str(r#"{"Level":"4"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 14,
                Type: "KurotatoFinishLevel".to_string(),
                LimitParams: Some(p_kuro),
                ..Default::default()
            }
        ));

        let p_pb: Value = serde_json::from_str(r#"{"ChallengeId":"12"}"#).unwrap();
        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 15,
                Type: "PhantomBattleCompleteChallenge".to_string(),
                LimitParams: Some(p_pb),
                ..Default::default()
            }
        ));

        assert!(eval_condition_record(
            &ctx,
            &Condition {
                Id: 16,
                Type: "CiacconaChapterResult".to_string(),
                NeedNum: 1,
                ..Default::default()
            }
        ));

        let scene_no_rb = Scene::from_parts(
            1,
            8,
            sonoro_gamesv_fs::scene_instance::SceneInstance::default(),
            sonoro_gamesv_fs::formation_info::FormationInfo::default(),
            sonoro_gamesv_fs::explore_tools_info::ExploreToolsInfo::default(),
        );
        let mut scene_rb = Scene::from_parts(
            1,
            8,
            sonoro_gamesv_fs::scene_instance::SceneInstance::default(),
            sonoro_gamesv_fs::formation_info::FormationInfo::default(),
            sonoro_gamesv_fs::explore_tools_info::ExploreToolsInfo::default(),
        );
        scene_rb.last_server_rollback_quest_id = 880_000_044;
        scene_rb.instance.players.push(sonoro_gamesv_fs::scene_instance::ScenePlayer {
            id: 1,
            group_type: 1,
            location: [1000.0, 2000.0, 300.0],
            rotation: [0.0, 0.0, 0.0],
        });

        let mut ctx_first = EvalContext::new(&basic, &echo, &inv, &map, &quest);
        ctx_first.owner_quest_id = 880_000_044;
        ctx_first.scene = Some(&scene_no_rb);
        let p_first: Value = serde_json::from_str(
            r#"{"Type":"SaveConfigEnterMethod","EnterMethod":"FirstTime","Compare":"Eq"}"#,
        )
        .unwrap();
        let p_rb: Value = serde_json::from_str(
            r#"{"Type":"SaveConfigEnterMethod","EnterMethod":"FailureRollBack","Compare":"Eq"}"#,
        )
        .unwrap();
        assert!(eval_single_inline_condition(&ctx_first, &p_first));
        assert!(!eval_single_inline_condition(&ctx_first, &p_rb));

        let mut ctx_rb = EvalContext::new(&basic, &echo, &inv, &map, &quest);
        ctx_rb.owner_quest_id = 880_000_044;
        ctx_rb.scene = Some(&scene_rb);
        assert!(!eval_single_inline_condition(&ctx_rb, &p_first));
        assert!(eval_single_inline_condition(&ctx_rb, &p_rb));

        let p_pos_in: Value = serde_json::from_str(
            r#"{"Type":"CheckPlayerPosition","Range":{"Type":"CircleRange","Pos":{"X":1000.0,"Y":2000.0,"Z":300.0},"Radius":500},"IsOnRange":true}"#,
        )
        .unwrap();
        let p_pos_out: Value = serde_json::from_str(
            r#"{"Type":"CheckPlayerPosition","Range":{"Type":"CircleRange","Pos":{"X":9000.0,"Y":9000.0,"Z":300.0},"Radius":100},"IsOnRange":true}"#,
        )
        .unwrap();
        let p_pos_not_in: Value = serde_json::from_str(
            r#"{"Type":"CheckPlayerPosition","Range":{"Type":"BoxRange","Pos":{"X":9000.0,"Y":9000.0,"Z":300.0},"Size":{"X":200,"Y":200,"Z":200}},"IsOnRange":false}"#,
        )
        .unwrap();
        assert!(eval_single_inline_condition(&ctx_rb, &p_pos_in));
        assert!(!eval_single_inline_condition(&ctx_rb, &p_pos_out));
        assert!(eval_single_inline_condition(&ctx_rb, &p_pos_not_in));
    }
}
