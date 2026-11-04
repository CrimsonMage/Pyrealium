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
        let mut lines = BTreeMap::new();
        for (at, f) in &tables[2] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            let trigger = Trigger::parse(f[3]).ok_or_else(|| bad(&format!("trigger must be one of {}",
                Trigger::NAMES.map(|(n, _)| n).join(", "))))?;
            let owner_entity = if f[1].is_empty() { None } else { Some(hex32(f[1]).filter(|e| e >> 24 == 0x47).ok_or_else(|| bad("owner_entity must be an EntityDesc DID"))?) };
            let owner_iid = if f[2].is_empty() { None } else { Some(hex64(f[2]).ok_or_else(|| bad("owner_iid must be hex"))?) };
            if matches!(trigger, Trigger::Crafted | Trigger::EscortLost | Trigger::EscortAbandoned) && (owner_entity.is_none() || owner_iid.is_some()) {
                return Err(bad("a Crafted, EscortLost or EscortAbandoned set names an EntityDesc in owner_entity (no owner_iid)"));
            }
            if trigger == Trigger::Enter && f[2].is_empty() {
                return Err(bad("an Enter set names its point: owner_iid, the world object it is around (range: the radius)"));
            }
            let ownerless = trigger.branch() || trigger.personal();
            if ownerless == (owner_entity.is_some() || owner_iid.is_some()) {
                return Err(bad("a branch, Goto, PlayerDeath, QuestExpired or Logout set has no owner; every other set names owner_entity or owner_iid"));
            }
            if trigger.personal() && f[5].is_empty() {
                return Err(bad("a PlayerDeath, QuestExpired or Logout set names its quest (and the stages it applies to)"));
            }
            let item = if f[4].is_empty() { None } else { Some(hex32(f[4]).ok_or_else(|| bad("item must be hex"))?) };
            let quest = if f[5].is_empty() { None } else { Some(*t.keys.get(f[5]).ok_or_else(|| bad("quest is not in the registry"))?) };
            let stage = |v: &str, d: u8| if v.is_empty() { Ok(d) } else { v.parse::<u8>().map_err(|_| bad("stages are 0 to 255")) };
            let stages = (stage(f[6], 0)?, stage(f[7], 255)?);
            if quest.is_none() && (f[6].len() + f[7].len()) > 0 { return Err(bad("a stage condition needs its quest")); }
            let range = if f[8].is_empty() { 4.0 } else { f[8].parse().ok().filter(|r: &f32| *r > 0.0).ok_or_else(|| bad("range must be positive metres"))? };
            let probability = if f[9].is_empty() { 1.0 } else { f[9].parse().ok().filter(|p: &f32| (0.0..=1.0).contains(p) && *p > 0.0).ok_or_else(|| bad("probability must be in (0, 1]"))? };
            if f[0].is_empty() || f[10].is_empty() { return Err(bad("set and source must not be empty")); }
            if lines.insert(f[0].to_owned(), at.clone()).is_some() { return Err(bad("duplicate set")); }
            t.sets.push(Set { id: f[0].to_owned(), owner_entity, owner_iid, trigger, item, quest, stages, range, probability });
        }
        let mut orders = BTreeMap::<String, Vec<(u32, String, (u32, Action))>>::new();
        for (at, f) in &tables[3] {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let bad = |why: &str| io::Error::other(format!("{at}: {why}"));
            if !lines.contains_key(f[0]) { return Err(bad("set is not in emote-sets")); }
            let order: u32 = f[1].parse().map_err(|_| bad("order must be a whole number"))?;
            let delay: u32 = if f[3].is_empty() { 0 } else { f[3].parse().ok().filter(|d| *d <= 3_600_000).ok_or_else(|| bad("delay_ms must be 0 to 3600000 (an hour)"))? };
            if f[9].is_empty() { return Err(bad("source must not be empty")); }
            let names = super::spec::Names { quests: &t.keys, sets: &lines };
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
        for start in self.sets.iter().filter(|s| s.trigger.personal()) {
            let mut seen = std::collections::BTreeSet::new();
            let mut todo = vec![start.id.as_str()];
            while let Some(set) = todo.pop() {
                if !seen.insert(set) { continue; }
                for (_, action) in self.actions.get(set).map_or(&[][..], Vec::as_slice) {
                    let refused = matches!(action, Action::GiveItem { .. } | Action::TakeItem { .. } | Action::RandomGive(_)
                        | Action::AwardXp(_) | Action::AwardLevelProportionalXp { .. } | Action::AwardGold(_) | Action::MoveTo(_)
                        | Action::StartEscort(_));
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
