//! Combat resolution shared by player and creature attacks.
//!
//! Player attack/defense ratings follow the recovered 2012 server's
//! Player.CalculateAttackDefenseRatings. Encounter base damage, armor and the
//! creature's level * 8 starting ratings remain authored Pyrealium policy.
//!
//! The hit-chance curve is authored: neither the legacy server nor the
//! recovered 7148 client contains the original formula. Equal ratings hit 75%
//! of the time, and the chance stays between 5% and 95%.
use crate::skills::Book;

/// Per-school mastery skills (SkillList): master, grandmaster, paragon.
const MELEE: [u32; 3] = [0xa, 0x9, 0xb];
const MISSILE: [u32; 3] = [0xf, 0xe, 0x10];
const MAGIC: [u32; 3] = [0x4, 0x3, 0x5];

/// xorshift64* seeded from the OS; combat rolls need no cryptographic strength.
#[derive(Clone)]
pub struct Dice(u64);
impl Dice {
    #[cfg(test)]
    pub(crate) fn for_test(seed: u64) -> Self {
        Self(seed | 1)
    }
    pub fn seeded() -> Self {
        let mut seed = [0u8; 8];
        let _ = getrandom::getrandom(&mut seed);
        Self(u64::from_le_bytes(seed) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn roll(&mut self, chance: f32) -> bool {
        self.unit() < f64::from(chance)
    }
}

fn rank(book: &Book, skill: u32) -> f32 {
    book.get(skill)
        .filter(|_| book.usable(skill))
        .map_or(0.0, |trained| f32::from(trained.rank))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum School {
    Melee,
    Missile,
    Magic,
}
impl School {
    fn skills(self) -> [u32; 3] {
        match self {
            Self::Melee => MELEE,
            Self::Missile => MISSILE,
            Self::Magic => MAGIC,
        }
    }
}

/// Level * 8 plus mastery: attack 2.4/4.4/6.4 per rank, defense 2/4/6.
pub fn player_attack(level: u16, book: &Book, school: School) -> u32 {
    let [master, grand, paragon] = school.skills();
    (f32::from(level) * 8.0
        + rank(book, master) * 2.4
        + rank(book, grand) * 4.4
        + rank(book, paragon) * 6.4) as u32
}
pub fn player_defense(level: u16, book: &Book, school: School) -> u32 {
    let [master, grand, paragon] = school.skills();
    (f32::from(level) * 8.0
        + rank(book, master) * 2.0
        + rank(book, grand) * 4.0
        + rank(book, paragon) * 6.0) as u32
}

/// Magic, melee, missile. A spell that lists every school repeats one bonus per school.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ratings {
    pub armor: i32,
    pub attack: [i32; 3],
    pub defense: [i32; 3],
}
impl Ratings {
    pub fn is_empty(self) -> bool {
        self.armor == 0 && self.attack == [0; 3] && self.defense == [0; 3]
    }
    pub fn add(&mut self, other: Self) {
        self.armor += other.armor;
        for index in 0..3 {
            self.attack[index] += other.attack[index];
            self.defense[index] += other.defense[index];
        }
    }
    fn index(school: School) -> usize {
        match school {
            School::Magic => 0,
            School::Melee => 1,
            School::Missile => 2,
        }
    }
    pub fn attack(self, school: School) -> i32 {
        self.attack[Self::index(school)]
    }
    pub fn defense(self, school: School) -> i32 {
        self.defense[Self::index(school)]
    }
    /// One creature has one attack and one defense. A spell that fills every
    /// school is one bonus, not three.
    pub fn creature_attack(self) -> i32 {
        Self::one(self.attack)
    }
    pub fn creature_defense(self) -> i32 {
        Self::one(self.defense)
    }
    fn one(values: [i32; 3]) -> i32 {
        if values[1] != 0 {
            values[1]
        } else if values[0] != 0 {
            values[0]
        } else {
            values[2]
        }
    }
}
pub fn adjusted(base: u32, bonus: i32) -> u32 {
    (i64::from(base) + i64::from(bonus)).clamp(0, i64::from(u32::MAX)) as u32
}
pub fn describe(ratings: &Ratings, duration_ms: u64) -> String {
    let mut parts = Vec::new();
    let mut push = |name: &str, value: i32| {
        if value != 0 {
            parts.push(format!("{name} {value:+}"));
        }
    };
    push("armor", ratings.armor);
    push("magic attack", ratings.attack(School::Magic));
    push("melee attack", ratings.attack(School::Melee));
    push("missile attack", ratings.attack(School::Missile));
    push("magic defense", ratings.defense(School::Magic));
    push("melee defense", ratings.defense(School::Melee));
    push("missile defense", ratings.defense(School::Missile));
    format!(
        "{} for {} seconds.",
        parts.join(", "),
        duration_ms.div_ceil(1000).max(1)
    )
}
pub fn hit_chance(attack: u32, defense: u32) -> f32 {
    let (a, d) = (attack as f32, defense as f32);
    if a + d <= 0.0 {
        return 0.75;
    }
    (0.75 + 0.5 * (a - d) / (a + d)).clamp(0.05, 0.95)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hit_chance_is_bounded_and_even_at_parity() {
        assert_eq!(hit_chance(100, 100), 0.75);
        assert!(hit_chance(8, 2000) >= 0.05);
        assert!(hit_chance(2000, 8) <= 0.95);
    }
    #[test]
    fn ratings_shift_a_hit() {
        assert_eq!(adjusted(8, 5), 13);
        assert_eq!(adjusted(3, -10), 0);
        let mut ratings = Ratings {
            armor: 4,
            attack: [0, 10, 0],
            defense: [0, -2, 0],
            ..Ratings::default()
        };
        ratings.add(Ratings {
            armor: 1,
            ..Ratings::default()
        });
        assert_eq!(
            (
                ratings.armor,
                ratings.attack(School::Melee),
                ratings.creature_defense()
            ),
            (5, 10, -2)
        );
    }
    #[test]
    fn common_mastery_can_train_advance_and_change_ratings_for_each_playable_race() {
        let state = crate::progression::Progression::default()
            .apply(&crate::progression::Command::AwardXp(1000))
            .unwrap();
        for race in [1, 2, 4] {
            let book = Book::default()
                .apply(crate::skills::Command::Train(4), &state, race)
                .unwrap();
            let (book, state) = book.advance(4, &state, race).unwrap();
            let base = u32::from(state.level().number) * 8;
            assert!(player_attack(state.level().number, &book, School::Magic) > base);
            assert_eq!(
                player_attack(state.level().number, &book, School::Melee),
                base
            );
            // Untraining must also stop applying the passive bonus immediately.
            let untraining = book
                .apply(crate::skills::Command::Untrain(4), &state, race)
                .unwrap();
            assert_eq!(
                player_attack(state.level().number, &untraining, School::Magic),
                base
            );
        }
    }
}
