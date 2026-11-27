use std::collections::BTreeMap;

use sonoro_gamesv_data::zon_config::GameConfig;
use sonoro_gamesv_fs::guide_info::GuideInfo;
use sonoro_gamesv_logic::component::comp_util;
use sonoro_gamesv_logic::component::player::PlayerCosmeticComponent;
use sonoro_gamesv_logic::event_queue::EventQueue;
use sonoro_gamesv_logic::helpers::{calabash, energy, quest_engine, reward};
use sonoro_proto::enums::{ErrorCode, IllustratedType};
use sonoro_proto::pb::{
    GuideFinishRequest, GuideFinishResponse, GuideInfoRequest, GuideInfoResponse,
    GuideTriggerRequest, GuideTriggerResponse, IllustratedClass, IllustratedEntry,
    IllustratedInfoRequest, IllustratedInfoResponse, IllustratedReadRequest,
    IllustratedReadResponse, IllustratedRedDotRequest, IllustratedRedDotResponse,
    RoleHandbookInfo, RoleIllustratedInfoRequest, RoleIllustratedInfoResponse, TutorialInfo,
    TutorialInfoRequest, TutorialInfoResponse, TutorialReceiveRequest, TutorialReceiveResponse,
    TutorialUnlockRequest, TutorialUnlockResponse, WeaponHandbookInfo,
};
use sonoro_proto::wire::PbEnum;

use super::State;

const FINISHED_BY_DEFAULT: &[i32] = &[10116, 10193, 10194, 10120, 10175, 10270, 10285];
const DEFAULT_TUTORIAL_REWARD_ASTRITE: i32 = 1;
const TUTORIAL_DROP_PACKAGE_ID: i32 = 100001;

fn save_guide_info(state: &State) -> Result<(), String> {
    let path = format!("player/{}/{}", state.player_id, GuideInfo::DATA_PATH);
    comp_util::save_struct(&state.fs, &state.player_components.guide.info, &path)
        .map_err(|e| e.to_string())
}

pub fn handle_guide_info_request(
    state: &mut State,
    _req: GuideInfoRequest,
) -> Result<GuideInfoResponse, String> {
    let mut finished = Vec::new();
    if GameConfig::embedded().autocomplete_guides {
        finished.extend(
            state
                .assets
                .tables
                .guide_group
                .items
                .iter()
                .map(|g| g.Id),
        );
    } else {
        for &group_id in FINISHED_BY_DEFAULT {
            if !state.player_components.guide.info.has_finished(group_id) {
                finished.push(group_id);
            }
        }
        finished.extend_from_slice(&state.player_components.guide.info.finished_groups);
    }
    Ok(GuideInfoResponse {
        GuideGroupFinishList: finished,
    })
}

pub fn handle_guide_trigger_request(
    state: &mut State,
    events: &mut EventQueue,
    req: GuideTriggerRequest,
) -> Result<GuideTriggerResponse, String> {
    if req.GroupId > 0 && GameConfig::embedded().autocomplete_guides {
        let mut qctx = state.quest_context(events);
        quest_engine::triggers::on_guide_finish(&mut qctx, req.GroupId)
            .map_err(|e| e.to_string())?;
    }
    Ok(GuideTriggerResponse {
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_guide_finish_request(
    state: &mut State,
    events: &mut EventQueue,
    req: GuideFinishRequest,
) -> Result<GuideFinishResponse, String> {
    state
        .player_components
        .guide
        .info
        .add_finished(req.GroupId);
    save_guide_info(state)?;
    let mut qctx = state.quest_context(events);
    quest_engine::triggers::on_guide_finish(&mut qctx, req.GroupId)
        .map_err(|e| e.to_string())?;
    Ok(GuideFinishResponse {
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_illustrated_info_request(
    state: &mut State,
    req: IllustratedInfoRequest,
) -> Result<IllustratedInfoResponse, String> {
    let mut classes = Vec::new();
    let cosmetic = &state.player_components.cosmetic;
    let echo = &state.player_components.echo;
    let weapon = &state.player_components.weapon;

    for illustrated_type in req.TypeList {
        let type_int = illustrated_type as i32;
        let mut entries: Vec<IllustratedEntry> = Vec::new();

        if illustrated_type == IllustratedType::VocalCorpse {
            for reward_cfg in &state.assets.tables.calabash_develop_reward.items {
                if !reward_cfg.IsShow {
                    continue;
                }
                let max_q = calabash::discovered_max_quality(&state.assets, echo, reward_cfg.MonsterId);
                let saved = cosmetic.info.find_illustrated(type_int, reward_cfg.MonsterId);
                if max_q <= 0 && saved.is_none() {
                    continue;
                }
                let mut unlocked_conds = 0i32;
                for (idx, &cond_id) in reward_cfg.DevelopCondition.iter().enumerate() {
                    if max_q >= calabash::required_quality_for_condition(&state.assets, cond_id, idx)
                    {
                        unlocked_conds += 1;
                    }
                }
                let min_num = if max_q > 0 {
                    unlocked_conds.max(1)
                } else {
                    1
                };
                entries.push(IllustratedEntry {
                    Id: reward_cfg.MonsterId,
                    CreateTime: saved.as_ref().map_or(1, |r| r.create_time),
                    Num: saved.as_ref().map_or(min_num, |r| r.num.max(min_num)),
                    IsRead: saved.as_ref().is_some_and(|r| r.is_read),
                    ..Default::default()
                });
            }
        } else if illustrated_type == IllustratedType::Weapon {
            for w in weapon.weapon_map.values() {
                if entries.iter().any(|e| e.Id == w.id) {
                    continue;
                }
                let copies = weapon
                    .weapon_map
                    .values()
                    .filter(|other| other.id == w.id)
                    .count() as i32;
                let saved = cosmetic.info.find_illustrated(type_int, w.id);
                entries.push(IllustratedEntry {
                    Id: w.id,
                    CreateTime: saved.as_ref().map_or(1, |r| r.create_time),
                    Num: saved.as_ref().map_or(copies, |r| r.num.max(copies)),
                    IsRead: saved.as_ref().is_none_or(|r| r.is_read),
                    ..Default::default()
                });
            }
        }

        for rec in &cosmetic.info.illustrated_entries {
            if rec.entry_type != type_int || entries.iter().any(|e| e.Id == rec.id) {
                continue;
            }
            entries.push(IllustratedEntry {
                Id: rec.id,
                CreateTime: rec.create_time,
                Num: rec.num,
                IsRead: rec.is_read,
                ..Default::default()
            });
        }

        classes.push(IllustratedClass {
            Type: Some(illustrated_type),
            IllustratedEntryList: entries,
        });
    }

    Ok(IllustratedInfoResponse {
        ErrorCode: Some(ErrorCode::Success),
        IllustratedClassList: classes,
        ..Default::default()
    })
}

pub fn handle_illustrated_red_dot_request(
    state: &mut State,
    _req: IllustratedRedDotRequest,
) -> Result<IllustratedRedDotResponse, String> {
    let mut red_dots = Vec::new();
    for rec in &state.player_components.cosmetic.info.illustrated_entries {
        if rec.is_read {
            continue;
        }
        let itype = IllustratedType::from_i32_or_default(rec.entry_type);
        if (itype as i32) == rec.entry_type && !red_dots.contains(&itype) {
            red_dots.push(itype);
        }
    }
    Ok(IllustratedRedDotResponse {
        RedDotList: red_dots,
    })
}

pub fn handle_illustrated_read_request(
    state: &mut State,
    req: IllustratedReadRequest,
) -> Result<IllustratedReadResponse, String> {
    if let Some(entry_type) = req.Type
        && state
            .player_components
            .cosmetic
            .info
            .mark_illustrated_read(entry_type as i32, req.Id)
    {
        PlayerCosmeticComponent::save(
            &state.fs,
            state.player_id,
            &state.player_components.cosmetic.info,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(IllustratedReadResponse {
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_role_illustrated_info_request(
    state: &mut State,
    _req: RoleIllustratedInfoRequest,
) -> Result<RoleIllustratedInfoResponse, String> {
    let mut roles = Vec::new();
    for (&role_id, role) in &state.player_components.role.role_map {
        if state.player_components.role.is_trial_role(role_id) || !(1000..2000).contains(&role_id)
        {
            continue;
        }
        roles.push(RoleHandbookInfo {
            RoleId: role_id,
            UnlockTime: if role.create_time > 0 {
                i64::from(role.create_time)
            } else {
                1
            },
        });
    }

    let mut weapons: Vec<WeaponHandbookInfo> = Vec::new();
    for w in state.player_components.weapon.weapon_map.values() {
        if weapons.iter().any(|existing| existing.WeaponId == w.id) {
            continue;
        }
        weapons.push(WeaponHandbookInfo {
            WeaponId: w.id,
            UnlockTime: 1,
        });
    }

    Ok(RoleIllustratedInfoResponse {
        Roles: roles,
        Weapons: weapons,
    })
}

pub fn handle_tutorial_info_request(
    state: &mut State,
    _req: TutorialInfoRequest,
) -> Result<TutorialInfoResponse, String> {
    let list = state
        .player_components
        .guide
        .info
        .unlocked_tutorials
        .iter()
        .map(|rec| TutorialInfo {
            Id: rec.id,
            CreateTime: rec.create_time,
            GetAward: rec.get_award,
        })
        .collect();
    Ok(TutorialInfoResponse { UnlockList: list })
}

pub fn handle_tutorial_unlock_request(
    state: &mut State,
    req: TutorialUnlockRequest,
) -> Result<TutorialUnlockResponse, String> {
    let create_time = energy::now_seconds().clamp(1, i64::from(u32::MAX)) as u32;
    let rec = state
        .player_components
        .guide
        .info
        .unlock_tutorial(req.Id, create_time);
    save_guide_info(state)?;
    Ok(TutorialUnlockResponse {
        UnLockInfo: Some(TutorialInfo {
            Id: rec.id,
            CreateTime: rec.create_time,
            GetAward: rec.get_award,
        }),
        ErrorCode: Some(ErrorCode::Success),
        ..Default::default()
    })
}

pub fn handle_tutorial_receive_request(
    state: &mut State,
    req: TutorialReceiveRequest,
) -> Result<TutorialReceiveResponse, String> {
    let tutorial_id = req.Id;
    let mut item_map = BTreeMap::new();
    if tutorial_id <= 0 {
        return Ok(TutorialReceiveResponse {
            ErrorCode: Some(ErrorCode::Success),
            ItemMap: item_map,
            ..Default::default()
        });
    }

    let create_time = energy::now_seconds().clamp(1, i64::from(u32::MAX)) as u32;
    if state
        .player_components
        .guide
        .info
        .find_tutorial_mut(tutorial_id)
        .is_none()
    {
        state
            .player_components
            .guide
            .info
            .unlock_tutorial(tutorial_id, create_time);
    }

    let should_grant = {
        let Some(rec) = state
            .player_components
            .guide
            .info
            .find_tutorial_mut(tutorial_id)
        else {
            return Ok(TutorialReceiveResponse {
                ErrorCode: Some(ErrorCode::Success),
                ItemMap: item_map,
                ..Default::default()
            });
        };
        if rec.get_award {
            false
        } else {
            rec.get_award = true;
            true
        }
    };

    if should_grant {
        save_guide_info(state)?;
        let mut rctx = state.reward_context();
        let _ = reward::grant_items(
            &mut rctx,
            TUTORIAL_DROP_PACKAGE_ID,
            false,
            &[reward::RewardItem {
                item_id: 3,
                count: DEFAULT_TUTORIAL_REWARD_ASTRITE,
            }],
            1,
        )
        .map_err(|e| e.to_string())?;
        item_map.insert(3, DEFAULT_TUTORIAL_REWARD_ASTRITE);
    }

    Ok(TutorialReceiveResponse {
        ErrorCode: Some(ErrorCode::Success),
        ItemMap: item_map,
        ..Default::default()
    })
}
