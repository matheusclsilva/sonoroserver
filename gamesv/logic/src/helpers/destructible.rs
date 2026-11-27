//! Durability of destructible scene items (the `DestructibleItem` entity component).
//!
//! The client only sends `ThrowDamageRequest` while an entity's spawn `DurabilityValue` is above
//! zero, and each hit subtracts the `WeaponDamage` configured for the attacker's weapon family.

pub const DEFAULT_DURABILITY: i32 = 100;
pub const DEFAULT_HIT_DAMAGE: i32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponKind {
    GreatSword,
    Dagger,
    Pistol,
    Pugilism,
    Ring,
    Unknown,
}

pub fn weapon_kind_from_role_weapon_type(weapon_type: i32) -> WeaponKind {
    match weapon_type {
        1 => WeaponKind::GreatSword,
        2 => WeaponKind::Dagger,
        3 => WeaponKind::Pistol,
        4 => WeaponKind::Pugilism,
        5 => WeaponKind::Ring,
        _ => WeaponKind::Unknown,
    }
}

pub fn initial_durability(config_durability: Option<i32>, saved_durability: i64) -> i32 {
    if saved_durability > 0
        && let Ok(saved) = i32::try_from(saved_durability)
    {
        return saved;
    }
    let configured = config_durability.unwrap_or(0);
    if configured > 0 {
        configured
    } else {
        DEFAULT_DURABILITY
    }
}

pub fn hit_damage(
    weapon_damage: Option<&sonoro_gamesv_data::tables::entity_components::destructible_item::WeaponDamageConfig>,
    kind: WeaponKind,
) -> i32 {
    let Some(cfg) = weapon_damage else {
        return DEFAULT_HIT_DAMAGE;
    };
    let for_weapon = match kind {
        WeaponKind::GreatSword => cfg.GreatSword,
        WeaponKind::Dagger => cfg.Dagger,
        WeaponKind::Pistol => cfg.Pistol,
        WeaponKind::Pugilism => cfg.Pugilism,
        WeaponKind::Ring => cfg.Ring,
        WeaponKind::Unknown => None,
    };
    let dmg = for_weapon.or(cfg.DefaultValue).unwrap_or(DEFAULT_HIT_DAMAGE);
    if dmg > 0 { dmg } else { DEFAULT_HIT_DAMAGE }
}

pub fn after_hit(current: i32, damage: i32) -> i32 {
    (current - damage).max(0)
}

/// `Aki.TDConfigMgr.Component.ERewardType`: Nearby, Owner, Collect, HostInteract.
const REWARD_TYPE_NEARBY: i32 = 0;
const REWARD_TYPE_OWNER: i32 = 1;

/// The drop a destructible pays when its durability reaches zero. `Nearby` and `Owner`
/// rewards belong to the destruction (rocks, crates); `Collect` and `HostInteract` rewards
/// are paid by an interaction instead.
pub fn destruction_reward_id(
    reward: Option<&sonoro_gamesv_data::tables::entity_components::reward::RewardComponent>,
) -> Option<i32> {
    let rc = reward?;
    if rc.Disabled.unwrap_or(false) {
        return None;
    }
    let reward_id = rc.RewardId.unwrap_or(0);
    let on_destruction = matches!(
        rc.RewardType.unwrap_or(REWARD_TYPE_NEARBY),
        REWARD_TYPE_NEARBY | REWARD_TYPE_OWNER
    );
    (reward_id > 0 && on_destruction).then_some(reward_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonoro_gamesv_data::tables::entity_components::destructible_item::WeaponDamageConfig;

    fn ore_weapon_damage() -> WeaponDamageConfig {
        WeaponDamageConfig {
            DefaultValue: Some(100),
            GreatSword: Some(150),
            Dagger: Some(100),
            Pugilism: Some(75),
            Pistol: Some(60),
            Ring: Some(50),
        }
    }

    #[test]
    fn initial_durability_uses_saved_or_config_or_default() {
        assert_eq!(initial_durability(Some(300), 0), 300);
        assert_eq!(initial_durability(Some(300), 100), 100);
        assert_eq!(initial_durability(None, 0), DEFAULT_DURABILITY);
        assert_eq!(initial_durability(Some(0), 0), DEFAULT_DURABILITY);
    }

    #[test]
    fn hit_damage_uses_weapon_family_and_fallbacks() {
        let ore = ore_weapon_damage();
        assert_eq!(hit_damage(Some(&ore), weapon_kind_from_role_weapon_type(1)), 150);
        assert_eq!(hit_damage(Some(&ore), weapon_kind_from_role_weapon_type(2)), 100);
        assert_eq!(hit_damage(Some(&ore), weapon_kind_from_role_weapon_type(3)), 60);
        assert_eq!(hit_damage(Some(&ore), weapon_kind_from_role_weapon_type(4)), 75);
        assert_eq!(hit_damage(Some(&ore), weapon_kind_from_role_weapon_type(5)), 50);

        let only_default = WeaponDamageConfig {
            DefaultValue: Some(40),
            GreatSword: Some(80),
            ..Default::default()
        };
        assert_eq!(hit_damage(Some(&only_default), WeaponKind::Pistol), 40);
        assert_eq!(hit_damage(Some(&only_default), WeaponKind::Unknown), 40);
        assert_eq!(hit_damage(None, WeaponKind::Dagger), DEFAULT_HIT_DAMAGE);
        assert_eq!(after_hit(100, 100), 0);
        assert_eq!(after_hit(1, 100), 0);
    }

    #[test]
    fn destruction_reward_only_for_nearby_and_owner_rewards() {
        use sonoro_gamesv_data::tables::entity_components::reward::RewardComponent;

        let reward = |id: Option<i32>, ty: Option<i32>, disabled: Option<bool>| RewardComponent {
            RewardId: id,
            RewardType: ty,
            Disabled: disabled,
        };
        // Collect501 ore: {"RewardId": 2701, "RewardType": 0}.
        assert_eq!(destruction_reward_id(Some(&reward(Some(2701), Some(0), None))), Some(2701));
        assert_eq!(destruction_reward_id(Some(&reward(Some(2202), None, None))), Some(2202));
        assert_eq!(destruction_reward_id(Some(&reward(Some(1315), Some(1), None))), Some(1315));
        // Collect001 plant: {"RewardId": 2401, "RewardType": 2} is paid by the Collect action.
        assert_eq!(destruction_reward_id(Some(&reward(Some(2401), Some(2), None))), None);
        assert_eq!(destruction_reward_id(Some(&reward(Some(2401), Some(3), None))), None);
        assert_eq!(destruction_reward_id(Some(&reward(Some(2701), Some(0), Some(true)))), None);
        assert_eq!(destruction_reward_id(Some(&reward(None, Some(0), None))), None);
        assert_eq!(destruction_reward_id(None), None);
    }
}
