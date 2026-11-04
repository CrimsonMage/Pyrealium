//! Characters and ground items, held in memory and saved to the SQLite database (`db::shard`).
//!
//! Every mutator validates a candidate state and publishes it in memory, marking the character
//! dirty; nothing touches the database on the game loop. `save` (called periodically, at logoff
//! and at shutdown) writes every dirty character and, when they changed, the ground items as one
//! transaction on the database thread, so a crash rolls the whole world back to the last save
//! together: an item can never be both dropped and kept, or traded and kept. Character creation,
//! deletion and restoration are saved immediately (`save_now`).
use crate::db::{self, shard};
use crate::movement::Placement;
use crate::world::CharacterCreationRequest;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

mod drops;
pub use drops::DroppedItem;

#[derive(Debug, Clone, PartialEq)]
pub struct Character {
    pub id: u64,
    pub account: String,
    pub name: String,
    /// Exact ordered client appearance values; None for legacy CLI-created characters.
    pub creation: Option<CharacterCreationRequest>,
    pub deleted: bool,
    /// Durable avatar lifetime; never reused by reconnecting transports.
    pub instance: u16,
    pub ui: Option<crate::preferences::UiSettings>,
    pub hotbar: crate::preferences::Hotbar,
    pub gameplay_options: Option<crate::preferences::GameplayOptions>,
    /// The chat options page's tables (`preferences::ChatOptions`), once uploaded.
    pub chat_options: Option<crate::preferences::ChatOptions>,
    pub inventory: crate::inventory::Inventory,
    pub progression: crate::progression::Progression,
    pub skills: crate::skills::Book,
    pub professions: crate::professions::Book,
    pub quests: crate::quests::Book,
    pub training_kit: bool,
    pub training_supplies: bool,
    pub starter_gear: bool,
    pub binding: Option<Placement>,
    /// Not saved: the world-clock second each skill was last used since the server
    /// started, sent as SkillData's last-use stamp (`ui::skill_object`).
    pub skill_used: BTreeMap<u32, f64>,
}

/// Default time between periodic saves of online characters (five minutes).
pub const SAVE_INTERVAL: Duration = Duration::from_secs(300);

pub struct Store {
    writer: db::Writer,
    pub characters: BTreeMap<u64, Character>,
    pub drops: BTreeMap<u64, DroppedItem>,
    positions: BTreeMap<u64, Placement>,
    /// Tradeskill craft experience and ranks per character (`tradeskills.rs`), one line each.
    trade: BTreeMap<u64, String>,
    /// Characters changed since their last queued save.
    dirty: BTreeSet<u64>,
    ground_dirty: bool,
    /// Saves the database thread could not commit report back here and are retried.
    failed_tx: Sender<(Vec<u64>, bool)>,
    failed: Receiver<(Vec<u64>, bool)>,
    last_save: Instant,
    interval: Duration,
    /// Tests: simulate a crash (no final save on drop).
    abandoned: bool,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b' ' || b == b'-')
}

/// A character name the server stores: 1 to 32 UTF-16 units, each an ASCII letter or digit,
/// `_`, space, `-`, or a character the client's name field takes in any of its languages
/// (`gmCharNameCharacterValidator::IsCharacterValid` / `IsCharacterValidForTransitions` and
/// their `LanguageLetters` table, Legions 0x005CD990 / 0x005CDA50 / 0x00A3B7D0): `'`, the French
/// accents (e, a, c and retail's U-acute), the German umlauts, the Hangul syllables
/// AC00-D7A3 and compatibility jamo 3131-3163, and the CJK range 3400-9FB0. The server does not
/// know the client's language, so it takes the union. What the retail server checked is not
/// known (Pyrealium policy); digits and `_` stay accepted for names made before this rule.
fn valid_character_name(s: &str) -> bool {
    const LETTERS: &[char] = &[
        '\'', 'è', 'È', 'é', 'É', 'à', 'À', 'ç', 'Ç', 'ù', 'Ú', 'ü', 'Ü', 'ö', 'Ö', 'ä', 'Ä',
    ];
    let units = s.encode_utf16().count();
    (1..=32).contains(&units)
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | ' ' | '-')
                || LETTERS.contains(&c)
                || matches!(u32::from(c), 0xAC00..=0xD7A3 | 0x3131..=0x3163 | 0x3400..=0x9FB0)
        })
}

impl Store {
    /// Connects (creating and migrating the database when needed), takes the server lock and
    /// loads every saved character and ground item.
    pub fn open(database: &db::Database) -> io::Result<Self> {
        let writer = db::Writer::start(database)?;
        let (rows, ground) = writer.call(shard::load)?;
        let mut characters = BTreeMap::new();
        let mut positions = BTreeMap::new();
        let mut trade = BTreeMap::new();
        for row in rows {
            let id = row.id;
            let (character, placement, line) =
                from_row(row).map_err(|e| invalid(&format!("saved character {id}: {e}")))?;
            if let Some(placement) = placement {
                positions.insert(id, placement);
            }
            if let Some(line) = line {
                trade.insert(id, line);
            }
            characters.insert(id, character);
        }
        let mut drops = BTreeMap::new();
        for d in ground {
            if drops.insert(d.id(), d).is_some() || drops.len() > drops::MAX_DROPS {
                return Err(invalid("duplicate or too many saved ground items"));
            }
        }
        let (failed_tx, failed) = mpsc::channel();
        let store = Self {
            writer,
            characters,
            drops,
            positions,
            trade,
            dirty: BTreeSet::new(),
            ground_dirty: false,
            failed_tx,
            failed,
            last_save: Instant::now(),
            interval: SAVE_INTERVAL,
            abandoned: false,
        };
        // Item identities are allocated past every character's high water; a saved item above
        // it would be handed out twice.
        let next = store.next_item_id()?;
        if store.drops.values().any(|d| d.item.id >= next) {
            return Err(invalid("saved ground item is beyond the item identity high water"));
        }
        Ok(store)
    }
    /// Drops the Store without its final save, as a crash would (queued saves still run).
    #[doc(hidden)]
    pub fn abandon(mut self) {
        self.abandoned = true;
    }
    /// The database thread, for other world state that must be ordered with character saves.
    pub fn writer(&self) -> &db::Writer {
        &self.writer
    }
    pub fn set_save_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }
    fn touch(&mut self, id: u64) {
        self.dirty.insert(id);
    }
    /// Marks a character changed outside the Store's own mutators (`credit_offline`, tools).
    pub fn mark_dirty(&mut self, id: u64) {
        self.touch(id);
    }
    pub fn dirty(&self) -> bool {
        !self.dirty.is_empty() || self.ground_dirty
    }
    fn take_failures(&mut self) {
        while let Ok((ids, ground)) = self.failed.try_recv() {
            self.dirty.extend(ids);
            self.ground_dirty |= ground;
        }
    }
    fn snapshot(&mut self) -> io::Result<(shard::Snapshot, Vec<u64>, bool)> {
        self.take_failures();
        let ids: Vec<u64> = std::mem::take(&mut self.dirty).into_iter().filter(|id| self.characters.contains_key(id)).collect();
        let ground = std::mem::take(&mut self.ground_dirty);
        let mut snapshot = shard::Snapshot::default();
        for id in &ids {
            let c = &self.characters[id];
            snapshot.characters.push(to_row(c, self.positions.get(id).copied(), self.trade.get(id).cloned())?);
        }
        if ground {
            snapshot.ground = Some(self.drops.values().cloned().collect());
        }
        Ok((snapshot, ids, ground))
    }
    /// Queues one transaction with every changed character and the ground items. Returns at
    /// once; a failed commit is logged and its characters are saved again next time.
    pub fn save(&mut self) -> io::Result<()> {
        self.last_save = Instant::now();
        let (snapshot, ids, ground) = self.snapshot()?;
        if snapshot.is_empty() {
            return Ok(());
        }
        let failed = self.failed_tx.clone();
        self.writer.submit(move |client| {
            if let Err(e) = shard::save(client, &snapshot) {
                eprintln!("Save of {} character(s) failed, retrying at the next save: {e}", ids.len());
                let _ = failed.send((ids, ground));
            }
        });
        Ok(())
    }
    /// Saves now and waits for the commit.
    pub fn save_now(&mut self) -> io::Result<()> {
        self.last_save = Instant::now();
        let (snapshot, ids, ground) = self.snapshot()?;
        if snapshot.is_empty() {
            return self.writer.drain();
        }
        match self.writer.call(move |client| shard::save(client, &snapshot)) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.dirty.extend(ids);
                self.ground_dirty |= ground;
                Err(e)
            }
        }
    }
    /// The periodic save: saves when the interval has passed since the last one.
    pub fn save_due(&mut self) -> io::Result<bool> {
        if self.last_save.elapsed() < self.interval {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }
    pub fn create(&mut self, account: &str, name: &str) -> io::Result<Character> {
        self.create_record(account, name, None)
    }
    pub fn placement(&self, id: u64) -> Option<Placement> {
        self.positions.get(&id).copied()
    }
    pub fn next_item_id(&self) -> io::Result<u64> {
        self.characters
            .values()
            .map(|c| c.inventory.high_water())
            .max()
            .unwrap_or(crate::inventory::ITEM_NAMESPACE)
            .checked_add(1)
            .filter(|id| id >> 48 == crate::inventory::ITEM_NAMESPACE >> 48)
            .ok_or_else(|| invalid("item identity exhausted"))
    }
    /// Persists structurally valid input. The caller must validate entity/world authority.
    pub fn create_native(
        &mut self,
        account: &str,
        request: &CharacterCreationRequest,
    ) -> io::Result<Character> {
        self.create_record(account, &request.name, Some(request))
    }



    pub fn begin_instance(&mut self, account: &str, id: u64) -> io::Result<u16> {
        let c = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("active character not owned"))?;
        let instance = c
            .instance
            .checked_add(1)
            .ok_or_else(|| invalid("character instance stamp exhausted"))?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().instance = instance;
        Ok(instance)
    }    pub fn trade_put(&mut self, id: u64, line: String) -> io::Result<()> {
        if self.characters.get(&id).is_none_or(|c| c.deleted) || line.len() > 4096 || line.contains(['\n', '\t']) {
            return Err(invalid("tradeskill state for an unavailable character"));
        }
        self.trade.insert(id, line);
        self.touch(id);
        Ok(())
    }
    /// Credits experience to a character whether or not it is online (an online character's
    /// state is the in-memory one; an offline character is saved with the next save). The hook
    /// for experience that reaches a character from others, such as allegiance pass-up.
    pub fn credit_offline(&mut self, id: u64, xp: u64) -> io::Result<()> {
        let c = self.characters.get(&id).filter(|c| !c.deleted).ok_or_else(|| invalid("credited character unavailable"))?;
        let next = c.progression.apply(&crate::progression::Command::AwardXp(xp)).map_err(io::Error::other)?;
        self.characters.get_mut(&id).unwrap().progression = next;
        self.touch(id);
        Ok(())
    }
    fn create_record(
        &mut self,
        account: &str,
        name: &str,
        creation: Option<&CharacterCreationRequest>,
    ) -> io::Result<Character> {
        if !valid_name(account) {
            return Err(invalid(
                "account names must contain 1-32 ASCII letters, digits, spaces, hyphens or underscores",
            ));
        }
        if !valid_character_name(name) {
            return Err(invalid(
                "character names must be 1-32 letters the client's name field takes (any of its languages), digits, spaces, hyphens, apostrophes or underscores",
            ));
        }
        if let Some(request) = creation {
            validate_creation(request)?;
        }
        if self
            .characters
            .values()
            .any(|c| c.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid("character name already exists"));
        }
        if self
            .characters
            .values()
            .filter(|c| c.account == account && !c.deleted)
            .count()
            >= 5
        {
            return Err(invalid("local character limit is five"));
        }
        let id = self
            .characters
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invalid("character IDs exhausted"))?;
        let c = Character {
            id,
            account: account.into(),
            name: name.into(),
            creation: creation.cloned(),
            deleted: false,
            instance: 0,
            ui: None,
            gameplay_options: None,
            chat_options: None,
            hotbar: crate::preferences::Hotbar::default(),
            inventory: crate::inventory::Inventory::default(),
            progression: crate::progression::Progression::default(),
            skills: crate::skills::Book::default(),
            professions: crate::professions::Book::default(),
            quests: crate::quests::Book::default(),
            training_kit: false,
            training_supplies: false,
            starter_gear: false,
            binding: None,
            skill_used: BTreeMap::new(),
        };
        self.characters.insert(id, c.clone());
        self.touch(id);
        if let Err(e) = self.save_now() {
            self.characters.remove(&id);
            self.dirty.remove(&id);
            return Err(e);
        }
        Ok(c)
    }
    fn set_deleted(&mut self, id: u64, deleted: bool) -> io::Result<()> {
        self.characters.get_mut(&id).unwrap().deleted = deleted;
        self.touch(id);
        if let Err(e) = self.save_now() {
            self.characters.get_mut(&id).unwrap().deleted = !deleted;
            return Err(e);
        }
        Ok(())
    }
    pub fn delete(&mut self, account: &str, id: u64) -> io::Result<()> {
        if !self
            .characters
            .get(&id)
            .is_some_and(|c| c.account == account && !c.deleted)
        {
            return Err(invalid("character not owned or already deleted"));
        }
        self.set_deleted(id, true)
    }
    pub fn restore(&mut self, account: &str, id: u64) -> io::Result<()> {
        if !self
            .characters
            .get(&id)
            .is_some_and(|c| c.account == account && c.deleted)
        {
            return Err(invalid("deleted character not owned"));
        }
        if self
            .characters
            .values()
            .filter(|c| c.account == account && !c.deleted)
            .count()
            >= 5
        {
            return Err(invalid("local character limit is five"));
        }
        self.set_deleted(id, false)
    }
}
mod setup;
mod transactions;

impl Drop for Store {
    /// A dropped Store (shutdown, tests) saves what changed and waits; the writer then releases
    /// the server lock.
    fn drop(&mut self) {
        if self.abandoned {
            return;
        }
        if let Err(e) = self.save_now() {
            eprintln!("Final save failed: {e}");
        }
    }
}

const MAX_PHYSICAL_VALUES: usize = 256;
/// A creation request is kept exactly as received; its physical values must be bounded, unique
/// and finite.
fn validate_creation(request: &CharacterCreationRequest) -> io::Result<()> {
    if request.physical_values.len() > MAX_PHYSICAL_VALUES {
        return Err(invalid("too many physical values"));
    }
    let mut keys = BTreeSet::new();
    if request
        .physical_values
        .iter()
        .any(|(key, bits)| !keys.insert(*key) || !f32::from_bits(*bits).is_finite())
    {
        return Err(invalid("invalid or duplicate physical value"));
    }
    Ok(())
}

fn json<T: serde::Serialize>(value: &T) -> io::Result<serde_json::Value> {
    serde_json::to_value(value).map_err(io::Error::other)
}
fn unjson<T: serde::de::DeserializeOwned>(value: serde_json::Value, what: &str) -> io::Result<T> {
    serde_json::from_value(value).map_err(|e| invalid(&format!("{what}: {e}")))
}

fn to_row(c: &Character, placement: Option<Placement>, tradeskills: Option<String>) -> io::Result<shard::CharacterRow> {
    let p = &c.progression;
    Ok(shard::CharacterRow {
        id: c.id,
        account: c.account.clone(),
        name: c.name.clone(),
        deleted: c.deleted,
        instance: c.instance,
        xp: p.xp,
        available_xp: p.available_xp,
        gold: p.gold,
        health: p.health,
        vigor: p.vigor,
        placement,
        item_high_water: c.inventory.saved_high_water(),
        training_kit: c.training_kit,
        training_supplies: c.training_supplies,
        starter_gear: c.starter_gear,
        creation: c.creation.as_ref().map(json).transpose()?,
        binding: c.binding.as_ref().map(json).transpose()?,
        ui: c.ui.as_ref().map(crate::preferences::UiSettings::encode),
        gameplay_options: c.gameplay_options.as_ref().map(crate::preferences::GameplayOptions::encode),
        chat_options: c.chat_options.as_ref().map(crate::preferences::ChatOptions::encode),
        hotbar: json(&c.hotbar)?,
        skills: json(&c.skills)?,
        professions: json(&c.professions)?,
        quests: json(&c.quests)?,
        tradeskills,
        items: c.inventory.items().cloned().collect(),
    })
}

fn from_row(r: shard::CharacterRow) -> io::Result<(Character, Option<Placement>, Option<String>)> {
    if r.placement.is_some_and(|p| !p.valid()) {
        return Err(invalid("invalid saved placement"));
    }
    let binding: Option<Placement> = r.binding.map(|v| unjson(v, "binding")).transpose()?;
    if binding.is_some_and(|p| !p.valid()) {
        return Err(invalid("invalid saved binding"));
    }
    let creation: Option<CharacterCreationRequest> = r.creation.map(|v| unjson(v, "creation")).transpose()?;
    if let Some(request) = &creation {
        validate_creation(request)?;
        if request.name != r.name {
            return Err(invalid("saved creation request names another character"));
        }
    }
    let character = Character {
        id: r.id,
        account: r.account,
        name: r.name,
        creation,
        deleted: r.deleted,
        instance: r.instance,
        ui: r
            .ui
            .map(|b| crate::preferences::UiSettings::decode(&b).map_err(|_| invalid("invalid saved UI settings")))
            .transpose()?,
        hotbar: unjson(r.hotbar, "hotbar")?,
        gameplay_options: r
            .gameplay_options
            .map(|b| crate::preferences::GameplayOptions::decode(&b).map_err(|_| invalid("invalid saved gameplay options")))
            .transpose()?,
        chat_options: r
            .chat_options
            .map(|b| crate::preferences::ChatOptions::decode(&b).map_err(|_| invalid("invalid saved chat options")))
            .transpose()?,
        inventory: crate::inventory::Inventory::restore(r.items, r.item_high_water)
            .map_err(|e| invalid(&format!("invalid saved inventory: {e}")))?,
        progression: crate::progression::Progression {
            xp: r.xp,
            available_xp: r.available_xp,
            gold: r.gold,
            health: r.health,
            vigor: r.vigor,
        },
        skills: unjson(r.skills, "skills")?,
        professions: unjson(r.professions, "professions")?,
        quests: unjson(r.quests, "quests")?,
        training_kit: r.training_kit,
        training_supplies: r.training_supplies,
        starter_gear: r.starter_gear,
        binding,
        skill_used: BTreeMap::new(),
    };
    Ok((character, r.placement, r.tradeskills))
}

/// The emote set that pays a legacy `Shelter*` exchange record.
fn legacy_shelter_set(command:crate::quests::Command)->Option<&'static str> {
    use crate::quests::Command as Q;
    match command {Q::ShelterTool=>Some("scout_trade"),Q::ShelterGlyph=>Some("kit_portal_glyph"),Q::ShelterMenhir(_)=>Some("menhir_portal_glyph"),
        Q::ShelterAdvance=>Some("advance_end"),Q::ShelterComplete=>Some("kit_shard_first"),_=>None}
}
/// A quest exchange applied to a character: its quest commands, items and experience.
fn apply_emote(c:&Character,first:u64,quests:&[crate::quests::Command],items:&[crate::inventory::Command],changes:&[crate::progression::Command])
    ->io::Result<(crate::quests::Book,crate::inventory::Inventory,crate::progression::Progression)> {
    let book=quests.iter().try_fold(c.quests.clone(),|b,q|b.apply(q)).map_err(io::Error::other)?;
    let (inventory,progression)=if items.is_empty()&&changes.is_empty() {(c.inventory.clone(),c.progression.clone())}
        else {crate::player_transaction::candidate(&c.inventory,&c.progression,items,changes,Some(first))?};
    Ok((book,inventory,progression))
}
/// The quest commands that move a book's retired hunts onto their data quests.
fn retired_hunt_commands(book:&crate::quests::Book)->Vec<crate::quests::Command> {
    use crate::quests::Command as Q;
    let mut out=Vec::new();
    for (quest,state) in &book.hunting.0 {
        if !crate::hunting_quests::retired(*quest) {continue;}
        let Some(q)=crate::hunting_quests::profile(*quest) else {continue};
        let goal=u8::try_from(q.goal).unwrap_or(u8::MAX - 2);
        out.push(Q::Started{quest:*quest,at:state.started});
        if state.completed>0 {
            out.push(Q::Stage{quest:*quest,stage:goal+2});
            out.push(Q::Solved{quest:*quest,at:state.completed});
        } else {
            out.push(Q::Stage{quest:*quest,stage:1+u8::try_from(state.count.min(q.goal)).unwrap_or(goal)});
        }
        out.push(Q::RetireHunt{quest:*quest});
    }
    out
}
/// A kill's quest emotes (`quest_emotes::kill`), credited to the killer: the stage and counter
/// commands and the experience (its items are the encounter's loot).
/// `time` is the kill's (Unix seconds): a quest's time limit and the run's solve time go by it.
fn kill_emotes(c:&Character,entity:u32,time:u64)->(Vec<crate::quests::Command>,Vec<crate::progression::Command>) {
    crate::quest_emotes::kill(&c.quests,&c.inventory,&c.progression,entity,0,time).map_or((Vec::new(),Vec::new()),|o|(o.quests,o.changes))
}

#[cfg(test)]
#[cfg(test)]
mod tests;
