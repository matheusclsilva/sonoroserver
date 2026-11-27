use std::collections::BTreeMap;
use std::io;

use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_fs::echo_info;
use sonoro_gamesv_fs::inventory_info::InventoryInfo;
use sonoro_gamesv_fs::special_item_incr;
use sonoro_gamesv_fs::weapon_item::WeaponItem;
use sonoro_proto::enums::{PlayerAttrKey, PlayerAttrType};
use sonoro_proto::pb::{
    CalabashMsgNotify, ItemRewardNotify, NormalItem, NormalItemAddNotify, NormalItemUpdateNotify,
    PhantomItemAddNotify, PlayerAttr, PlayerAttrNotify, PlayerAttrValueOneof, RewardItemInfo,
    RewardItemInfoList, WeaponItemAddNotify,
};

use crate::component::comp_util;
use crate::component::player::{
    PlayerBasicComponent, PlayerEchoComponent, PlayerInventoryComponent, PlayerProgressComponent,
    PlayerWeaponComponent,
};
use crate::helpers::{calabash, energy, morale};
use crate::sink::PushSink;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewardItem {
    pub item_id: i32,
    pub count: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrantResult {
    pub leveled_up: bool,
    pub new_level: i32,
    pub granted_count: usize,
}

pub struct RewardContext<'a, S: PushSink + ?Sized> {
    pub fs: &'a FileSystem,
    pub assets: &'a Assets,
    pub sink: &'a mut S,
    pub basic_comp: &'a mut PlayerBasicComponent,
    pub inventory_comp: &'a mut PlayerInventoryComponent,
    pub weapon_comp: &'a mut PlayerWeaponComponent,
    pub echo_comp: &'a mut PlayerEchoComponent,
    pub progress_comp: Option<&'a mut PlayerProgressComponent>,
}

pub fn grant_drop_package<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    drop_id: i32,
    reason: i32,
) -> io::Result<GrantResult> {
    grant_drop_package_internal(ctx, drop_id, reason, None, false)
}

pub fn grant_drop_package_popup<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    drop_id: i32,
    reason: i32,
) -> io::Result<GrantResult> {
    grant_drop_package_internal(ctx, drop_id, reason, Some(true), false)
}

/// A drop the player takes from the world: a gathered plant, a collected item or a broken
/// rock or crate. These packages have `ShowBg` false, so the client's
/// `ItemHintController.HandleItemRewardNotify` shows nothing for them, and the item hint
/// comes only from `NormalItemAddNotify` / `NormalItemUpdateNotify` with `NoTips` false
/// (`InventoryController`). The popup still follows `ShowBg`.
pub fn grant_world_pickup_drop_package<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    drop_id: i32,
    reason: i32,
) -> io::Result<GrantResult> {
    grant_drop_package_internal(ctx, drop_id, reason, None, true)
}

fn chest_drop_override_items(drop_id: i32) -> Option<&'static [RewardItem]> {
    const BASIC: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 5 },
        RewardItem { item_id: 1, count: 10 },
        RewardItem { item_id: 2, count: 1000 },
        RewardItem { item_id: 43_010_001, count: 2 },
        RewardItem { item_id: 43_020_001, count: 2 },
    ];
    const STANDARD: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 10 },
        RewardItem { item_id: 1, count: 20 },
        RewardItem { item_id: 2, count: 2000 },
        RewardItem { item_id: 43_010_002, count: 2 },
        RewardItem { item_id: 43_020_002, count: 2 },
        RewardItem { item_id: 41_100_021, count: 1 },
    ];
    const OVERFLOWING: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 20 },
        RewardItem { item_id: 1, count: 30 },
        RewardItem { item_id: 2, count: 4000 },
        RewardItem { item_id: 43_010_002, count: 3 },
        RewardItem { item_id: 43_020_002, count: 3 },
        RewardItem { item_id: 41_100_031, count: 1 },
    ];
    const PREMIUM: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 40 },
        RewardItem { item_id: 1, count: 40 },
        RewardItem { item_id: 2, count: 8000 },
        RewardItem { item_id: 43_010_003, count: 2 },
        RewardItem { item_id: 43_020_003, count: 2 },
        RewardItem { item_id: 41_100_031, count: 2 },
    ];
    const TIDE_CHEST: &[RewardItem] = &[
        RewardItem { item_id: 50_001, count: 1 },
        RewardItem { item_id: 3, count: 20 },
        RewardItem { item_id: 1, count: 30 },
        RewardItem { item_id: 2, count: 4000 },
        RewardItem { item_id: 43_010_002, count: 3 },
    ];
    const PHANTOM_GREEN: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 10 },
        RewardItem { item_id: 1, count: 20 },
        RewardItem { item_id: 2, count: 2000 },
        RewardItem { item_id: 36_000_002, count: 2 },
    ];
    const PHANTOM_PURPLE: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 20 },
        RewardItem { item_id: 1, count: 30 },
        RewardItem { item_id: 2, count: 4000 },
        RewardItem { item_id: 36_000_003, count: 2 },
    ];
    const PHANTOM_GOLD: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 40 },
        RewardItem { item_id: 1, count: 40 },
        RewardItem { item_id: 2, count: 8000 },
        RewardItem { item_id: 36_000_003, count: 4 },
    ];
    const SUPPLY_LOW: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 20 },
        RewardItem { item_id: 1, count: 30 },
        RewardItem { item_id: 2, count: 6000 },
        RewardItem { item_id: 43_010_002, count: 3 },
    ];
    const SUPPLY_HIGH: &[RewardItem] = &[
        RewardItem { item_id: 3, count: 40 },
        RewardItem { item_id: 1, count: 60 },
        RewardItem { item_id: 2, count: 12000 },
        RewardItem { item_id: 43_010_002, count: 4 },
        RewardItem { item_id: 43_020_002, count: 3 },
    ];

    match drop_id {
        1001 | 1011 | 1091 | 1101 | 1301 | 1314..=1317 | 1330 => Some(BASIC),
        1002 | 1012 | 1092 | 1102 | 1302 | 1318..=1321 | 1331 => Some(STANDARD),
        1003 | 1013 | 1093 | 1303 | 1322..=1325 | 1332 => Some(OVERFLOWING),
        1004 | 1014 | 1094 | 1304 | 1326..=1329 | 1333 | 45143 => Some(PREMIUM),
        1308 | 1334 => Some(TIDE_CHEST),
        1305 => Some(PHANTOM_GREEN),
        1306 => Some(PHANTOM_PURPLE),
        1307 => Some(PHANTOM_GOLD),
        2201..=2203 | 2205..=2212 => Some(SUPPLY_LOW),
        2204 | 2213 => Some(SUPPLY_HIGH),
        5_120_601..=5_120_690 | 5_200_101 => Some(STANDARD),
        20_400_001..=20_400_057 | 110_153 | 110_159 => Some(BASIC),
        _ => None,
    }
}

fn preview_item_count(drop_id: i32, item_id: i32, raw_count: i32) -> i32 {
    if raw_count > 0 {
        return raw_count;
    }
    let tier = match drop_id % 10 {
        2 | 6 => 2,
        3 | 7 => 3,
        4 | 8 => 4,
        _ => 1,
    };
    match item_id {
        1 => tier * 10,
        2 => match tier {
            1 => 1000,
            2 => 2000,
            3 => 4000,
            _ => 8000,
        },
        3 => match tier {
            1 => 5,
            2 => 10,
            3 => 20,
            _ => 40,
        },
        43_010_001..=43_029_999 | 36_000_001..=36_000_999 => (tier + 1).min(4),
        _ => 1,
    }
}

pub fn preview_drop_items(assets: &Assets, drop_id: i32) -> Vec<RewardItem> {
    if drop_id <= 0 {
        return Vec::new();
    }
    if let Some(items) = chest_drop_override_items(drop_id) {
        return items.to_vec();
    }
    let Some(pkg) = assets.tables.drop_package.get_data_by_id(&drop_id) else {
        return Vec::new();
    };
    let mut grants = Vec::new();
    for (&item_id, &raw_count) in pkg.DropPreview.iter() {
        if item_id <= 0 {
            continue;
        }
        grants.push(RewardItem {
            item_id,
            count: preview_item_count(drop_id, item_id, raw_count),
        });
    }
    grants
}

fn grant_drop_package_internal<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    drop_id: i32,
    reason: i32,
    force_popup: Option<bool>,
    item_tips: bool,
) -> io::Result<GrantResult> {
    if drop_id <= 0 {
        return Ok(GrantResult::default());
    }
    let grants = preview_drop_items(ctx.assets, drop_id);
    if grants.is_empty() {
        tracing::debug!("DropPackage {drop_id} has no preview/override items, skipping");
        return Ok(GrantResult::default());
    }
    let show_bg = ctx
        .assets
        .tables
        .drop_package
        .get_data_by_id(&drop_id)
        .is_none_or(|pkg| pkg.ShowBg);
    let show_popup = force_popup.unwrap_or(show_bg);
    grant_items_with_tips(
        ctx,
        drop_id,
        show_popup,
        show_popup || item_tips,
        &grants,
        reason,
    )
}

pub fn grant_treasure_box_reward<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    _chest_type_id: i32,
    drop_id: i32,
    reason: i32,
) -> io::Result<GrantResult> {
    let effective_reason = if reason <= 2 { 7000 } else { reason };
    if drop_id > 0 {
        let pkg_res = grant_drop_package_popup(ctx, drop_id, effective_reason)?;
        if pkg_res.granted_count > 0 {
            return Ok(pkg_res);
        }
    }

    let items = chest_drop_override_items(1002).expect("standard chest override");
    grant_items(
        ctx,
        if drop_id > 0 { drop_id } else { 1002 },
        true,
        items,
        effective_reason,
    )
}

fn resolve_phantom_drop_item_id(
    assets: &Assets,
    vision_capture_id: i32,
    calabash_level: i32,
) -> Option<i32> {
    if vision_capture_id <= 0 {
        return None;
    }
    let max_quality = if calabash_level >= 15 {
        5
    } else if calabash_level >= 9 {
        4
    } else if calabash_level >= 5 {
        3
    } else {
        2
    };

    for q in (2..=max_quality).rev() {
        let candidate_i64 = i64::from(vision_capture_id) * 10 + i64::from(q);
        if let Ok(candidate) = i32::try_from(candidate_i64)
            && assets.tables.phantom_item.get_data_by_id(&candidate).is_some()
        {
            return Some(candidate);
        }
    }
    if assets
        .tables
        .phantom_item
        .get_data_by_id(&vision_capture_id)
        .is_some()
    {
        return Some(vision_capture_id);
    }

    let mut best_id = None;
    let mut best_quality = -1;
    for p in &assets.tables.phantom_item.items {
        if p.MonsterId == vision_capture_id
            && p.QualityId <= max_quality
            && p.QualityId > best_quality
        {
            best_quality = p.QualityId;
            best_id = Some(p.ItemId);
        }
    }
    best_id
}

pub fn grant_monster_drop<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    reward_id: i32,
    vision_capture_id: i32,
    is_boss: bool,
    calabash_level: i32,
) -> io::Result<GrantResult> {
    grant_monster_drop_with_echo_option(
        ctx,
        reward_id,
        vision_capture_id,
        is_boss,
        calabash_level,
        true,
    )
}

pub fn grant_monster_drop_with_echo_option<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    reward_id: i32,
    vision_capture_id: i32,
    is_boss: bool,
    calabash_level: i32,
    grant_echo_now: bool,
) -> io::Result<GrantResult> {
    let mut total = if reward_id > 0 {
        grant_drop_package(ctx, reward_id, 1)?
    } else {
        GrantResult::default()
    };

    let mut extras = Vec::new();
    if total.granted_count == 0 {
        extras.push(RewardItem {
            item_id: 1,
            count: if is_boss { 200 } else { 10 },
        });
        extras.push(RewardItem {
            item_id: 2,
            count: if is_boss { 2000 } else { 150 },
        });
        if is_boss {
            extras.push(RewardItem {
                item_id: 41_100_012,
                count: 2,
            });
        }
    }

    if grant_echo_now
        && vision_capture_id > 0
        && let Some(echo_item_id) =
            resolve_phantom_drop_item_id(ctx.assets, vision_capture_id, calabash_level)
    {
        extras.push(RewardItem {
            item_id: echo_item_id,
            count: 1,
        });
    }

    if !extras.is_empty() {
        let extra_res = grant_items(ctx, 0, false, &extras, 1)?;
        total.granted_count += extra_res.granted_count;
        total.leveled_up = total.leveled_up || extra_res.leveled_up;
        if extra_res.new_level > total.new_level {
            total.new_level = extra_res.new_level;
        }
    }
    Ok(total)
}

pub fn grant_echo_capture_only<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    vision_capture_id: i32,
    calabash_level: i32,
) -> io::Result<GrantResult> {
    if vision_capture_id <= 0 {
        return Ok(GrantResult::default());
    }
    let Some(echo_item_id) =
        resolve_phantom_drop_item_id(ctx.assets, vision_capture_id, calabash_level)
    else {
        return Ok(GrantResult::default());
    };
    grant_items(
        ctx,
        0,
        false,
        &[RewardItem {
            item_id: echo_item_id,
            count: 1,
        }],
        1,
    )
}

pub fn grant_items<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    drop_id: i32,
    show_popup: bool,
    items: &[RewardItem],
    reason: i32,
) -> io::Result<GrantResult> {
    grant_items_with_tips(ctx, drop_id, show_popup, show_popup, items, reason)
}

/// `show_popup` sends `ItemRewardNotify`; `show_tips` clears `NoTips` on the item
/// add/update notifies, which is what makes the client list the items in its hint panel.
fn grant_items_with_tips<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    drop_id: i32,
    show_popup: bool,
    show_tips: bool,
    items: &[RewardItem],
    reason: i32,
) -> io::Result<GrantResult> {
    let mut result = GrantResult::default();
    if items.is_empty() {
        return Ok(result);
    }

    let mut basic_dirty = false;
    let mut inv_dirty = false;
    let mut calabash_dirty = false;
    let mut energy_dirty = false;
    let mut morale_dirty = false;
    let mut bonus_union_exp = 0i32;

    let effective_reason = if reason > 2 { reason } else { 19_500 };
    let mut show_plan_id = match effective_reason {
        7000 | 19_500 | 6003 | 18_500 | 15_008 | 20_200 => 1,
        _ => 3,
    };
    if ctx.assets.tables.drop_show_plan.get_data_by_id(&show_plan_id).is_none() {
        show_plan_id = 1;
    }

    let mut add_normal_list = Vec::new();
    let mut upd_normal_list = Vec::new();
    let mut add_weapons = Vec::new();
    let mut add_echoes = Vec::new();
    let mut reward_infos = Vec::new();

    for &entry in items {
        if entry.item_id <= 0 || entry.count <= 0 {
            continue;
        }

        if entry.item_id == 1 {
            result.granted_count += 1;
            add_union_exp(ctx.assets, ctx.basic_comp, entry.count, &mut result);
            basic_dirty = true;
            reward_infos.push(RewardItemInfo {
                ShowPlanId: 0,
                ItemId: 1,
                Count: entry.count,
                IncrId: 0,
            });
        } else if entry.item_id == 2 {
            result.granted_count += 1;
            ctx.basic_comp.info.attributes.coin = ctx
                .basic_comp
                .info
                .attributes
                .coin
                .saturating_add(entry.count);
            basic_dirty = true;
            upd_normal_list.push(NormalItem {
                Id: 2,
                Count: ctx.basic_comp.info.attributes.coin,
                ExpireTime: 0,
            });
            reward_infos.push(RewardItemInfo {
                ShowPlanId: 0,
                ItemId: 2,
                Count: entry.count,
                IncrId: 0,
            });
        } else if entry.item_id == 3 {
            result.granted_count += 1;
            ctx.basic_comp.info.attributes.rare_coin = ctx
                .basic_comp
                .info
                .attributes
                .rare_coin
                .saturating_add(entry.count);
            basic_dirty = true;
            let old_count = ctx.inventory_comp.info.normal_item_count(3);
            ctx.inventory_comp.info.add_normal_item(3, entry.count);
            inv_dirty = true;
            let new_total = ctx.inventory_comp.info.normal_item_count(3);
            let pb_item = NormalItem {
                Id: 3,
                Count: new_total,
                ExpireTime: 0,
            };
            if old_count == 0 {
                add_normal_list.push(pb_item);
            } else {
                upd_normal_list.push(pb_item);
            }
            reward_infos.push(RewardItemInfo {
                ShowPlanId: 0,
                ItemId: 3,
                Count: entry.count,
                IncrId: 0,
            });
        } else if entry.item_id == 4 {
            result.granted_count += 1;
            ctx.basic_comp.info.attributes.cash_coin = ctx
                .basic_comp
                .info
                .attributes
                .cash_coin
                .saturating_add(entry.count);
            basic_dirty = true;
            upd_normal_list.push(NormalItem {
                Id: 4,
                Count: ctx.basic_comp.info.attributes.cash_coin,
                ExpireTime: 0,
            });
            reward_infos.push(RewardItemInfo {
                ShowPlanId: 0,
                ItemId: 4,
                Count: entry.count,
                IncrId: 0,
            });
        } else if entry.item_id == 5 {
            result.granted_count += 1;
            let now_sec = energy::now_seconds();
            energy::sync_energy(ctx.basic_comp, now_sec);
            ctx.basic_comp.info.energy_count = energy::WAVEPLATE_HARD_CAP
                .min(ctx.basic_comp.info.energy_count.saturating_add(entry.count));
            basic_dirty = true;
            energy_dirty = true;
            reward_infos.push(RewardItemInfo {
                ShowPlanId: 0,
                ItemId: 5,
                Count: entry.count,
                IncrId: 0,
            });
        } else if entry.item_id == 6 {
            result.granted_count += 1;
            let now_sec = energy::now_seconds();
            energy::sync_energy(ctx.basic_comp, now_sec);
            ctx.basic_comp.info.store_energy_count = energy::CRYSTALWAVE_CAP
                .min(ctx.basic_comp.info.store_energy_count.saturating_add(entry.count));
            basic_dirty = true;
            energy_dirty = true;
            reward_infos.push(RewardItemInfo {
                ShowPlanId: 0,
                ItemId: 6,
                Count: entry.count,
                IncrId: 0,
            });
        } else if ctx
            .assets
            .tables
            .weapon_conf
            .get_data_by_id(&entry.item_id)
            .is_some()
        {
            result.granted_count += 1;
            for _ in 0..entry.count {
                let incr_id = special_item_incr::next(ctx.fs, ctx.weapon_comp.player_id)?;
                let weapon = WeaponItem {
                    id: entry.item_id,
                    func_value: WeaponItem::default_func_value(ctx.assets, entry.item_id),
                    ..Default::default()
                };
                ctx.weapon_comp.weapon_map.insert(incr_id, weapon.clone());
                let w_path = format!(
                    "player/{}/{}/{incr_id}",
                    ctx.weapon_comp.player_id,
                    WeaponItem::DATA_DIR
                );
                comp_util::save_struct(ctx.fs, &weapon, &w_path)?;
                add_weapons.push(weapon.to_proto(incr_id));
                reward_infos.push(RewardItemInfo {
                    ShowPlanId: 0,
                    ItemId: entry.item_id,
                    Count: 1,
                    IncrId: incr_id,
                });
            }
        } else if let Some(phantom_cfg) = ctx
            .assets
            .tables
            .phantom_item
            .get_data_by_id(&entry.item_id)
        {
            result.granted_count += 1;
            if echo_info::valid_default_owned_item(ctx.assets, &phantom_cfg) {
                let fetter = phantom_cfg.FetterGroup[0];
                for _ in 0..entry.count {
                    let incr_id = special_item_incr::next(ctx.fs, ctx.echo_comp.player_id)?;
                    let new_echo = echo_info::build_default_echo(ctx.assets, &phantom_cfg, fetter);
                    add_echoes.push(new_echo.to_proto(incr_id));
                    let e_path = format!(
                        "player/{}/{}/{incr_id}",
                        ctx.echo_comp.player_id,
                        echo_info::EchoInfo::DATA_DIR
                    );
                    comp_util::save_struct(ctx.fs, &new_echo, &e_path)?;
                    ctx.echo_comp.echo_map.insert(incr_id, new_echo);
                    reward_infos.push(RewardItemInfo {
                        ShowPlanId: 0,
                        ItemId: entry.item_id,
                        Count: 1,
                        IncrId: incr_id,
                    });

                    let discovery = calabash::record_echo_discovery(
                        ctx.assets,
                        ctx.echo_comp,
                        phantom_cfg.MonsterId,
                        phantom_cfg.QualityId,
                    );
                    if discovery.changed {
                        calabash_dirty = true;
                    }
                    bonus_union_exp += discovery.union_exp;
                }
            }
        } else if entry.item_id == morale::DEFAULT_KEEP_ITEM_ID
            || entry.item_id == morale::DEFAULT_TEMP_ITEM_ID
            || ctx
                .assets
                .tables
                .item_info
                .get_data_by_id(&entry.item_id)
                .is_some()
        {
            result.granted_count += 1;
            let old_count = ctx.inventory_comp.info.normal_item_count(entry.item_id);
            ctx.inventory_comp
                .info
                .add_normal_item(entry.item_id, entry.count);
            inv_dirty = true;
            if entry.item_id == morale::DEFAULT_KEEP_ITEM_ID
                || entry.item_id == morale::DEFAULT_TEMP_ITEM_ID
            {
                morale_dirty = true;
            }
            let new_total = ctx.inventory_comp.info.normal_item_count(entry.item_id);
            let pb_item = NormalItem {
                Id: entry.item_id,
                Count: new_total,
                ExpireTime: 0,
            };
            if old_count == 0 {
                add_normal_list.push(pb_item);
            } else {
                upd_normal_list.push(pb_item);
            }
            reward_infos.push(RewardItemInfo {
                ShowPlanId: 0,
                ItemId: entry.item_id,
                Count: entry.count,
                IncrId: 0,
            });
        }
    }

    if bonus_union_exp > 0 {
        add_union_exp(ctx.assets, ctx.basic_comp, bonus_union_exp, &mut result);
        basic_dirty = true;
    }

    if basic_dirty {
        let b_path = format!("player/{}/basic_info", ctx.basic_comp.player_id);
        comp_util::save_struct(ctx.fs, &ctx.basic_comp.info, &b_path)?;

        let attrs = vec![
            PlayerAttr {
                Key: Some(PlayerAttrKey::Level),
                ValueType: Some(PlayerAttrType::Int32),
                Value: Some(PlayerAttrValueOneof::Int32Value(
                    ctx.basic_comp.info.attributes.level,
                )),
            },
            PlayerAttr {
                Key: Some(PlayerAttrKey::Exp),
                ValueType: Some(PlayerAttrType::Int32),
                Value: Some(PlayerAttrValueOneof::Int32Value(
                    ctx.basic_comp.info.attributes.exp,
                )),
            },
            PlayerAttr {
                Key: Some(PlayerAttrKey::Coin),
                ValueType: Some(PlayerAttrType::Int32),
                Value: Some(PlayerAttrValueOneof::Int32Value(
                    ctx.basic_comp.info.attributes.coin,
                )),
            },
            PlayerAttr {
                Key: Some(PlayerAttrKey::RareCoin),
                ValueType: Some(PlayerAttrType::Int32),
                Value: Some(PlayerAttrValueOneof::Int32Value(
                    ctx.basic_comp.info.attributes.rare_coin,
                )),
            },
            PlayerAttr {
                Key: Some(PlayerAttrKey::CashCoin),
                ValueType: Some(PlayerAttrType::Int32),
                Value: Some(PlayerAttrValueOneof::Int32Value(
                    ctx.basic_comp.info.attributes.cash_coin,
                )),
            },
        ];
        ctx.sink
            .push(PlayerAttrNotify::MSG_ID, &PlayerAttrNotify { Attributes: attrs });
    }

    if energy_dirty {
        energy::push_energy_notify(ctx.sink, ctx.basic_comp);
    }

    if inv_dirty {
        let i_path = format!(
            "player/{}/{}",
            ctx.inventory_comp.player_id,
            InventoryInfo::DATA_PATH
        );
        comp_util::save_struct(ctx.fs, &ctx.inventory_comp.info, &i_path)?;
    }

    if !add_normal_list.is_empty() {
        ctx.sink.push(
            NormalItemAddNotify::MSG_ID,
            &NormalItemAddNotify {
                NormalItemList: add_normal_list,
                NoTips: !show_tips,
                Reason: reason,
            },
        );
    }
    if !upd_normal_list.is_empty() {
        ctx.sink.push(
            NormalItemUpdateNotify::MSG_ID,
            &NormalItemUpdateNotify {
                NormalItemList: upd_normal_list,
                NoTips: !show_tips,
            },
        );
    }
    if !add_weapons.is_empty() {
        ctx.sink.push(
            WeaponItemAddNotify::MSG_ID,
            &WeaponItemAddNotify {
                WeaponItemList: add_weapons,
                AddFromRole: false,
                Reason: reason,
            },
        );
    }
    if !add_echoes.is_empty() {
        ctx.sink.push(
            PhantomItemAddNotify::MSG_ID,
            &PhantomItemAddNotify {
                PhantomItemList: add_echoes,
                Reason: reason,
            },
        );
    }
    if calabash_dirty {
        PlayerEchoComponent::save_calabash(
            ctx.fs,
            ctx.echo_comp.player_id,
            &ctx.echo_comp.calabash_info,
        )?;
        let notify = calabash::build_msg_notify(ctx.assets, ctx.basic_comp, ctx.echo_comp);
        ctx.sink.push(CalabashMsgNotify::MSG_ID, &notify);
    }

    if morale_dirty {
        let _ = morale::sync_from_inventory(
            Some(&mut *ctx.sink),
            ctx.fs,
            ctx.assets,
            ctx.basic_comp,
            ctx.inventory_comp,
            ctx.progress_comp.as_deref_mut(),
            None,
            None,
            true,
        )?;
    }

    if show_popup && !reward_infos.is_empty() {
        for info in &mut reward_infos {
            info.ShowPlanId = show_plan_id;
        }
        let mut reward_entries = BTreeMap::new();
        reward_entries.insert(
            0,
            RewardItemInfoList {
                ItemList: reward_infos,
            },
        );
        ctx.sink.push(
            ItemRewardNotify::MSG_ID,
            &ItemRewardNotify {
                DropId: if drop_id > 0 && ctx.assets.tables.drop_package.get_data_by_id(&drop_id).is_some()
                {
                    drop_id
                } else {
                    1001
                },
                Reason: effective_reason,
                Magnification: 1,
                DropFrom: None,
                RewardItems: reward_entries,
            },
        );
    }

    Ok(result)
}

fn add_union_exp(
    assets: &Assets,
    basic_comp: &mut PlayerBasicComponent,
    amount: i32,
    result: &mut GrantResult,
) {
    if amount <= 0 {
        return;
    }
    basic_comp.info.attributes.exp += amount;
    let prev_level = basic_comp.info.attributes.level;
    apply_player_exp_level_up(
        assets,
        &mut basic_comp.info.attributes.level,
        &mut basic_comp.info.attributes.exp,
    );
    if basic_comp.info.attributes.level > prev_level {
        result.leveled_up = true;
    }
    result.new_level = basic_comp.info.attributes.level;
}

fn apply_player_exp_level_up(assets: &Assets, level_ptr: &mut i32, exp_ptr: &mut i32) {
    while *level_ptr < 80 {
        let Some(cfg) = assets.tables.player_exp.get_data_by_id(level_ptr) else {
            break;
        };
        if cfg.LevelExp <= 0 || *exp_ptr < cfg.LevelExp {
            break;
        }
        *exp_ptr -= cfg.LevelExp;
        *level_ptr += 1;
    }
    if *level_ptr >= 80 {
        *exp_ptr = 0;
    }
}

pub fn collect_drop_items(assets: &Assets, drop_id: i32, out_items: &mut Vec<RewardItem>) {
    out_items.extend(preview_drop_items(assets, drop_id));
}

#[allow(clippy::too_many_arguments)]
pub fn grant_table_reward<S: PushSink + ?Sized>(
    ctx: &mut RewardContext<'_, S>,
    table_name: &str,
    key: i64,
    use_pay: bool,
    show_popup: bool,
    reason: i32,
    fallback: &[RewardItem],
    out_items: Option<&mut Vec<RewardItem>>,
) -> io::Result<GrantResult> {
    let mut grants = Vec::new();
    let mut used_drop_id = 0i32;

    if let Some(row) = ctx.assets.reward_index.get_row(table_name, key) {
        let pair_slice = if use_pay { &row.pay_items } else { &row.items };
        for p in pair_slice {
            if p.item_id > 0 && p.count > 0 {
                grants.push(RewardItem {
                    item_id: p.item_id,
                    count: p.count,
                });
            }
        }
        let drop_id = if use_pay && row.alt_drop_id > 0 {
            row.alt_drop_id
        } else {
            row.drop_id
        };
        if grants.is_empty() && drop_id > 0 {
            used_drop_id = drop_id;
            collect_drop_items(ctx.assets, drop_id, &mut grants);
        }
    }

    if grants.is_empty() {
        for &item in fallback {
            if item.item_id > 0 && item.count > 0 {
                grants.push(item);
            }
        }
    }

    if let Some(out) = out_items {
        out.extend_from_slice(&grants);
    }
    if grants.is_empty() {
        return Ok(GrantResult::default());
    }
    grant_items(ctx, used_drop_id, show_popup, &grants, reason)
}
