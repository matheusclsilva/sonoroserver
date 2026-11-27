use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sonoro_gamesv_data::Assets;
use sonoro_gamesv_data::data_tables::base_property_to_props;
use sonoro_proto::enums::EAttributeType;
use sonoro_proto::pb::{
    ArrayIntInt, ArraySkillNode, ResonInfo, RoleInfo as PbRoleInfo,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Reson {
    pub id: i32,
    pub open: bool,
    pub increase: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SkillNode {
    pub node_id: i32,
    pub active: bool,
    pub skill_id: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OrnamentEquip {
    pub role_skin_id: i32,
    pub ornament_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoleInfo {
    pub level: i32,
    pub exp: i32,
    pub breakthrough: i32,
    pub skills: Vec<(i32, i32)>,
    pub skill_branch: i32,
    pub phantom: Vec<(i32, i32)>,
    pub weapon: i32,
    pub star: i32,
    pub favor: i32,
    pub reson: Vec<Reson>,
    pub cur_model: i32,
    pub models: Vec<i32>,
    pub base_prop: Vec<i32>,
    pub add_prop: Vec<i32>,
    pub current_hp: Option<i32>,
    pub create_time: u32,
    pub skill_node_state: Vec<SkillNode>,
    pub resonant_chain_group_index: i32,
    pub role_skin_id: i32,
    pub paragliding_skin_id: i32,
    pub soar_skin_id: i32,
    pub weapon_skin_id: i32,
    pub ornaments: Vec<OrnamentEquip>,
    pub calabash_skin_id: i32,
    pub enable_self_bgm: bool,
    pub voice_language: i32,
}

impl Default for RoleInfo {
    fn default() -> Self {
        Self {
            level: 1,
            exp: 0,
            breakthrough: 0,
            skills: Vec::new(),
            skill_branch: 0,
            phantom: Vec::new(),
            weapon: 0,
            star: 0,
            favor: 5,
            reson: Vec::new(),
            cur_model: 0,
            models: Vec::new(),
            base_prop: Vec::new(),
            add_prop: Vec::new(),
            current_hp: None,
            create_time: 0,
            skill_node_state: Vec::new(),
            resonant_chain_group_index: 0,
            role_skin_id: 0,
            paragliding_skin_id: 0,
            soar_skin_id: 0,
            weapon_skin_id: 0,
            ornaments: Vec::new(),
            calabash_skin_id: 0,
            enable_self_bgm: false,
            voice_language: 0,
        }
    }
}

impl RoleInfo {
    pub const DATA_DIR: &'static str = "role";

    pub fn get_ornament(&self, role_skin_id: i32) -> i32 {
        self.ornaments
            .iter()
            .find(|e| e.role_skin_id == role_skin_id && e.ornament_id != 0)
            .map_or(0, |e| e.ornament_id)
    }

    pub fn has_ornament(&self, role_skin_id: i32, ornament_id: i32) -> bool {
        ornament_id != 0
            && self
                .ornaments
                .iter()
                .any(|e| e.role_skin_id == role_skin_id && e.ornament_id == ornament_id)
    }

    pub fn dress_ornament(
        &mut self,
        assets: &Assets,
        role_skin_id: i32,
        ornament_id: i32,
    ) -> Result<bool, &'static str> {
        if ornament_id == 0 || self.has_ornament(role_skin_id, ornament_id) {
            return Ok(false);
        }
        let ornament = assets
            .tables
            .ornament
            .get_data_by_id(&ornament_id)
            .ok_or("OrnamentNotFound")?;

        self.ornaments.retain(|entry| {
            if entry.ornament_id == 0 {
                return false;
            }
            if entry.role_skin_id == role_skin_id
                && ornament.OrGroupId != 0
                && let Some(equipped) = assets.tables.ornament.get_data_by_id(&entry.ornament_id)
                && equipped.OrGroupId == ornament.OrGroupId
            {
                return false;
            }
            true
        });
        self.ornaments.push(OrnamentEquip {
            role_skin_id,
            ornament_id,
        });
        Ok(true)
    }

    pub fn undress_ornament(&mut self, role_skin_id: i32, ornament_id: i32) -> bool {
        if ornament_id == 0 {
            return false;
        }
        let before = self.ornaments.len();
        self.ornaments.retain(|entry| {
            entry.ornament_id != 0
                && !(entry.role_skin_id == role_skin_id && entry.ornament_id == ornament_id)
        });
        self.ornaments.len() != before
    }

    pub fn to_proto(&self, id: i32) -> PbRoleInfo {
        PbRoleInfo {
            RoleId: id,
            Name: Vec::new(),
            Level: self.level,
            Exp: self.exp,
            Breakthrough: self.breakthrough,
            Skills: array_int_int(&self.skills),
            Phantom: array_int_int(&self.phantom),
            Star: self.star,
            Favor: self.favor,
            Reson: self
                .reson
                .iter()
                .map(|r| ResonInfo {
                    ResonId: r.id,
                    IsOpen: r.open,
                    Increase: r.increase,
                })
                .collect(),
            CurModel: self.cur_model,
            Models: self.models.clone(),
            BaseProp: array_int_int_by_index(&self.base_prop),
            AddProp: array_int_int_by_index(&self.add_prop),
            CreateTime: self.create_time,
            SkillNodeState: self
                .skill_node_state
                .iter()
                .map(|n| ArraySkillNode {
                    SkillNodeId: n.node_id,
                    IsActive: n.active,
                    SkillId: n.skill_id,
                })
                .collect(),
            ResonantChainGroupIndex: self.resonant_chain_group_index,
            SkinId: self.role_skin_id,
            EnableSelfBgm: self.enable_self_bgm,
        }
    }

    pub fn reset_properties(&mut self, assets: &Assets, id: i32) {
        let Some(bp) = assets.tables.base_property.get_data_by_id(&id) else {
            return;
        };
        self.base_prop = base_property_to_props(&bp);
        self.scale_properties(assets);
    }

    pub fn compute_scaled_base_props(
        assets: &Assets,
        id: i32,
        level: i32,
        breakthrough: i32,
    ) -> Option<Vec<i32>> {
        let prop_id = assets
            .tables
            .resolve_base_role_info(id)
            .map_or(id, |rc| rc.PropertyId);
        let bp = assets
            .tables
            .base_property
            .get_data_by_id(&prop_id)
            .or_else(|| assets.tables.base_property.get_data_by_id(&id))?;
        let mut out = base_property_to_props(&bp);
        if level >= 1
            && let Some(rp_growth) = assets.tables.get_role_property_growth(level, breakthrough)
        {
            out[EAttributeType::Lv as usize] = level;
            scale_property(&mut out[EAttributeType::LifeMax as usize], rp_growth.LifeMaxRatio);
            scale_property(&mut out[EAttributeType::Life as usize], rp_growth.LifeMaxRatio);
            scale_property(&mut out[EAttributeType::Atk as usize], rp_growth.AtkRatio);
            scale_property(&mut out[EAttributeType::Def as usize], rp_growth.DefRatio);
        }
        Some(out)
    }

    pub fn refresh_scaled_properties(&mut self, assets: &Assets, id: i32) -> bool {
        let max_attr = EAttributeType::MAX as usize;
        let had_valid_len = self.base_prop.len() == max_attr;
        let prev_lv = if had_valid_len { self.base_prop[EAttributeType::Lv as usize] } else { 0 };
        let prev_life_max = if had_valid_len { self.base_prop[EAttributeType::LifeMax as usize] } else { 0 };
        let prev_life = if had_valid_len { self.base_prop[EAttributeType::Life as usize] } else { 0 };
        let prev_atk = if had_valid_len { self.base_prop[EAttributeType::Atk as usize] } else { 0 };
        let prev_def = if had_valid_len { self.base_prop[EAttributeType::Def as usize] } else { 0 };

        let Some(mut next_props) =
            Self::compute_scaled_base_props(assets, id, self.level, self.breakthrough)
        else {
            if !had_valid_len {
                self.base_prop = vec![0; max_attr];
            }
            return !had_valid_len;
        };

        let new_life_max = next_props[EAttributeType::LifeMax as usize];
        if had_valid_len && prev_life_max > 0 && prev_life < prev_life_max {
            next_props[EAttributeType::Life as usize] = if prev_life <= 0 {
                0
            } else {
                let scaled = (i64::from(prev_life) * i64::from(new_life_max))
                    / i64::from(prev_life_max);
                (scaled as i32).min(new_life_max).max(1)
            };
        }

        let changed = !had_valid_len
            || prev_lv != next_props[EAttributeType::Lv as usize]
            || prev_life_max != new_life_max
            || prev_atk != next_props[EAttributeType::Atk as usize]
            || prev_def != next_props[EAttributeType::Def as usize];
        self.base_prop = next_props;
        changed
    }

    pub fn scale_properties(&mut self, assets: &Assets) {
        if self.level >= 1
            && let Some(rp_growth) = assets
                .tables
                .get_role_property_growth(self.level, self.breakthrough)
        {
            self.base_prop[EAttributeType::Lv as usize] = self.level;
            scale_property(&mut self.base_prop[EAttributeType::LifeMax as usize], rp_growth.LifeMaxRatio);
            scale_property(&mut self.base_prop[EAttributeType::Life as usize], rp_growth.LifeMaxRatio);
            scale_property(&mut self.base_prop[EAttributeType::Atk as usize], rp_growth.AtkRatio);
            scale_property(&mut self.base_prop[EAttributeType::Def as usize], rp_growth.DefRatio);
        }
    }

    pub fn add_default_skills(&mut self, assets: &Assets, id: i32) -> Result<(), &'static str> {
        let role_config = assets
            .tables
            .resolve_base_role_info(id)
            .ok_or("RoleNotFound")?;
        let skill_tree_group_id = role_config.SkillTreeGroupId;

        let mut nodes = Vec::new();
        let mut skills = Vec::new();
        for node in &assets.tables.skill_tree.items {
            if node.NodeGroup != skill_tree_group_id {
                continue;
            }
            let active = node.Condition.is_empty() && node.UnLockCondition == 0;
            nodes.push(SkillNode {
                node_id: node.Id,
                active,
                skill_id: node.SkillId,
            });
            if active && node.SkillId != 0 {
                skills.push((node.SkillId, 1));
            }
        }
        self.skill_node_state = nodes;
        self.skills = skills;
        Ok(())
    }

    pub fn create_default(assets: &Assets, id: i32) -> Result<Self, &'static str> {
        let info = assets
            .tables
            .resolve_base_role_info(id)
            .ok_or("RoleNotFound")?;
        let mut role = Self {
            level: 1,
            breakthrough: 0,
            skill_branch: info.DefaultSkillBranchId,
            role_skin_id: info.SkinId,
            ..Default::default()
        };
        role.reset_properties(assets, id);
        role.add_default_skills(assets, id)?;
        Ok(role)
    }

    pub fn add_main_defaults(assets: &Assets, map: &mut BTreeMap<i32, RoleInfo>) {
        const MAIN_IDS: [i32; 2] = [1501, 1502];
        for id in MAIN_IDS {
            if assets.tables.role_info.get_data_by_id(&id).is_some()
                && let Ok(role) = Self::create_default(assets, id)
            {
                map.insert(id, role);
            }
        }
    }

    pub fn add_defaults(assets: &Assets, map: &mut BTreeMap<i32, RoleInfo>) {
        const STARTER_IDS: [i32; 5] = [1501, 1502, 1402, 1202, 1102];
        for id in STARTER_IDS {
            if assets.tables.role_info.get_data_by_id(&id).is_some()
                && let Ok(role) = Self::create_default(assets, id)
            {
                map.insert(id, role);
            }
        }
    }
}

fn scale_property(value: &mut i32, ratio: i32) {
    let float = ((*value as f32) * ((ratio as f32) / 10000.0)).trunc();
    *value = float.trunc() as i32;
}

fn array_int_int_by_index(data: &[i32]) -> Vec<ArrayIntInt> {
    data.iter()
        .enumerate()
        .map(|(k, &v)| ArrayIntInt {
            Key: k as i32,
            Value: v,
        })
        .collect()
}

fn array_int_int(data: &[(i32, i32)]) -> Vec<ArrayIntInt> {
    data.iter()
        .map(|&(k, v)| ArrayIntInt { Key: k, Value: v })
        .collect()
}
