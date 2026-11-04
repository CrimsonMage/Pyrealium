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
            Action::GiveItem { entity, .. } | Action::GiveStack { entity, .. } => vec![(set.as_str(), *entity)],
            Action::RandomGive(pairs) => pairs.iter().map(|(e, _)| (set.as_str(), *e)).collect(),
            _ => Vec::new() }))
    }
    pub fn quest(&self, key: &str) -> Option<&Quest> { self.keys.get(key).and_then(|id| self.quests.get(id)) }
    pub fn quests(&self) -> impl Iterator<Item = &Quest> { self.quests.values() }
    /// A quest recipe (`quest_recipe`): its ingredient, quantity and output.
    pub fn quest_recipe(&self, recipe: u32) -> Option<(u32, u32, u32)> { self.recipes.get(&recipe).copied() }
    /// Whether the player may craft `recipe`: one a quest teaches only once learned.
    pub fn recipe_allowed(&self, book: &Book, recipe: u32) -> bool { !self.taught.contains(&recipe) || book.knows(recipe) }
    /// Crafts a quest recipe: takes its ingredient, gives its output (the craft's record).
    #[allow(clippy::too_many_arguments)]
    pub fn craft(&self, recipe: u32, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, now: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> io::Result<Outcome> {
        if !self.recipes.contains_key(&recipe) { return Err(io::Error::other("Not a quest recipe.")); }
        if !self.recipe_allowed(book, recipe) { return Err(io::Error::other("You have not learned this recipe yet.")); }
        self.perform_named(&super::load::craft_set(recipe), 4.0, book, inventory, progression, first, now, make)
    }
    /// The quest a UseItem set of the item starts (an elixir), for the item's examination.
    pub fn started_by_item(&self, entity: u32) -> Option<&Quest> {
        let quest = self.sets.iter().find(|s| s.trigger == Trigger::UseItem && s.owner_entities.contains(&entity) && s.stages.0 == 0)?.quest?;
        self.quests.get(&quest)
    }
    /// Every quest's name (`quest.name`).
    pub fn quest_names(&self) -> impl Iterator<Item = &str> { self.keys.keys().map(String::as_str) }
    /// The journal line and finished flag of `quest` at `stage`.
    pub fn stage(&self, quest: u32, stage: u8) -> Option<(bool, &str)> {
        self.stages.get(&(quest, stage)).map(|(c, t)| (*c, t.as_str()))
    }

    fn owned_by(&self, set: &Set, entity: u32, iid: u64) -> bool {
        (set.owner_iids.is_empty() || set.owner_iids.contains(&iid)) && (set.owner_entities.is_empty() || set.owner_entities.contains(&entity))
            && set.owner_slots.as_ref().is_none_or(|g| self.slots.values().any(|sl| sl.group == *g && sl.iids.values().any(|i| *i == iid)))
    }
    /// The world objects Enter sets are around, with their radius: a player's position within
    /// it runs the set (`server.rs`, on each position update).
    pub fn enter_points(&self) -> impl Iterator<Item = (u64, f32)> + '_ {
        self.sets.iter().filter(|s| s.trigger == Trigger::Enter).flat_map(|s| s.owner_iids.iter().map(move |i| (*i, s.range)))
    }
    /// Whether the object (entity, iid) has sets for `trigger` at all.
    pub fn reacts(&self, entity: u32, iid: u64, trigger: Trigger) -> bool {
        self.sets.iter().any(|s| s.trigger == trigger && self.owned_by(s, entity, iid))
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
        self.sets.iter().find(|s| s.trigger == trigger && self.owned_by(s, entity, iid)
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
            // A set with no quest (an EnterWorld set for everyone) runs every time.
            let Some(q) = s.quest else { return true };
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
    /// A kill for the killer: every quest's first Kill set that answers the creature runs (one
    /// kill counts for each quest that wants it, as ACE's kill tasks do), each on the book the
    /// previous left, in priority order; a set without a quest runs once. A set that refuses (a
    /// quest that cannot start yet) is left out and the others still run. The outcomes merge
    /// into one (the kill's record); `None` when no set ran.
    #[allow(clippy::too_many_arguments)]
    pub fn run_kill(&self, entity: u32, iid: u64, book: &Book, inventory: &Inventory, progression: &crate::progression::Progression,
        first: u64, now: u64, make: &dyn Fn(u32, u64, u16) -> Option<Item>) -> Option<Outcome> {
        let (mut book, mut bag, mut progression) = (book.clone(), inventory.clone(), progression.clone());
        let mut seen = std::collections::BTreeSet::new();
        let mut merged: Option<Outcome> = None;
        let mut next = first;
        let candidates: Vec<&Set> = self.sets.iter().filter(|s| s.trigger == Trigger::Kill && self.owned_by(s, entity, iid)).collect();
        for set in candidates {
            if !seen.insert(set.quest) { continue; }
            if !set.quest.is_none_or(|q| (set.stages.0..=set.stages.1).contains(&self.stage_now(&book, q, now))) { seen.remove(&set.quest); continue; }
            let Ok(o) = self.perform_named(&set.id, set.range, &book, &bag, &progression, next, now, make) else { continue };
            book = o.quests.iter().try_fold(book, |b, c| b.apply(c)).ok()?;
            bag = o.items.iter().try_fold(bag, |b, c| b.apply(c)).ok()?;
            progression = o.changes.iter().try_fold(progression, |p, c| p.apply(c)).ok()?;
            next = bag.high_water().max(next);
            match merged.as_mut() {
                None => merged = Some(o),
                Some(m) => {
                    m.tells.extend(o.tells); m.lines.extend(o.lines); m.quests.extend(o.quests); m.items.extend(o.items);
                    m.changes.extend(o.changes); m.given.extend(o.given); m.world.extend(o.world); m.says.extend(o.says);
                    m.claims.extend(o.claims); m.releases.extend(o.releases);
                    if m.resume.is_none() { m.resume = o.resume; }
                    if m.moveto.is_none() { m.moveto = o.moveto; }
                }
            }
        }
        merged
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
                Action::TakeGold(gold) => {
                    if s.progression.gold < *gold { return Err(io::Error::other("You do not have enough coins.")); }
                    s.change(crate::progression::Command::SpendGold(*gold))?;
                }
                Action::AwardLevelProportionalXp { basis_points, min, max } => {
                    // At the last level (no span) a capped reward pays its cap.
                    let Some(span) = s.progression.level_span() else {
                        if *max > 0 { s.change(crate::progression::Command::AwardXp(*max))?; }
                        continue;
                    };
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
                        Stat::QuestPoints => s.book.quest_points(),
                    };
                    if let Some(next) = branch(value >= *min && max.is_none_or(|m| value <= m), success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::ClaimInstance { group, success, failure } => {
                    let ok = match self.groups.get(group) {
                        Some(g) if s.book.lane(g.id).is_some() => true,
                        Some(g) => match super::instances::free_lane(g, &s.outcome.claims) {
                            Some(lane) => { s.quest(Q::Lane { group: g.id, lane })?; s.outcome.claims.push((g.id, lane)); true }
                            None => false,
                        },
                        None => false,
                    };
                    if let Some(next) = branch(ok, success, failure) { if let Some(frames) = self.descend(&next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                }
                Action::ReleaseInstance(group) => {
                    if let Some((id, lane)) = self.groups.get(group).and_then(|g| s.book.lane(g.id).map(|l| (g.id, l))) {
                        self.dismiss_lane(group, lane, s);
                        s.quest(Q::LaneOff { group: id })?;
                        s.outcome.releases.push((id, lane));
                    }
                }
                Action::DismissSlots(group) => {
                    if let Some(lane) = self.groups.get(group).and_then(|g| s.book.lane(g.id)) { self.dismiss_lane(group, lane, s); }
                }
                Action::Generate(slot) => {
                    let iid = self.slots.get(slot).and_then(|sl| {
                        let lane = self.groups.get(&sl.group).and_then(|g| s.book.lane(g.id))?;
                        sl.iids.get(&lane).copied()
                    });
                    if let Some(iid) = iid { s.outcome.world.push(super::instances::WorldOp::Generate(iid)); }
                }
                Action::MoveToLane(group) => {
                    if let Some(entry) = self.groups.get(group).and_then(|g| g.lanes.get(&s.book.lane(g.id)?)) { s.outcome.moveto = Some(*entry); }
                }
                Action::MoveToSlot(slot) => {
                    let to = self.slots.get(slot).and_then(|sl| sl.arrivals.get(&self.groups.get(&sl.group).and_then(|g| s.book.lane(g.id))?));
                    if let Some(to) = to { s.outcome.moveto = Some(*to); }
                }
                Action::Say { text, speaker } => s.outcome.says.push((speaker.clone(), text.clone())),
                Action::GiveStack { entity, quantity } => {
                    let mut item = make(*entity, s.next, 0).ok_or_else(|| io::Error::other(format!("No item profile for {entity:08x}")))?;
                    let limit = item.stack_limit.max(*quantity);
                    // Merged into a stack the bag holds when the sum fits; else a new stack.
                    let merge = s.bag.items().find(|i| i.definition == *entity && i.roll.is_none() && matches!(i.location, Location::Bag(_))
                        && i.quantity.saturating_add(*quantity) <= limit).cloned();
                    let mut amount = *quantity;
                    if let Some(old) = merge {
                        amount += old.quantity;
                        s.item(I::ConsumeStackDense { item: old.id, quantity: old.quantity })?;
                    }
                    let slot = s.bag.free_slot().ok_or_else(|| io::Error::other("Make room in your bag for the quest's rewards."))?;
                    item.location = Location::Bag(slot);
                    item.quantity = amount;
                    item.stack_limit = limit;
                    s.next = s.next.checked_add(1).ok_or_else(|| io::Error::other("Item identity exhausted"))?;
                    s.item(I::Grant(item))?;
                    s.outcome.given.push(*entity);
                }
                Action::TakeQuantity { entity, quantity } => {
                    let mut stacks: Vec<Item> = s.bag.items().filter(|i| i.definition == *entity && matches!(i.location, Location::Bag(_))).cloned().collect();
                    stacks.sort_by_key(|i| i.id);
                    if stacks.iter().map(|i| u64::from(i.quantity.max(1))).sum::<u64>() < u64::from(*quantity) {
                        return Err(io::Error::other("You do not have enough of the required quest item."));
                    }
                    let (mut need, mut rest) = (*quantity, None);
                    for stack in stacks {
                        if need == 0 { break; }
                        let have = stack.quantity.max(1);
                        let used = need.min(have);
                        need -= used;
                        s.item(I::ConsumeStackDense { item: stack.id, quantity: stack.quantity })?;
                        if have > used { rest = Some((stack, have - used)); }
                    }
                    // What a partly used stack keeps goes back as a stack of its own.
                    if let Some((mut stack, left)) = rest {
                        let slot = s.bag.free_slot().ok_or_else(|| io::Error::other("Make room in your bag."))?;
                        stack.id = s.next;
                        s.next = s.next.checked_add(1).ok_or_else(|| io::Error::other("Item identity exhausted"))?;
                        stack.quantity = left;
                        stack.location = Location::Bag(slot);
                        s.item(I::Grant(stack))?;
                    }
                }
                Action::GrantScene(scene) => { if !s.book.scenes().any(|x| x == *scene) { s.quest(Q::Scene(*scene))?; } }
                Action::LearnRecipe(recipe) => { if !s.book.knows(*recipe) { s.quest(Q::Recipe(*recipe))?; } }
                Action::Goto(next) => { if let Some(frames) = self.descend(next, set, i, s, make, depth)? { return Ok(Some(frames)); } }
                Action::SetStage { quest, stage } => self.set_stage(*quest, *stage, s)?,
                Action::IncrementStage { quest } => {
                    let next = self.stage_now(&s.book, *quest, s.now).saturating_add(1);
                    self.set_stage(*quest, next, s)?;
                }
                Action::GenerateStep { group, quest } => {
                    if let Some(iid) = self.step_slot(group, *quest, s).and_then(|(sl, lane)| sl.iids.get(&lane).copied()) {
                        s.outcome.world.push(super::instances::WorldOp::Generate(iid));
                    }
                }
                Action::MoveToStep { group, quest } => {
                    if let Some(to) = self.step_slot(group, *quest, s).and_then(|(sl, lane)| sl.arrivals.get(&lane).copied()) {
                        s.outcome.moveto = Some(to);
                    }
                }
                Action::AwardXpShare { basis_points, quest, part, whole } => {
                    let (part, whole) = (u64::from(s.book.subcount(*quest, *part)), u64::from(s.book.subcount(*quest, *whole)));
                    let span = s.progression.level_span().unwrap_or(0);
                    let xp = span.saturating_mul(*basis_points).saturating_mul(part.min(whole)).checked_div(10_000 * whole).unwrap_or(0);
                    if xp > 0 { s.change(crate::progression::Command::AwardXp(xp))?; }
                }
                Action::AwardGoldShare { gold, quest, part, whole } => {
                    let (part, whole) = (u64::from(s.book.subcount(*quest, *part)), u64::from(s.book.subcount(*quest, *whole)));
                    let paid = (u64::from(*gold) * part.min(whole)).checked_div(whole).unwrap_or(0);
                    if paid > 0 { s.change(crate::progression::Command::AwardGold(paid as u32))?; }
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
                Action::AwardQuestPoints(points) => {
                    let have = s.book.quest_points();
                    s.quest(Q::QuestPoints(have.saturating_add(*points)))?;
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
    /// Moves `quest` to `stage` for the player: a start (leaving stage 0 or a completed stage) is
    /// held to the registry's level, repeat and solve rules; a completed stage solves it; the
    /// stage's journal line is said.
    fn set_stage(&self, quest: u32, stage: u8, s: &mut State) -> io::Result<()> {
        let done = |st: u8| self.stage(quest, st).is_some_and(|(d, _)| d);
        let before = self.stage_now(&s.book, quest, s.now);
        let starts = stage != 0 && !done(stage) && (before == 0 || done(before));
        if starts {
            // The registry's rules hold whatever the rows checked first.
            if !self.level_ok(&s.progression, quest) {
                return Err(io::Error::other("You are not experienced enough to begin this quest."));
            }
            if !self.ready(&s.book, quest, s.now) {
                return Err(io::Error::other("You cannot begin this quest again yet."));
            }
            s.quest(Q::Started { quest, at: s.now })?;
        }
        s.quest(Q::Stage { quest, stage })?;
        if done(stage) && !done(before) { s.quest(Q::Solved { quest, at: s.now })?; }
        if let Some((_, text)) = self.stage(quest, stage) { s.outcome.lines.push(text.to_owned()); }
        Ok(())
    }
    /// The group's slot whose step is `quest`'s current stage, and the player's lane of the group.
    fn step_slot(&self, group: &str, quest: u32, s: &State) -> Option<(&super::instances::Slot, u8)> {
        let lane = s.book.lane(self.groups.get(group)?.id)?;
        let stage = self.stage_now(&s.book, quest, s.now);
        self.slots.values().find(|sl| sl.group == group && sl.step == Some(stage)).map(|sl| (sl, lane))
    }
    /// Sends away the creatures of `group`'s slots in `lane`.
    fn dismiss_lane(&self, group: &str, lane: u8, s: &mut State) {
        for slot in self.slots.values().filter(|sl| sl.group == group) {
            if let Some(iid) = slot.iids.get(&lane) { s.outcome.world.push(super::instances::WorldOp::Dismiss(*iid)); }
        }
    }
    /// Every spawn slot's creature (the encounters hold them until summoned).
    pub fn slot_iids(&self) -> impl Iterator<Item = u64> + '_ { self.slots.values().flat_map(|s| s.iids.values().copied()) }

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

