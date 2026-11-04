//! Running the tables: which set answers a trigger, and what its actions do to one player's
//! quest book, bag and progression (an `Outcome` the caller commits as one record).
use super::*;

impl Tables {
    /// Every TakeItem's set and entity.
    pub fn takes(&self) -> impl Iterator<Item = (&str, u32)> {
        self.actions.iter().flat_map(|(set, list)| list.iter().filter_map(move |a| match a {
            (_, Action::TakeItem { entity, .. }) => Some((set.as_str(), *entity)), _ => None }))
    }
    /// Every GiveItem's set and entity, for the start's item-profile check.
    pub fn gives(&self) -> impl Iterator<Item = (&str, u32)> {
        self.actions.iter().flat_map(|(set, list)| list.iter().flat_map(move |(_, a)| match a {
            Action::GiveItem { entity, .. } => vec![(set.as_str(), *entity)],
            Action::RandomGive(pairs) => pairs.iter().map(|(e, _)| (set.as_str(), *e)).collect(),
            _ => Vec::new() }))
    }
    pub fn quest(&self, key: &str) -> Option<&Quest> { self.keys.get(key).and_then(|id| self.quests.get(id)) }
    pub fn quests(&self) -> impl Iterator<Item = &Quest> { self.quests.values() }
    /// Every quest's name (`quest.name`).
    pub fn quest_names(&self) -> impl Iterator<Item = &str> { self.keys.keys().map(String::as_str) }
    /// The journal line and finished flag of `quest` at `stage`.
    pub fn stage(&self, quest: u32, stage: u8) -> Option<(bool, &str)> {
        self.stages.get(&(quest, stage)).map(|(c, t)| (*c, t.as_str()))
    }

    fn owned_by(set: &Set, entity: u32, iid: u64) -> bool {
        set.owner_iid.is_none_or(|i| i == iid) && set.owner_entity.is_none_or(|e| e == entity)
    }
    /// The world objects Enter sets are around, with their radius: a player's position within
    /// it runs the set (`server.rs`, on each position update).
    pub fn enter_points(&self) -> impl Iterator<Item = (u64, f32)> + '_ {
        self.sets.iter().filter(|s| s.trigger == Trigger::Enter).filter_map(|s| s.owner_iid.map(|i| (i, s.range)))
    }
    /// Whether the object (entity, iid) has sets for `trigger` at all.
    pub fn reacts(&self, entity: u32, iid: u64, trigger: Trigger) -> bool {
        self.sets.iter().any(|s| s.trigger == trigger && Self::owned_by(s, entity, iid))
    }

    /// The quest's stage for a player at `now` (Unix seconds): its recorded stage, or 0 when an
    /// underway run is older than the quest's time limit (it may be started anew).
    pub fn stage_now(&self, book: &Book, quest: u32, now: u64) -> u8 {
        let stage = book.stage(quest);
        let underway = stage != 0 && !self.stage(quest, stage).is_some_and(|(done, _)| done);
        let started = book.timer(quest).started;
        match self.quests.get(&quest).and_then(|q| q.time_limit) {
            Some(limit) if underway && started != 0 && now > started.saturating_add(limit) => 0,
            _ => stage,
        }
    }
    /// Whether a new run of `quest` may start at `now` (its solve limit and repeat delay).
    pub fn ready(&self, book: &Book, quest: u32, now: u64) -> bool {
        let Some(q) = self.quests.get(&quest) else { return false };
        let t = book.timer(quest);
        q.max_solves.is_none_or(|max| t.solves < max)
            && (t.solves == 0 || q.repeat.is_none_or(|r| now >= t.completed.saturating_add(r)))
    }
    /// Whether `quest` has been solved: a recorded solve, or a completed stage (runs recorded
    /// before solves were).
    pub fn solved(&self, book: &Book, quest: u32) -> bool {
        book.timer(quest).solves > 0 || self.stage(quest, book.stage(quest)).is_some_and(|(done, _)| done)
    }
    fn level_ok(&self, progression: &crate::progression::Progression, quest: u32) -> bool {
        self.quests.get(&quest).is_none_or(|q| progression.level().number >= q.min_level)
    }

    #[allow(clippy::too_many_arguments)]
    fn find(&self, trigger: Trigger, entity: u32, iid: u64, item: Option<u32>, book: &Book, roll: f32, now: u64) -> Option<&Set> {
        self.sets.iter().find(|s| s.trigger == trigger && Self::owned_by(s, entity, iid)
            && s.item.is_none_or(|i| Some(i) == item)
            && s.quest.is_none_or(|q| (s.stages.0..=s.stages.1).contains(&self.stage_now(book, q, now)))
            && roll < s.probability)
    }
    /// Whether a set of the object (entity, iid) would answer `trigger` for this player at `now`
    /// (any probability): callers dispatch to `run` only then, so an object with no matching set
    /// keeps its other behaviour (a portal's route).
    pub fn answers(&self, trigger: Trigger, entity: u32, iid: u64, item: Option<u32>, book: &Book, now: u64) -> bool {
        self.find(trigger, entity, iid, item, book, 0.0, now).is_some()
    }
    /// Runs `trigger` on the object (entity, iid) for a player with this state at `now` (Unix
    /// seconds). The first set in file order whose owner, item and stage condition match and whose
    /// probability passes `roll` (0 to 1) runs; `None` when no set matches.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&self, trigger: Trigger, entity: u32, iid: u64, item: Option<u32>, book: &Book,
        inventory: &Inventory, progression: &crate::progression::Progression, first: u64, roll: f32, now: u64,
        make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Option<Outcome>> {
        let Some(set) = self.find(trigger, entity, iid, item, book, roll, now) else { return Ok(None) };
        let mut state = State { book: book.clone(), bag: inventory.clone(), progression: progression.clone(), next: first, now, roll, pause: 0,
            outcome: Outcome { set: set.id.clone(), range: set.range, ..Outcome::default() } };
        let frames = self.perform(&set.id, 0, false, &mut state, make, 0)?;
        state.paused(frames);
        Ok(Some(state.outcome))
    }

    /// The sets a personal trigger (PlayerDeath, QuestExpired, Logout) runs for a player at
    /// `now`: per quest, the first set whose stage condition holds (QuestExpired: only for a run
    /// past its time limit, on the stage it reached). Each runs on the quest book the previous
    /// left; they change quest state and talk only (`load` checks it), so the caller commits each.
    #[allow(clippy::too_many_arguments)]
    pub fn run_personal(&self, trigger: Trigger, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, now: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Vec<Outcome>> {
        let mut book = book.clone();
        let mut done = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        let picked: Vec<&Set> = self.sets.iter().filter(|s| s.trigger == trigger).filter(|s| {
            let Some(q) = s.quest else { return false };
            let stage = if trigger == Trigger::QuestExpired {
                if !self.expired(&book, q, now) { return false; }
                book.stage(q)
            } else { self.stage_now(&book, q, now) };
            (s.stages.0..=s.stages.1).contains(&stage) && done.insert(q)
        }).collect();
        for set in picked {
            let outcome = self.perform_named(&set.id, set.range, &book, inventory, progression, first, now, make)?;
            book = outcome.quests.iter().try_fold(book, |b, c| b.apply(c)).map_err(io::Error::other)?;
            out.push(outcome);
        }
        Ok(out)
    }
    /// Whether the player's run of `quest` is underway and past its time limit at `now`.
    pub fn expired(&self, book: &Book, quest: u32, now: u64) -> bool {
        let stage = book.stage(quest);
        stage != 0 && !self.stage(quest, stage).is_some_and(|(done, _)| done) && self.stage_now(book, quest, now) == 0
    }
    /// The quests that have QuestExpired sets (the tick looks only at these).
    pub fn timed_quests(&self) -> impl Iterator<Item = u32> + '_ {
        self.sets.iter().filter(|s| s.trigger == Trigger::QuestExpired).filter_map(|s| s.quest)
    }

    /// Carries on a set that paused for a delay (`Outcome::resume`), for the same player now. A
    /// set a live edit removed, or an action past its end, ends it quietly.
    #[allow(clippy::too_many_arguments)]
    pub fn resume(&self, resume: &Resume, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, roll: f32, now: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Outcome> {
        let set = resume.frames.first().map(|(s, _)| s.clone()).unwrap_or_default();
        let range = self.sets.iter().find(|s| s.id == set).map_or(4.0, |s| s.range);
        let mut state = State { book: book.clone(), bag: inventory.clone(), progression: progression.clone(), next: first, now, roll, pause: 0,
            outcome: Outcome { set, range, ..Outcome::default() } };
        let mut frames = resume.frames.clone();
        let mut waited = true;
        while !frames.is_empty() {
            let (set, start) = frames.remove(0);
            if let Some(mut inner) = self.perform(&set, start, waited, &mut state, make, 0)? {
                inner.extend(frames);
                state.paused(Some(inner));
                break;
            }
            waited = false;
        }
        Ok(state.outcome)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn perform_named(&self, set: &str, range: f32, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, now: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Outcome> {
        let mut state = State { book: book.clone(), bag: inventory.clone(), progression: progression.clone(), next: first, now, roll: 0.0, pause: 0,
            outcome: Outcome { set: set.to_owned(), range, ..Outcome::default() } };
        let frames = self.perform(set, 0, false, &mut state, make, 0)?;
        state.paused(frames);
        Ok(state.outcome)
    }
    /// Runs one set's actions directly at `now` (Unix seconds).
    #[allow(clippy::too_many_arguments)]
    pub fn perform_set(&self, set: &str, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, now: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Outcome> {
        if !self.actions.contains_key(set) { return Err(io::Error::other(format!("no emote set {set}"))); }
        let mut state = State { book: book.clone(), bag: inventory.clone(), progression: progression.clone(), next: first, now, roll: 0.0, pause: 0,
            outcome: Outcome { set: set.to_owned(), range: 4.0, ..Outcome::default() } };
        let frames = self.perform(set, 0, false, &mut state, make, 0)?;
        state.paused(frames);
        Ok(state.outcome)
    }
    /// Runs `set` from action `start` (`waited`: its delay has passed). Returns where it paused
    /// for a delay (the frames to carry on from, innermost first; the delay is `s.pause`).
    fn perform(&self, set: &str, start: usize, waited: bool, s: &mut State, make: &dyn Fn(u32, u64, u16) -> Option<Item>, depth: u8)
        -> io::Result<Option<Vec<(String, usize)>>> {
        // An NPC giving many quests falls from one offer to the next: up to 32 branches deep.
        if depth > 32 { return Err(io::Error::other("emote branches nest too deep")); }
        for (i, (delay, action)) in self.actions.get(set).map_or(&[][..], Vec::as_slice).iter().enumerate().skip(start) {
            if *delay > 0 && !(waited && i == start) {
                s.pause = *delay;
                return Ok(Some(vec![(set.to_owned(), i)]));
            }
            let branch = |ok: bool, success: &Option<String>, failure: &Option<String>| if ok { success.clone() } else { failure.clone() };
            match action {
                Action::Tell { table, key } => s.outcome.tells.push((*table, *key)),
                Action::Message(text) => s.outcome.lines.push(text.clone()),
                Action::AwardXp(xp) => s.change(crate::progression::Command::AwardXp(*xp))?,
                Action::AwardGold(gold) => s.change(crate::progression::Command::AwardGold(*gold))?,
                Action::AwardLevelProportionalXp { basis_points, min, max } => {
                    let span = s.progression.level_span().unwrap_or(0);
                    let xp = (span.saturating_mul(*basis_points) / 10_000).max(*min);
                    let xp = if *max == 0 { xp } else { xp.min(*max) };
                    if xp > 0 { s.change(crate::progression::Command::AwardXp(xp))?; }
                }
                Action::GiveItem { entity, count } => for _ in 0..*count {
                    let slot = s.bag.free_slot().ok_or_else(|| io::Error::other("Make room in your bag for the quest's rewards."))?;
                    let item = make(*entity, s.next, slot).ok_or_else(|| io::Error::other(format!("No item profile for {entity:08x}")))?;
                    s.next = s.next.checked_add(1).ok_or_else(|| io::Error::other("Item identity exhausted"))?;
                    s.item(I::Grant(item))?;
                    s.outcome.given.push(*entity);
                },
                Action::TakeItem { entity, count } => for _ in 0..*count {
                    let id = owned(&s.bag, *entity).ok_or_else(|| io::Error::other("You do not have the required quest item."))?;
                    s.item(I::ConsumeDense { item: id })?;
                },
                Action::EraseQuest { quest } => {
                    if s.book.stage(*quest) != 0 || s.book.timer(*quest).started != 0 { s.quest(Q::Erase { quest: *quest })?; }
                }
                Action::DecrementCounter { quest, counter } => {
                    let count = s.book.subcount(*quest, *counter);
                    if count > 0 { s.quest(Q::SubCount { quest: *quest, counter: *counter, count: count - 1 })?; }
                }
                Action::SetCounter { quest, counter, value } => {
                    if s.book.subcount(*quest, *counter) != *value { s.quest(Q::SubCount { quest: *quest, counter: *counter, count: *value })?; }
                }
                Action::InqOwnsItems { entity, count, success, failure } => {
                    let have: u64 = s.bag.items().filter(|i| i.definition == *entity && matches!(i.location, Location::Bag(_)))
                        .map(|i| u64::from(i.quantity.max(1))).sum();
                    if let Some(next) = branch(have >= u64::from(*count), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::InqQuestSolves { quest, min, max, success, failure } => {
                    let solves = s.book.timer(*quest).solves;
                    if let Some(next) = branch(solves >= *min && max.is_none_or(|m| solves <= m), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::InqStat { stat, min, max, success, failure } => {
                    let value = match stat {
                        Stat::Level => u32::from(s.progression.level().number), Stat::Gold => s.progression.gold,
                        Stat::Health => s.progression.health, Stat::Vigor => s.progression.vigor,
                        Stat::Kingdom => u32::from(s.book.kingdom().0), Stat::KingdomPoints => s.book.kingdom().1,
                    };
                    if let Some(next) = branch(value >= *min && max.is_none_or(|m| value <= m), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::Goto(next) => { if let Some(frames) = self.descend(next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                Action::SetStage { quest, stage } => {
                    let done = |st: u8| self.stage(*quest, st).is_some_and(|(d, _)| d);
                    let before = self.stage_now(&s.book, *quest, s.now);
                    let starts = *stage != 0 && !done(*stage) && (before == 0 || done(before));
                    if starts {
                        // The registry's rules hold whatever the rows checked first.
                        if !self.level_ok(&s.progression, *quest) {
                            return Err(io::Error::other("You are not experienced enough to begin this quest."));
                        }
                        if !self.ready(&s.book, *quest, s.now) {
                            return Err(io::Error::other("You cannot begin this quest again yet."));
                        }
                        s.quest(Q::Started { quest: *quest, at: s.now })?;
                    }
                    s.quest(Q::Stage { quest: *quest, stage: *stage })?;
                    if done(*stage) && !done(before) { s.quest(Q::Solved { quest: *quest, at: s.now })?; }
                    if let Some((_, text)) = self.stage(*quest, *stage) { s.outcome.lines.push(text.to_owned()); }
                }
                Action::IncrementQuest { quest } => {
                    let goal = self.quests.get(quest).map_or(0, |q| q.goal);
                    let count = s.book.count(*quest);
                    if count < goal { s.quest(Q::Count { quest: *quest, count: count + 1 })?; }
                }
                Action::InqStage { quest, min, max, success, failure } => {
                    if let Some(next) = branch((*min..=*max).contains(&self.stage_now(&s.book, *quest, s.now)), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::InqQuestCount { quest, count, success, failure } => {
                    if let Some(next) = branch(s.book.count(*quest) >= *count, success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::InqLevel { quest, success, failure } => {
                    if let Some(next) = branch(self.level_ok(&s.progression, *quest), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::InqQuestReady { quest, success, failure } => {
                    if let Some(next) = branch(self.ready(&s.book, *quest, s.now), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::InqSolved { quest, success, failure } => {
                    if let Some(next) = branch(self.solved(&s.book, *quest), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::MoveTo(placement) => s.outcome.moveto = Some(*placement),
                Action::RandomGive(pairs) => {
                    let total: u64 = pairs.iter().map(|(_, w)| u64::from(*w)).sum();
                    let pick = ((f64::from(s.roll.clamp(0.0, 0.999_999)) * total as f64) as u64).min(total - 1);
                    let mut acc = 0u64;
                    let entity = pairs.iter().find(|(_, w)| { acc += u64::from(*w); pick < acc }).map(|(e, _)| *e).unwrap_or(pairs[0].0);
                    let slot = s.bag.free_slot().ok_or_else(|| io::Error::other("Make room in your bag for the quest's rewards."))?;
                    let item = make(entity, s.next, slot).ok_or_else(|| io::Error::other(format!("No item profile for {entity:08x}")))?;
                    s.next = s.next.checked_add(1).ok_or_else(|| io::Error::other("Item identity exhausted"))?;
                    s.item(I::Grant(item))?;
                    s.outcome.given.push(entity);
                }
                Action::IncrementCounter { quest, counter, cap } => {
                    let count = s.book.subcount(*quest, *counter);
                    if count < *cap { s.quest(Q::SubCount { quest: *quest, counter: *counter, count: count + 1 })?; }
                }
                Action::InqCounter { quest, counter, count, success, failure } => {
                    if let Some(next) = branch(s.book.subcount(*quest, *counter) >= *count, success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::StartEscort(e) => s.outcome.escort = Some(*e),
                Action::EndEscort => s.outcome.end_escort = true,
                Action::SetKingdom(k) => { if s.book.kingdom().0 != *k { s.quest(Q::Kingdom(*k))?; } }
                Action::InqKingdom { kingdom, success, failure } => {
                    let mine = s.book.kingdom().0;
                    if let Some(next) = branch(mine != 0 && (*kingdom == 0 || mine == *kingdom), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::AwardKingdomPoints { points, max } => {
                    let have = s.book.kingdom().1;
                    let next = if *max == 0 { have.saturating_add(*points) } else { have.saturating_add(*points).min((*max).max(have)) };
                    if next != have { s.quest(Q::KingdomPoints(next))?; }
                }
                Action::InqKingdomPoints { points, success, failure } => {
                    if let Some(next) = branch(s.book.kingdom().1 >= *points, success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::ApplyEffect { effect, seconds } => {
                    s.quest(Q::EffectOn { effect: *effect, at: s.now, until: s.now.saturating_add(*seconds) })?;
                }
                Action::RemoveEffect { effect } => {
                    if s.book.quest_effects(s.now).iter().any(|(e, _)| e == effect) { s.quest(Q::EffectOff { effect: *effect })?; }
                }
            }
        }
        Ok(None)
    }
    fn descend(&self, next: &str, set: &str, i: usize, s: &mut State, make: &dyn Fn(u32, u64, u16) -> Option<Item>, depth: u8)
        -> io::Result<Option<Vec<(String, usize)>>> {
        Ok(self.perform(next, 0, false, s, make, depth + 1)?.map(|mut frames| { frames.push((set.to_owned(), i + 1)); frames }))
    }
}

struct State { book: Book, bag: Inventory, progression: crate::progression::Progression, next: u64, now: u64, roll: f32, pause: u32, outcome: Outcome }
impl State {
    fn paused(&mut self, frames: Option<Vec<(String, usize)>>) {
        self.outcome.resume = frames.map(|frames| Resume { frames, after_ms: self.pause });
    }
    fn change(&mut self, change: crate::progression::Command) -> io::Result<()> {
        self.progression = self.progression.apply(&change).map_err(io::Error::other)?;
        self.outcome.changes.push(change);
        Ok(())
    }
    fn item(&mut self, command: I) -> io::Result<()> {
        self.bag = self.bag.apply(&command).map_err(io::Error::other)?;
        self.outcome.items.push(command);
        Ok(())
    }
    fn quest(&mut self, command: Q) -> io::Result<()> {
        self.book = self.book.apply(&command).map_err(io::Error::other)?;
        self.outcome.quests.push(command);
        Ok(())
    }
}


/// The first bag item of `entity`.
pub fn owned(bag: &Inventory, entity: u32) -> Option<u64> {
    bag.items().find(|i| i.definition == entity && matches!(i.location, Location::Bag(_))).map(|i| i.id)
}

