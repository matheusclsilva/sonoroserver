use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IdCount {
    pub id: i32,
    pub count: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IdRoleCount {
    pub id: i32,
    pub count: i32,
    pub last_role_id: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ShopGoodCount {
    pub shop_id: i32,
    pub goods_id: i32,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AchievementRecord {
    pub id: i32,
    pub finish_time: u32,
    pub received: bool,
    pub cur_progress: i32,
    pub total_progress: i32,
}

impl Default for AchievementRecord {
    fn default() -> Self {
        Self {
            id: 0,
            finish_time: 0,
            received: false,
            cur_progress: 0,
            total_progress: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LordGymPass {
    pub gym_id: i32,
    pub pass_time: i32,
}

impl Default for LordGymPass {
    fn default() -> Self {
        Self {
            gym_id: 0,
            pass_time: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TowerFloor {
    pub config_id: i32,
    pub difficulty: i32,
    pub area_num: i32,
    pub star: i32,
    pub star_mask: i32,
}

impl Default for TowerFloor {
    fn default() -> Self {
        Self {
            config_id: 0,
            difficulty: 1,
            area_num: 1,
            star: 3,
            star_mask: 7,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DifficultyReward {
    pub difficulty: i32,
    pub reward_index: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DifficultyScore {
    pub season: i32,
    pub difficulty: i32,
    pub score: i32,
}

impl Default for DifficultyScore {
    fn default() -> Self {
        Self {
            season: 1,
            difficulty: 0,
            score: 0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FishingCabinItem {
    pub cabin_type: i32,
    pub id: i32,
    pub incr_id: i32,
    pub item_id: i32,
    pub x: i32,
    pub y: i32,
    pub rotate: i32,
    pub size: i32,
    pub cup: i32,
    pub quality: i32,
    pub price: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DailyCounter {
    pub counter_type: i32,
    pub extra_param: i32,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CraftState {
    pub unlocked_forge: Vec<i32>,
    pub forge_counts: Vec<IdRoleCount>,
    pub synth_level: i32,
    pub synth_proficiency: i32,
    pub synth_claimed_level: i32,
    pub unlocked_synth: Vec<i32>,
    pub synth_counts: Vec<IdRoleCount>,
    pub cook_level: i32,
    pub cook_proficiency: i32,
    pub cook_claimed_level: i32,
    pub unlocked_cook: Vec<i32>,
    pub unlocked_processed: Vec<i32>,
    pub cook_counts: Vec<IdRoleCount>,
}

impl Default for CraftState {
    fn default() -> Self {
        Self {
            unlocked_forge: Vec::new(),
            forge_counts: Vec::new(),
            synth_level: 1,
            synth_proficiency: 0,
            synth_claimed_level: 1,
            unlocked_synth: Vec::new(),
            synth_counts: Vec::new(),
            cook_level: 1,
            cook_proficiency: 0,
            cook_claimed_level: 1,
            unlocked_cook: Vec::new(),
            unlocked_processed: Vec::new(),
            cook_counts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ShopState {
    pub payshop_bought: Vec<IdCount>,
    pub fixed_shop_bought: Vec<ShopGoodCount>,
    pub item_exchange_counts: Vec<IdCount>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AchievementState {
    pub entries: Vec<AchievementRecord>,
    pub claimed_groups: Vec<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LivenessState {
    pub liveness_count: i32,
    pub claimed_boxes: Vec<i32>,
    pub taken_tasks: Vec<i32>,
    pub task_progress: Vec<IdCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ChallengeState {
    pub read_lord_gyms: Vec<i32>,
    pub passed_lord_gyms: Vec<LordGymPass>,
    pub tower_season: i32,
    pub tower_floors: Vec<TowerFloor>,
    pub tower_claimed_rewards: Vec<DifficultyReward>,
    pub finished_rogue_insts: Vec<i32>,
    pub new_tower_scores: Vec<DifficultyScore>,
}

impl Default for ChallengeState {
    fn default() -> Self {
        Self {
            read_lord_gyms: Vec::new(),
            passed_lord_gyms: Vec::new(),
            tower_season: 1,
            tower_floors: Vec::new(),
            tower_claimed_rewards: Vec::new(),
            finished_rogue_insts: Vec::new(),
            new_tower_scores: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FishingState {
    pub tech_levels: Vec<IdCount>,
    pub cabin_items: Vec<FishingCabinItem>,
    pub next_fish_incr_id: i32,
    pub total_sold: i32,
}

impl Default for FishingState {
    fn default() -> Self {
        Self {
            tech_levels: Vec::new(),
            cabin_items: Vec::new(),
            next_fish_incr_id: 1,
            total_sold: 0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CounterState {
    pub daily_counters: Vec<DailyCounter>,
    pub daily_quest_group_counts: Vec<IdCount>,
    pub records: Vec<i32>,
    pub claimed_cdkeys: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct StrInt {
    pub key: String,
    pub val: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct StrPair {
    pub key: String,
    pub val: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DynamicBucket {
    pub ints: Vec<StrInt>,
    pub strs: Vec<StrPair>,
    pub claims: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProgressInfo {
    pub craft: CraftState,
    pub shop: ShopState,
    pub achievement: AchievementState,
    pub liveness: LivenessState,
    pub challenge: ChallengeState,
    pub fishing: FishingState,
    pub counters: CounterState,
    pub dynamic: DynamicBucket,
}

impl ProgressInfo {
    pub const DATA_PATH: &'static str = "progress_info";

    pub fn append_unique_id(slice: &mut Vec<i32>, val: i32) -> bool {
        if val <= 0 || slice.contains(&val) {
            return false;
        }
        slice.push(val);
        true
    }

    pub fn get_id_count(slice: &[IdCount], id: i32) -> i32 {
        slice.iter().find(|e| e.id == id).map_or(0, |e| e.count)
    }

    pub fn add_id_count(slice: &mut Vec<IdCount>, id: i32, delta: i32) -> i32 {
        if id <= 0 || delta == 0 {
            return Self::get_id_count(slice, id);
        }
        if let Some(e) = slice.iter_mut().find(|e| e.id == id) {
            e.count += delta;
            return e.count;
        }
        slice.push(IdCount { id, count: delta });
        delta
    }

    pub fn find_id_role_count(slice: &[IdRoleCount], id: i32) -> Option<IdRoleCount> {
        slice.iter().find(|e| e.id == id).cloned()
    }

    pub fn add_id_role_count(
        slice: &mut Vec<IdRoleCount>,
        id: i32,
        delta: i32,
        role_id: i32,
    ) {
        if id <= 0 {
            return;
        }
        if let Some(e) = slice.iter_mut().find(|e| e.id == id) {
            e.count += delta;
            if role_id > 0 {
                e.last_role_id = role_id;
            }
            return;
        }
        slice.push(IdRoleCount {
            id,
            count: delta,
            last_role_id: role_id,
        });
    }

    pub fn get_shop_good_count(&self, shop_id: i32, goods_id: i32) -> i32 {
        self.shop
            .fixed_shop_bought
            .iter()
            .find(|r| r.shop_id == shop_id && r.goods_id == goods_id)
            .map_or(0, |r| r.count)
    }

    pub fn add_shop_good_count(&mut self, shop_id: i32, goods_id: i32, delta: i32) -> i32 {
        if let Some(r) = self
            .shop
            .fixed_shop_bought
            .iter_mut()
            .find(|r| r.shop_id == shop_id && r.goods_id == goods_id)
        {
            r.count += delta;
            return r.count;
        }
        self.shop.fixed_shop_bought.push(ShopGoodCount {
            shop_id,
            goods_id,
            count: delta,
        });
        delta
    }

    pub fn find_achievement_mut(&mut self, id: i32) -> Option<&mut AchievementRecord> {
        self.achievement.entries.iter_mut().find(|r| r.id == id)
    }

    pub fn finish_achievement(&mut self, id: i32, finish_time: u32) -> &mut AchievementRecord {
        let ts = if finish_time > 0 { finish_time } else { 1 };
        if let Some(pos) = self.achievement.entries.iter().position(|r| r.id == id) {
            let rec = &mut self.achievement.entries[pos];
            if rec.finish_time == 0 {
                rec.finish_time = ts;
            }
            rec.cur_progress = rec.total_progress;
            return rec;
        }
        self.achievement.entries.push(AchievementRecord {
            id,
            finish_time: ts,
            received: false,
            cur_progress: 1,
            total_progress: 1,
        });
        self.achievement.entries.last_mut().expect("just pushed")
    }

    pub fn has_passed_lord_gym(&self, gym_id: i32) -> bool {
        self.challenge
            .passed_lord_gyms
            .iter()
            .any(|r| r.gym_id == gym_id)
    }

    pub fn record_lord_gym_pass(&mut self, gym_id: i32, pass_time: i32) -> bool {
        if gym_id <= 0 {
            return false;
        }
        if let Some(rec) = self
            .challenge
            .passed_lord_gyms
            .iter_mut()
            .find(|r| r.gym_id == gym_id)
        {
            if pass_time > 0 && (rec.pass_time <= 0 || pass_time < rec.pass_time) {
                rec.pass_time = pass_time;
            }
            return false;
        }
        self.challenge.passed_lord_gyms.push(LordGymPass {
            gym_id,
            pass_time: if pass_time > 0 { pass_time } else { 1 },
        });
        true
    }

    pub fn has_claimed_tower_reward(&self, difficulty: i32, reward_index: i32) -> bool {
        self.challenge
            .tower_claimed_rewards
            .iter()
            .any(|r| r.difficulty == difficulty && r.reward_index == reward_index)
    }

    pub fn claim_tower_reward(&mut self, difficulty: i32, reward_index: i32) -> bool {
        if self.has_claimed_tower_reward(difficulty, reward_index) {
            return false;
        }
        self.challenge.tower_claimed_rewards.push(DifficultyReward {
            difficulty,
            reward_index,
        });
        true
    }

    pub fn record_tower_floor(
        &mut self,
        config_id: i32,
        difficulty: i32,
        area_num: i32,
        star: i32,
        star_mask: i32,
    ) {
        if let Some(rec) = self
            .challenge
            .tower_floors
            .iter_mut()
            .find(|r| r.config_id == config_id)
        {
            if star > rec.star {
                rec.star = star;
            }
            rec.star_mask |= star_mask;
            return;
        }
        self.challenge.tower_floors.push(TowerFloor {
            config_id,
            difficulty,
            area_num,
            star,
            star_mask,
        });
    }

    pub fn get_daily_counter(&self, counter_type: i32, extra_param: i32) -> i32 {
        self.counters
            .daily_counters
            .iter()
            .find(|c| c.counter_type == counter_type && c.extra_param == extra_param)
            .map_or(0, |c| c.count)
    }

    pub fn inc_daily_counter(&mut self, counter_type: i32, extra_param: i32, delta: i32) {
        if counter_type <= 0 || delta == 0 {
            return;
        }
        if let Some(c) = self
            .counters
            .daily_counters
            .iter_mut()
            .find(|c| c.counter_type == counter_type && c.extra_param == extra_param)
        {
            c.count += delta;
            return;
        }
        self.counters.daily_counters.push(DailyCounter {
            counter_type,
            extra_param,
            count: delta,
        });
    }

    pub fn get_new_tower_score(&self, season: i32, difficulty: i32) -> i32 {
        self.challenge
            .new_tower_scores
            .iter()
            .filter(|s| {
                (season <= 0 || s.season == season)
                    && (difficulty < 0 || s.difficulty == difficulty)
            })
            .map(|s| s.score)
            .max()
            .unwrap_or(0)
    }

    pub fn has_claimed_cdkey(&self, key: &str) -> bool {
        self.counters.claimed_cdkeys.iter().any(|k| k == key)
    }

    pub fn claim_cdkey(&mut self, key: &str) -> bool {
        if key.is_empty() || self.has_claimed_cdkey(key) {
            return false;
        }
        self.counters.claimed_cdkeys.push(key.to_string());
        true
    }

    pub fn get_key_int(&self, key: &str) -> i64 {
        self.dynamic
            .ints
            .iter()
            .find(|e| e.key == key)
            .map_or(0, |e| e.val)
    }

    pub fn set_key_int(&mut self, key: &str, val: i64) {
        if key.is_empty() {
            return;
        }
        if let Some(e) = self.dynamic.ints.iter_mut().find(|e| e.key == key) {
            e.val = val;
            return;
        }
        self.dynamic.ints.push(StrInt {
            key: key.to_string(),
            val,
        });
    }

    pub fn add_key_int(&mut self, key: &str, delta: i64) -> i64 {
        let next = self.get_key_int(key) + delta;
        self.set_key_int(key, next);
        next
    }

    pub fn get_key_str(&self, key: &str) -> &str {
        self.dynamic
            .strs
            .iter()
            .find(|e| e.key == key)
            .map_or("", |e| e.val.as_str())
    }

    pub fn set_key_str(&mut self, key: &str, val: &str) {
        if key.is_empty() {
            return;
        }
        if let Some(e) = self.dynamic.strs.iter_mut().find(|e| e.key == key) {
            e.val = val.to_string();
            return;
        }
        self.dynamic.strs.push(StrPair {
            key: key.to_string(),
            val: val.to_string(),
        });
    }

    pub fn has_claim(&self, key: &str) -> bool {
        self.dynamic.claims.iter().any(|c| c == key)
    }

    pub fn mark_claim(&mut self, key: &str) -> bool {
        if key.is_empty() || self.has_claim(key) {
            return false;
        }
        self.dynamic.claims.push(key.to_string());
        true
    }

    pub fn remove_claim(&mut self, key: &str) -> bool {
        let before = self.dynamic.claims.len();
        self.dynamic.claims.retain(|c| c != key);
        self.dynamic.claims.len() != before
    }

    pub fn remove_claims_with_prefix(&mut self, prefix: &str) -> bool {
        let before = self.dynamic.claims.len();
        self.dynamic.claims.retain(|c| !c.starts_with(prefix));
        self.dynamic.claims.len() != before
    }

    pub fn count_claims_prefix(&self, prefix: &str) -> i32 {
        self.dynamic
            .claims
            .iter()
            .filter(|c| c.starts_with(prefix))
            .count() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_info_tracks_shop_craft_achievements_challenges_counters() {
        let mut info = ProgressInfo::default();
        assert_eq!(ProgressInfo::add_id_count(&mut info.shop.payshop_bought, 10001, 3), 3);
        ProgressInfo::add_id_role_count(&mut info.craft.cook_counts, 10000, 2, 1104);
        assert_eq!(
            ProgressInfo::find_id_role_count(&info.craft.cook_counts, 10000)
                .unwrap()
                .count,
            2
        );

        let ach = info.finish_achievement(300101, 12345);
        assert_eq!(ach.finish_time, 12345);
        assert!(!ach.received);

        assert!(info.record_lord_gym_pass(1001, 45));
        assert!(info.has_passed_lord_gym(1001));
        info.record_tower_floor(1, 1, 1, 3, 7);
        assert!(info.claim_tower_reward(1, 0));
        assert!(!info.claim_tower_reward(1, 0));

        info.inc_daily_counter(15, 1, 2);
        assert_eq!(info.get_daily_counter(15, 1), 2);
        assert!(info.claim_cdkey("WUTHERINGGIFT"));
        assert!(!info.claim_cdkey("WUTHERINGGIFT"));
    }
}
