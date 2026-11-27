use std::collections::BTreeMap;
use std::io;

use sonoro_common::fs::FileSystem;
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::zon_config::GameConfig;
use sonoro_gamesv_fs::role_info::RoleInfo;

use crate::component::comp_util::{self, FsStoreItem, StoreType};
use crate::component::player::{PlayerEchoComponent, PlayerWeaponComponent};
use crate::helpers::role_equipment::RoleEquipment;

impl FsStoreItem for RoleInfo {
    const DATA_DIR: &'static str = RoleInfo::DATA_DIR;

    fn add_defaults(assets: &Assets, map: &mut BTreeMap<i32, Self>) {
        if GameConfig::embedded().normal_progression {
            RoleInfo::add_main_defaults(assets, map);
        } else {
            RoleInfo::add_defaults(assets, map);
        }
    }
}

#[derive(Debug, Default)]
pub struct PlayerRoleComponent {
    pub player_id: i32,
    pub role_map: BTreeMap<i32, RoleInfo>,
    pub equipment_map: BTreeMap<i32, RoleEquipment>,
    pub trial_roles: std::collections::HashSet<i32>,
}

impl PlayerRoleComponent {
    pub fn init(fs: &FileSystem, assets: &Assets, player_id: i32) -> io::Result<Self> {
        let role_map = comp_util::load_items::<RoleInfo>(fs, assets, player_id, StoreType::ByDataId)?;
        Ok(Self {
            player_id,
            role_map,
            equipment_map: BTreeMap::new(),
            trial_roles: std::collections::HashSet::new(),
        })
    }

    pub fn is_trial_role(&self, role_id: i32) -> bool {
        self.trial_roles.contains(&role_id)
    }

    pub fn remove_equipment_cache(&mut self, role_id: i32) {
        self.equipment_map.remove(&role_id);
    }

    pub fn rebuild_equipment(
        &mut self,
        assets: &Assets,
        role_id: i32,
        weapon_comp: &PlayerWeaponComponent,
        echo_comp: &PlayerEchoComponent,
    ) -> Result<&RoleEquipment, &'static str> {
        let role_info = self.role_map.get(&role_id).ok_or("RoleNotFound")?;
        let equipment = RoleEquipment::build(assets, role_id, role_info, weapon_comp, echo_comp);
        self.equipment_map.insert(role_id, equipment);
        Ok(self.equipment_map.get(&role_id).expect("just inserted"))
    }
}
