//! Saved state of the quests that were code before they were data (`quest_emotes`): the types
//! older characters' quest books still hold, and the move of that state onto the data quest of
//! the same journal id at start (`commands`, run by `Store::migrate_legacy_quests`). Nothing here
//! runs a quest; once moved, a character's legacy entry is gone.
use std::collections::BTreeMap;

use crate::quests::{Book, Command as Q};

/// A hunt's saved run (the hunting catalog's state).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HuntState { pub count: u16, pub started: u64, pub completed: u64, pub rewarded_xp: u64 }
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hunts(pub BTreeMap<u32, HuntState>);

/// The quest's completed stage (the highest one marked completed).
fn done_stage(tables: &crate::quest_emotes::Tables, quest: u32) -> Option<u8> {
    (0..=255u8).rev().find(|s| tables.stage(quest, *s).is_some_and(|(done, _)| done))
}

/// A run moved onto the data quest `quest`: its start, then its stage. A data quest whose stages
/// count the kills (one stage per kill, as the AC2 Vault rows: 1 + kills, then the report stage
/// and the completed one) takes the kill count as its stage; one with only an underway, goal-met
/// and completed stage takes the count as its counter.
fn run(tables: &crate::quest_emotes::Tables, quest: u32, count: u16, started: u64, completed: u64) -> Option<Vec<Q>> {
    let goal = tables.quests().find(|q| q.journal_id == quest)?.goal;
    let done = done_stage(tables, quest)?;
    let mut out = vec![Q::Started { quest, at: started }];
    if completed > 0 {
        out.extend([Q::Stage { quest, stage: done }, Q::Solved { quest, at: completed }]);
    } else if done > 3 {
        // A stage per kill: 1 + kills, at most the report stage (done - 1).
        out.push(Q::Stage { quest, stage: 1 + u8::try_from(count).unwrap_or(u8::MAX).min(done - 2) });
    } else {
        out.extend([Q::Stage { quest, stage: if count >= goal && goal > 0 { 2 } else { 1 } }, Q::Count { quest, count: count.min(goal) }]);
    }
    Some(out)
}

/// The commands that move a book's legacy quest state onto the data quests and drop it. State
/// of a quest the tables do not have stays (and is moved once they do).
pub fn commands(book: &Book, tables: &crate::quest_emotes::Tables) -> Vec<Q> {
    let mut out = Vec::new();
    for (quest, state) in &book.hunting.0 {
        let Some(moved) = run(tables, *quest, state.count, state.started, state.completed) else { continue };
        out.extend(moved);
        out.push(Q::RetireHunt { quest: *quest });
    }
    out
}
