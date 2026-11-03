//! Reading the tables: TSV text (a generator's output, `db::import_tsv`) into rows, and rows
//! (from the TSVs or the database) into checked `Tables`.
use super::*;

fn rows<'a>(text: &'a str, name: &'a str, header: &str, columns: usize)
    -> io::Result<Vec<(usize, Vec<&'a str>)>> {
    let mut lines = text.lines().enumerate();
    let first = lines.next().map(|(_, l)| l.trim_start_matches('\u{feff}').trim_end_matches('\r'));
    if first != Some(header) {
        return Err(io::Error::other(format!("{name}:1: the header must be `{}`", header.replace('\t', " TAB "))));
    }
    let mut out = Vec::new();
    for (index, line) in lines {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') { continue; }
        let f: Vec<&str> = line.split('\t').map(str::trim).collect();
        if f.len() != columns {
            return Err(io::Error::other(format!("{name}:{}: {} columns, {columns} expected", index + 1, f.len())));
        }
        out.push((index + 1, f));
    }
    Ok(out)
}

fn hex32(v: &str) -> Option<u32> { u32::from_str_radix(v.trim_start_matches("0x").trim_start_matches("0X"), 16).ok() }
fn hex64(v: &str) -> Option<u64> { u64::from_str_radix(v.trim_start_matches("0x").trim_start_matches("0X"), 16).ok() }

/// A table row: where it came from (`file:line`, or the database row) and its fields in the
/// TSV column order.
pub type Row = (String, Vec<String>);

/// Every file's rows of one table, each named `file:line`.
pub(super) fn all_rows(files: &[(String, String)], header: &str, columns: usize) -> io::Result<Vec<Row>> {
    let mut out = Vec::new();
    for (name, text) in files {
        out.extend(rows(text, name, header, columns)?.into_iter()
            .map(|(line, f)| (format!("{name}:{line}"), f.into_iter().map(str::to_owned).collect())));
    }
    Ok(out)
}

impl Tables {
    /// Parses the four tables (`texts` in `FILES` order; `names` for the errors).
    pub fn parse(texts: [&str; 4], names: [&str; 4]) -> io::Result<Self> {
        Self::parse_files(std::array::from_fn(|i| vec![(names[i].to_owned(), texts[i].to_owned())]))
    }

    /// Parses the four tables, each from one or more files `(name, text)` (`FILES` order).
    pub fn parse_files(files: [Vec<(String, String)>; 4]) -> io::Result<Self> {
        Self::parse_rows([
            all_rows(&files[0], HEADERS[0], 12)?, all_rows(&files[1], HEADERS[1], 5)?,
            all_rows(&files[2], HEADERS[2], 11)?, all_rows(&files[3], HEADERS[3], 10)?,
        ])
    }

    /// Checks and builds the tables from their rows (`FILES` order, TSV column order). Rows come
    /// from the TSVs (`parse_files`) or the content database (`db::load`); a bad row's error names
    /// where it came from.
    pub fn parse_rows(tables: [Vec<Row>; 4]) -> io::Result<Self> {
        Self::parse_all(super::db::QuestRows { tables, ..Default::default() })
    }

    /// The same, with the database's aliases and named counters (`db::rows`).
    pub fn parse_all(mut rows: super::db::QuestRows) -> io::Result<Self> {
        let tables = std::mem::take(&mut rows.tables);
        let mut t = Tables::default();
        for (at, f) in &tables[0] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let id = hex32(f[1]).ok_or_else(|| bad("journal_id must be hex"))?;
            let title = (hex32(f[2]).ok_or_else(|| bad("title_table must be hex"))?, hex32(f[3]).ok_or_else(|| bad("title_key must be hex"))?);
            let goal = if f[5].is_empty() { 0 } else { f[5].parse().map_err(|_| bad("goal must be a whole number"))? };
            let credit = match f[6] { "killer" => Credit::Killer, "fellowship" => Credit::Fellowship,
                _ => return Err(bad("credit must be `killer` or `fellowship`")) };
            let min_level = if f[7].is_empty() { 0 } else { f[7].parse().map_err(|_| bad("min_level must be a whole number"))? };
            let positive = |v: &str, what: &str| if v.is_empty() { Ok(None) } else {
                v.parse::<u64>().ok().filter(|n| *n > 0).map(Some).ok_or_else(|| bad(&format!("{what} must be a positive whole number")))
            };
            let time_limit = positive(f[8], "time_limit_seconds")?;
            let max_solves = positive(f[9], "max_solves")?.map(|n| u16::try_from(n).map_err(|_| bad("max_solves is at most 65535"))).transpose()?;
            let repeat = positive(f[10], "repeat_seconds")?;
            if f[0].is_empty() || f[4].is_empty() || f[11].is_empty() { return Err(bad("quest, title and source must not be empty")); }
            let quest = Quest { journal_id: id, title, title_text: f[4].to_owned(), goal, credit, min_level, time_limit, max_solves, repeat };
            if t.keys.insert(f[0].to_owned(), id).is_some() || t.quests.insert(id, quest).is_some() {
                return Err(bad("duplicate quest"));
            }
        }
        for (at, f) in &tables[1] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let quest = *t.keys.get(f[0]).ok_or_else(|| bad("quest is not in the registry"))?;
            let stage: u8 = f[1].parse().map_err(|_| bad("stage must be 0 to 255"))?;
            let completed = match f[2] { "0" => false, "1" => true, _ => return Err(bad("completed must be 0 or 1")) };
            if f[3].is_empty() || f[4].is_empty() { return Err(bad("journal_text and source must not be empty")); }
            if t.stages.insert((quest, stage), (completed, f[3].to_owned())).is_some() { return Err(bad("duplicate stage")); }
        }
        let aliases = super::spec::aliases(&rows.aliases)?;
        t.instances(&rows, &aliases)?;
        t.items = super::items::overrides(&rows.items)?;
        let counters = super::spec::counters(&rows.counters, &t.keys)?;
        let entity_or_alias = |v: &str| hex32(v).or_else(|| aliases.get(v).filter(|(k, _)| *k == super::spec::AliasKind::Entity).and_then(|(_, v)| hex32(v)));
        let mut lines = BTreeMap::new();
        for (at, f) in &tables[2] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let trigger = Trigger::parse(f[3]).ok_or_else(|| bad(&format!("trigger must be one of {}",
                Trigger::NAMES.map(|(n, _)| n).join(", "))))?;
            // An owner is a DID or iid, an alias of one, or an `entities` / `objects` alias of several.
            use super::spec::AliasKind;
            let owner_entities: Vec<u32> = if f[1].is_empty() { Vec::new() } else {
                match aliases.get(f[1]) {
                    Some((AliasKind::Entities, list)) => list.split(',').filter_map(|v| hex32(v.trim())).collect(),
                    _ => vec![entity_or_alias(f[1]).filter(|e| e >> 24 == 0x47).ok_or_else(|| bad("owner_entity must be an EntityDesc DID, an entity alias or an entities alias"))?],
                }
            };
            let owner_iids: Vec<u64> = if f[2].is_empty() { Vec::new() } else {
                match aliases.get(f[2]) {
                    Some((AliasKind::Objects, list)) => list.split(',').filter_map(|v| hex64(v.trim())).collect(),
                    _ => vec![hex64(f[2]).ok_or_else(|| bad("owner_iid must be hex or an objects alias"))?],
                }
            };
            if matches!(trigger, Trigger::Crafted | Trigger::EscortLost | Trigger::EscortAbandoned | Trigger::UseItem) && (owner_entities.is_empty() || !owner_iids.is_empty()) {
                return Err(bad("a Crafted, EscortLost, EscortAbandoned or UseItem set names its entity in owner_entity (no owner_iid)"));
            }
            if matches!(trigger, Trigger::HoldDone | Trigger::HoldLost) && owner_entities.is_empty() && owner_iids.is_empty() {
                return Err(bad("a HoldDone or HoldLost set names the held creature: owner_iid or owner_entity"));
            }
            if trigger == Trigger::Enter && f[2].is_empty() {
                return Err(bad("an Enter set names its point: owner_iid, the world object it is around (range: the radius)"));
            }
            // The database's owner_slots rides after the TSV columns (`db::rows`).
            let owner_slots = f.get(11).filter(|v| !v.is_empty()).map(|v| v.to_string());
            if owner_slots.as_ref().is_some_and(|g| !t.groups.contains_key(g)) { return Err(bad("owner_slots is not an instance_group")); }
            let ownerless = trigger.branch() || trigger.personal();
            // A Bind or Travel set may name its lifestone or portal, or answer for any.
            let either = matches!(trigger, Trigger::Bind | Trigger::Travel);
            if !either && ownerless == (!owner_entities.is_empty() || !owner_iids.is_empty() || owner_slots.is_some()) {
                return Err(bad("a branch, Goto, PlayerDeath, QuestExpired, Logout or EnterWorld set has no owner; every other set names owner_entity, owner_iid or owner_slots"));
            }
            if trigger.personal() && trigger != Trigger::EnterWorld && f[5].is_empty() {
                return Err(bad("a PlayerDeath, QuestExpired or Logout set names its quest (and the stages it applies to)"));
            }
            // An Ask set's topic is its item (`topic_key`); other sets name no phrase.
            let phrase = f.get(12).filter(|v| !v.is_empty());
            if phrase.is_some() && trigger != Trigger::Ask { return Err(bad("only an Ask set names a phrase")); }
            if trigger == Trigger::Ask && !f[4].is_empty() { return Err(bad("an Ask set names its topic in phrase, not item")); }
            let item = if let Some(phrase) = phrase { Some(super::topic_key(phrase)) }
                else if f[4].is_empty() { None } else { Some(entity_or_alias(f[4]).ok_or_else(|| bad("item must be hex or an entity alias"))?) };
            let quest = if f[5].is_empty() { None } else { Some(*t.keys.get(f[5]).ok_or_else(|| bad("quest is not in the registry"))?) };
            let stage = |v: &str, d: u8| if v.is_empty() { Ok(d) } else { v.parse::<u8>().map_err(|_| bad("stages are 0 to 255")) };
            let stages = (stage(f[6], 0)?, stage(f[7], 255)?);
            if quest.is_none() && (f[6].len() + f[7].len()) > 0 { return Err(bad("a stage condition needs its quest")); }
            let range = if f[8].is_empty() { 4.0 } else { f[8].parse().ok().filter(|r: &f32| *r > 0.0).ok_or_else(|| bad("range must be positive metres"))? };
            let probability = if f[9].is_empty() { 1.0 } else { f[9].parse().ok().filter(|p: &f32| (0.0..=1.0).contains(p) && *p > 0.0).ok_or_else(|| bad("probability must be in (0, 1]"))? };
            if f[0].is_empty() || f[10].is_empty() { return Err(bad("set and source must not be empty")); }
            if lines.insert(f[0].to_owned(), at.clone()).is_some() { return Err(bad("duplicate set")); }
            t.sets.push(Set { id: f[0].to_owned(), owner_entities, owner_iids, trigger, item, quest, stages, range, probability, owner_slots });
        }
        let mut orders = BTreeMap::<String, Vec<(u32, String, (u32, Action))>>::new();
        for (at, f) in &tables[3] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            if !lines.contains_key(f[0]) { return Err(bad("set is not in emote-sets")); }
            let order: u32 = f[1].parse().map_err(|_| bad("order must be a whole number"))?;
            let delay: u32 = if f[3].is_empty() { 0 } else { f[3].parse().ok().filter(|d| *d <= 3_600_000).ok_or_else(|| bad("delay_ms must be 0 to 3600000 (an hour)"))? };
            if f[9].is_empty() { return Err(bad("source must not be empty")); }
            let names = super::spec::Names { quests: &t.keys, sets: &lines, aliases: &aliases, counters: &counters, groups: &t.groups, slots: &t.slots };
            let action = super::spec::parse(f[2], &f[4..9], &names).map_err(|why| bad(&why))?;
            if let Action::Goto(target) = &action { if target == f[0] { return Err(bad("a set cannot Goto itself")); } }
            orders.entry(f[0].to_owned()).or_default().push((order, at.clone(), (delay, action)));
        }
        for (set, mut list) in orders {
            list.sort_by_key(|(order, _, _)| *order);
            if let Some(w) = list.windows(2).find(|w| w[0].0 == w[1].0) {
                return Err(io::Error::other(format!("{}: duplicate order in set {set}", w[1].1)));
            }
            t.actions.insert(set, list.into_iter().map(|(_, _, a)| a).collect());
        }
        t.check_personal(&lines)?;
        t.quest_recipes(&rows.recipes, &aliases)?;
        Ok(t)
    }

}

/// The family files of the base table `table` in `directory` (`quest-registry-vault.tsv` for
/// `quest-registry.tsv`), in name order.
pub fn families(directory: &Path, table: &str) -> io::Result<Vec<std::path::PathBuf>> {
    let prefix = format!("{}-", table.trim_end_matches(".tsv"));
    let mut out = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| io::Error::other(format!("{}: {e}", directory.display())))? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with(&prefix) && name.ends_with(".tsv") && path.is_file() { out.push(path); }
    }
    out.sort();
    Ok(out)
}

impl Tables {
    /// A death, a logout or a run's expiry changes quests and talks only: no set they reach (by
    /// a branch or Goto) gives, takes, awards or moves.
    fn check_personal(&self, lines: &BTreeMap<String, String>) -> io::Result<()> {
        // A timed hold's end commits quest state and talk only, as a personal set does.
        for start in self.sets.iter().filter(|s| s.trigger.personal() || matches!(s.trigger, Trigger::Bind | Trigger::Travel | Trigger::HoldDone | Trigger::HoldLost)) {
            let mut seen = std::collections::BTreeSet::new();
            let mut todo = vec![start.id.as_str()];
            while let Some(set) = todo.pop() {
                if !seen.insert(set) { continue; }
                for (_, action) in self.actions.get(set).map_or(&[][..], Vec::as_slice) {
                    let refused = matches!(action, Action::GiveItem { .. } | Action::TakeItem { .. } | Action::RandomGive(_)
                        | Action::AwardXp(_) | Action::AwardLevelProportionalXp { .. } | Action::AwardGold(_) | Action::MoveTo(_)
                        | Action::StartEscort(_) | Action::Generate(_) | Action::StartHold { .. } | Action::MoveToLane(_) | Action::MoveToSlot(_) | Action::ClaimInstance { .. }
                        | Action::GenerateStep { .. } | Action::MoveToStep { .. } | Action::AwardXpShare { .. } | Action::AwardGoldShare { .. }
                        | Action::TakeQuantity { .. } | Action::GiveStack { .. } | Action::TakeGold(_) | Action::AwardQuestPoints(_)
                        | Action::AwardKingdomPoints { .. });
                    if refused {
                        let at = lines.get(start.id.as_str()).map_or("", String::as_str);
                        return Err(io::Error::other(format!("{at}: a {:?} set changes quests and talks only, but reaches {action:?} in set {set}",
                            start.trigger)));
                    }
                    match action {
                        Action::Goto(next) => todo.push(next),
                        Action::InqStage { success, failure, .. } | Action::InqQuestCount { success, failure, .. }
                        | Action::InqCounter { success, failure, .. } | Action::InqOwnsItems { success, failure, .. }
                        | Action::InqLevel { success, failure, .. } | Action::InqQuestReady { success, failure, .. }
                        | Action::InqSolved { success, failure, .. } | Action::InqQuestSolves { success, failure, .. }
                        | Action::InqStat { success, failure, .. } | Action::InqKingdom { success, failure, .. }
                        | Action::InqKingdomPoints { success, failure, .. } => todo.extend(success.iter().chain(failure).map(String::as_str)),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}

impl Tables {
    /// The instance groups, their lanes (each entry a placement `cell x y z heading` or a
    /// placement alias) and the spawn slots (one iid per lane of the slot's group).
    fn instances(&mut self, rows: &super::db::QuestRows, aliases: &BTreeMap<String, (super::spec::AliasKind, String)>) -> io::Result<()> {
        use super::instances::{Group, Slot, group_id};
        for (_, f) in &rows.groups { self.groups.insert(f[0].clone(), Group { id: group_id(&f[0]), lanes: BTreeMap::new() }); }
        for (at, f) in &rows.lanes {
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let text = aliases.get(&f[2]).filter(|(k, _)| *k == super::spec::AliasKind::Placement).map_or(f[2].as_str(), |(_, v)| v.as_str());
            let entry = super::spec::placement(text).ok_or_else(|| bad("entry must be `cell x y z heading` or a placement alias"))?;
            let lane: u8 = f[1].parse().map_err(|_| bad("lane must be 0 to 255"))?;
            self.groups.get_mut(&f[0]).ok_or_else(|| bad("instance_group is not in instance_group"))?.lanes.insert(lane, entry);
        }
        for (at, f) in &rows.slots {
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let lane: u8 = f[1].parse().map_err(|_| bad("lane must be 0 to 255"))?;
            if !self.groups.get(&f[2]).is_some_and(|g| g.lanes.contains_key(&lane)) { return Err(bad("the slot's group has no such lane")); }
            let iid = hex64(&f[3]).ok_or_else(|| bad("iid must be hex"))?;
            let arrival = if f[4].is_empty() { None } else {
                let text = aliases.get(&f[4]).filter(|(k, _)| *k == super::spec::AliasKind::Placement).map_or(f[4].as_str(), |(_, v)| v.as_str());
                Some(super::spec::placement(text).ok_or_else(|| bad("arrival must be `cell x y z heading` or a placement alias"))?)
            };
            let step = if f[5].is_empty() { None } else { Some(f[5].parse::<u8>().map_err(|_| bad("step must be 0 to 255"))?) };
            let slot = self.slots.entry(f[0].clone()).or_insert_with(|| Slot { group: f[2].clone(), iids: BTreeMap::new(), arrivals: BTreeMap::new(), step });
            if slot.group != f[2] { return Err(bad("a slot belongs to one instance group")); }
            if slot.step != step { return Err(bad("every lane of a slot has the same step")); }
            if self.slots.values().any(|s| s.iids.values().any(|i| *i == iid)) { return Err(bad("another slot already holds this iid")); }
            let slot = self.slots.get_mut(&f[0]).unwrap();
            slot.iids.insert(lane, iid);
            if let Some(arrival) = arrival { slot.arrivals.insert(lane, arrival); }
        }
        Ok(())
    }
}

impl Tables {
    /// The quest recipes (`quest_recipe`: recipe, ingredient, quantity, output, source), each made
    /// a hidden set (`craft`) that takes the ingredient and gives the output; and the recipes the
    /// quests teach.
    fn quest_recipes(&mut self, rows: &[Row], aliases: &BTreeMap<String, (super::spec::AliasKind, String)>) -> io::Result<()> {
        let entity = |v: &str| hex32(v).or_else(|| aliases.get(v).filter(|(k, _)| *k == super::spec::AliasKind::Entity).and_then(|(_, v)| hex32(v)))
            .filter(|e| e >> 24 == 0x47);
        for (at, f) in rows {
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let recipe = hex32(&f[0]).ok_or_else(|| bad("recipe must be hex"))?;
            let ingredient = entity(&f[1]).ok_or_else(|| bad("ingredient must be an EntityDesc DID or an entity alias"))?;
            let quantity: u32 = f[2].parse().map_err(|_| bad("quantity must be 1 to 65535"))?;
            let output = entity(&f[3]).ok_or_else(|| bad("output must be an EntityDesc DID or an entity alias"))?;
            self.recipes.insert(recipe, (ingredient, quantity, output));
            self.actions.insert(craft_set(recipe), vec![(0, Action::TakeQuantity { entity: ingredient, quantity }), (0, Action::GiveItem { entity: output, count: 1 })]);
        }
        self.taught = self.actions.values().flatten().filter_map(|(_, a)| if let Action::LearnRecipe(r) = a { Some(*r) } else { None }).collect();
        Ok(())
    }
}

/// The hidden set a quest recipe crafts through.
pub(super) fn craft_set(recipe: u32) -> String { format!("__craft_{recipe:08x}") }
