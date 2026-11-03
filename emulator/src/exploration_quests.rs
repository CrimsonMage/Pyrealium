//! Reconstructed ringway conversations. `q9` is an immutable version-one policy.
//! Native quest IDs/text and NPC table identities are recovered; guide XP/timers
//! and new obelisk placements are authored. Buff rewards are not yet implemented.
use serde::Deserialize;
use std::{collections::BTreeMap, io, sync::OnceLock};

#[derive(Debug, Deserialize)]
pub struct Profile {
    pub id: u32,
    pub title: String,
    pub npc: u32,
    #[serde(default)]
    pub npc_name: String,
    #[serde(default)]
    pub objectives: Vec<u32>,
    pub text: String,
    #[serde(default)]
    pub phases: Vec<String>,
    pub xp: u64,
    pub level: u16,
    pub limit_seconds: u64,
    pub repeat_seconds: u64,
}
#[derive(Deserialize)]
pub struct Catalog {
    version: u32,
    pub quests: Vec<Profile>,
    pub referral: Profile,
}
pub fn catalog() -> &'static Catalog {
    static DATA: OnceLock<Catalog> = OnceLock::new();
    DATA.get_or_init(|| {
        let c: Catalog =
            serde_json::from_str(include_str!("../content/exploration-content-v1.json"))
                .expect("exploration policy");
        assert_eq!(c.version, 1);
        assert_eq!(c.quests.len(), 3);
        let mut ids = std::collections::BTreeSet::new();
        for q in c.quests.iter().chain([&c.referral]) {
            assert!(ids.insert(q.id) && q.id >> 24 == 0x40 && q.npc >> 24 == 0x47);
            assert!(q.limit_seconds > 0 && q.repeat_seconds > 0 && q.level == 1);
            assert!(
                !crate::recovered_quests::profiles()
                    .iter()
                    .any(|old| old.id == q.id)
            );
        }
        for q in &c.quests {
            assert_eq!(q.objectives.len(), 3);
            assert_eq!(q.phases.len(), 4);
        }
        c
    })
}
pub fn profile(id: u32) -> Option<&'static Profile> {
    let c = catalog();
    c.quests.iter().chain([&c.referral]).find(|q| q.id == id)
}
pub fn supported(entity: u32) -> bool {
    let c = catalog();
    entity == c.referral.npc
        || c.quests
            .iter()
            .any(|q| q.npc == entity || q.objectives.contains(&entity))
}
pub fn speaker(entity: u32) -> &'static str {
    let c = catalog();
    for q in &c.quests {
        if q.npc == entity {
            return &q.npc_name;
        }
        if let Some(i) = q.objectives.iter().position(|e| *e == entity) {
            return ["Ruby Obelisk", "Sapphire Obelisk", "Emerald Obelisk"][i];
        }
    }
    "Arwic Mines Overseer"
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub phase: usize,
    pub started: u64,
    pub completed: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Book(pub BTreeMap<u32, State>);
pub struct Conversation {
    pub book: Book,
    pub xp: u64,
    pub text: String,
}
fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}
impl State {
    fn active(&self, q: &Profile, now: u64) -> bool {
        self.completed == 0 && now >= self.started && now - self.started < q.limit_seconds
    }
    fn restartable(&self, q: &Profile, now: u64) -> bool {
        if self.completed > 0 {
            now >= self.completed && now - self.completed >= q.repeat_seconds
        } else {
            now >= self.started && now - self.started >= q.limit_seconds
        }
    }
}
impl Book {
    pub fn guidance(q: &Profile, s: &State) -> String {
        if s.completed > 0 {
            return format!(
                "{} complete. Reward: {} experience. Repeat after {} hours.",
                q.title,
                q.xp,
                q.repeat_seconds / 3600
            );
        }
        let text = q.phases.get(s.phase.saturating_sub(1)).unwrap_or(&q.text);
        format!(
            "{text} Time limit: {} hours from acceptance.",
            q.limit_seconds / 3600
        )
    }
    /// Server selects the profile and reward; a client supplies only a visible
    /// object identity. Range, ownership, life and combat admission are external.
    pub fn converse(&self, entity: u32, level: u16, now: u64) -> io::Result<Conversation> {
        if now == 0 || level == 0 {
            return Err(invalid("Invalid quest conversation."));
        }
        for (id, state) in &self.0 {
            let q = profile(*id).ok_or_else(|| invalid("Unknown exploration quest."))?;
            if state.started == 0
                || now < state.started
                || now < state.completed
                || !(1..=q.phases.len().max(1)).contains(&state.phase)
            {
                return Err(invalid("Invalid exploration quest state or time."));
            }
        }
        let c = catalog();
        let mut next = self.clone();
        let mut xp = 0;
        let text;
        if entity == c.referral.npc {
            let q = &c.referral;
            match next.0.get_mut(&q.id) {
                Some(s) if s.active(q, now) => {
                    s.completed = now;
                    text = "You found the Arwic Mines Overseer. The New Arwic Mines lie northeast of town at 45.6N, 2.6W. The mining and crafting lesson is not available yet.".to_owned();
                }
                Some(s) if s.completed > 0 => text = Self::guidance(q, s),
                _ => text = "Speak to an Arwic, Esper or Lost Wish Explorer and finish their Obelisk Search before seeking this introduction.".to_owned(),
            }
        } else {
            let q = c
                .quests
                .iter()
                .find(|q| q.npc == entity || q.objectives.contains(&entity))
                .ok_or_else(|| invalid("This object has no exploration conversation."))?;
            if level < q.level {
                return Err(invalid("You do not meet this quest's level requirement."));
            }
            if entity == q.npc {
                let s = next.0.entry(q.id).or_insert(State {
                    phase: 1,
                    started: now,
                    completed: 0,
                });
                if s.restartable(q, now) {
                    *s = State {
                        phase: 1,
                        started: now,
                        completed: 0,
                    };
                }
                if s.active(q, now) && s.phase == 4 {
                    s.completed = now;
                    xp = q.xp;
                    text = format!(
                        "{} complete. You receive {} experience. My friend, the Arwic Mines Overseer at 44.0N, 6.1W, needs help.",
                        q.title, xp
                    );
                    let referral = &c.referral;
                    if next
                        .0
                        .get(&referral.id)
                        .is_none_or(|s| s.restartable(referral, now))
                    {
                        next.0.insert(
                            referral.id,
                            State {
                                phase: 1,
                                started: now,
                                completed: 0,
                            },
                        );
                    }
                } else {
                    text = Self::guidance(q, s);
                }
            } else {
                let s = next.0.get_mut(&q.id).ok_or_else(|| {
                    invalid("Speak to this region's Explorer before using its obelisks.")
                })?;
                if !s.active(q, now) {
                    return Err(invalid(
                        "This Obelisk Search is complete or expired. Return to its Explorer.",
                    ));
                }
                let objective = q.objectives.iter().position(|e| *e == entity).unwrap() + 1;
                if s.phase == objective {
                    s.phase += 1;
                }
                text = Self::guidance(q, s);
            }
        }
        Ok(Conversation {
            book: next,
            xp,
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_regions_require_order_and_return_to_the_correct_explorer() {
        for q in &catalog().quests {
            let mut book = Book::default();
            assert!(book.converse(q.objectives[0], 1, 100).is_err());
            book = book.converse(q.npc, 1, 100).unwrap().book;
            let skip = book.converse(q.objectives[2], 1, 101).unwrap();
            assert_eq!(skip.book, book);
            assert_eq!(skip.xp, 0);
            for (i, entity) in q.objectives.iter().enumerate() {
                let r = book.converse(*entity, 1, 102 + i as u64).unwrap();
                assert_eq!(r.xp, 0);
                book = r.book;
                assert_eq!(book.0[&q.id].phase, i + 2);
                assert_eq!(book.converse(*entity, 1, 105).unwrap().book, book);
            }
            let r = book.converse(q.npc, 1, 106).unwrap();
            assert_eq!(r.xp, 80);
            assert_eq!(r.book.0[&q.id].completed, 106);
            assert_eq!(r.book.converse(q.npc, 1, 107).unwrap().xp, 0);
            let referral = &catalog().referral;
            assert_eq!(r.book.0[&referral.id].completed, 0);
            let done = r.book.converse(referral.npc, 1, 108).unwrap();
            assert_eq!(done.xp, 0);
            assert_eq!(done.book.0[&referral.id].completed, 108);
        }
    }
    #[test]
    fn timers_expiry_and_other_regions_cannot_skip_objectives() {
        let q = &catalog().quests[0];
        let other = &catalog().quests[1];
        let b = Book::default().converse(q.npc, 1, 100).unwrap().book;
        assert!(b.converse(other.objectives[0], 1, 101).is_err());
        assert!(b.converse(q.objectives[0], 1, 99).is_err());
        assert!(
            b.converse(q.objectives[0], 1, 100 + q.limit_seconds)
                .is_err()
        );
        let b = b.converse(q.npc, 1, 100 + q.limit_seconds).unwrap().book;
        assert_eq!(b.0[&q.id].started, 100 + q.limit_seconds);
        let mut b = b;
        for entity in &q.objectives {
            b = b.converse(*entity, 1, 90000).unwrap().book;
        }
        b = b.converse(q.npc, 1, 90001).unwrap().book;
        assert_eq!(
            b.converse(q.npc, 1, 90001 + q.repeat_seconds - 1)
                .unwrap()
                .book,
            b
        );
        let repeat = b.converse(q.npc, 1, 90001 + q.repeat_seconds).unwrap();
        assert_eq!(repeat.xp, 0);
        assert_eq!(repeat.book.0[&q.id].phase, 1);
        assert_eq!(repeat.book.0[&q.id].completed, 0);
    }
}
