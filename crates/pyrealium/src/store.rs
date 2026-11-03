//! Characters and ground items, held in memory and saved to PostgreSQL (`db::shard`).
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
    pub fn grant_starter_gear(&mut self, account: &str, id: u64) -> io::Result<bool> {
        let character = self.characters.get(&id)
            .filter(|character| character.account == account && !character.deleted)
            .ok_or_else(|| invalid("starter gear character not owned"))?;
        if character.starter_gear { return Ok(false); }
        let request = character.creation.as_ref().ok_or_else(|| invalid("Starter gear needs a saved creation profile."))?;
        let first = self.next_item_id()?;
        let next = crate::starter_gear::candidate(request, &character.inventory, first)?;
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.inventory = next;
        character.starter_gear = true;
        character.training_kit = true;
        Ok(true)
    }
    pub fn grant_training_kit(&mut self, account: &str, id: u64) -> io::Result<bool> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("training kit character not owned"))?;
        if character.training_kit {
            return Ok(false);
        }
        if !character.creation.as_ref().is_some_and(|request| {
            request.race == 1 && request.entity_did == crate::bootstrap::PLAYER_ENTITY
        }) {
            return Err(invalid(
                "training kit requires the human development profile",
            ));
        }
        let slot = character
            .inventory
            .free_slot()
            .ok_or_else(|| invalid("training kit requires a free bag slot"))?;
        let item = self.next_item_id()?;
        let next = character
            .inventory
            .apply(&crate::inventory::Command::Grant(
                crate::training_kit::item(item, slot),
            ))
            .map_err(|_| invalid("invalid training kit grant"))?;
        let skills = character
            .skills
            .with_training_attack(
                &character.progression,
                character.creation.as_ref().unwrap().race,
            )
            .map_err(io::Error::other)?;
        // The item, basic attack and once-per-character marker change together.
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.inventory = next;
        character.skills = skills;
        character.training_kit = true;
        Ok(true)
    }
    pub fn grant_training_supplies(&mut self, account: &str, id: u64) -> io::Result<bool> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted && c.training_kit)
            .ok_or_else(|| {
                invalid("training supplies character not owned or equipped for training")
            })?;
        if character.training_supplies {
            return Ok(false);
        }
        if !character.progression.alive() {
            return Err(invalid("training supplies require a living character"));
        }
        let first = self.next_item_id()?;
        let next = crate::training_kit::supply_inventory(&character.inventory, first)
            .map_err(io::Error::other)?;
        // All five items and the once-only marker change together.
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.inventory = next;
        character.training_supplies = true;
        Ok(true)
    }
    pub fn ensure_training_attack(&mut self, account: &str, id: u64) -> io::Result<bool> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted && c.training_kit)
            .ok_or_else(|| invalid("training attack requires an owned training character"))?;
        if character
            .skills
            .get(crate::skills::TRAINING_ATTACK)
            .is_some()
        {
            return Ok(false);
        }
        let skills = character
            .skills
            .with_training_attack(
                &character.progression,
                character
                    .creation
                    .as_ref()
                    .map_or(1, |request| request.race),
            )
            .map_err(io::Error::other)?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().skills = skills;
        Ok(true)
    }
    pub fn ensure_starter_skills(&mut self, account: &str, id: u64) -> io::Result<bool> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("starter skills require an owned character"))?;
        let Some((race, sex)) = character
            .creation
            .as_ref()
            .map(|request| (request.race, request.sex))
            .filter(|&(race, sex)| crate::appearance::body(race, sex).is_some())
        else {
            return Ok(false);
        };
        if character.skills.has_starter_skills(race, sex) || !character.progression.alive() {
            return Ok(false);
        }
        let skills = character
            .skills
            .with_starter_skills(&character.progression, race, sex)
            .map_err(io::Error::other)?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().skills = skills;
        Ok(true)
    }
    pub fn bind_lifestone(
        &mut self,
        account: &str,
        id: u64,
        placement: Placement,
    ) -> io::Result<()> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted && c.progression.alive())
            .ok_or_else(|| invalid("binding requires an owned living character"))?;
        if !placement.valid() {
            return Err(invalid("invalid lifestone bind point"));
        }
        let skills = character
            .skills
            .with_lifestone_recall(
                &character.progression,
                character
                    .creation
                    .as_ref()
                    .map_or(1, |request| request.race),
            )
            .map_err(io::Error::other)?;
        let quests = if character.quests.journey_phase() == 1 {
            character.quests.apply(&crate::quests::Command::BoundLifestone).map_err(io::Error::other)?
        } else { character.quests.clone() };
        if quests == character.quests && character.binding == Some(placement)
            && character
                .skills
                .get(crate::skills::LIFESTONE_RECALL)
                .is_some()
        {
            return Ok(());
        }
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.binding = Some(placement);
        character.skills = skills;
        character.quests = quests;
        Ok(())
    }
    pub fn ensure_lifestone_recall(&mut self, account: &str, id: u64) -> io::Result<bool> {
        let character = self
            .characters
            .get(&id)
            .filter(|character| {
                character.account == account && !character.deleted && character.binding.is_some()
            })
            .ok_or_else(|| invalid("recall grant requires an owned bound character"))?;
        if character
            .skills
            .get(crate::skills::LIFESTONE_RECALL)
            .is_some()
        {
            return Ok(false);
        }
        let skills = character
            .skills
            .with_lifestone_recall(
                &character.progression,
                character
                    .creation
                    .as_ref()
                    .map_or(1, |request| request.race),
            )
            .map_err(io::Error::other)?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().skills = skills;
        Ok(true)
    }
    /// Destination must have passed world/cell validation before this transaction.
    pub fn relocate(&mut self, account: &str, id: u64, placement: Placement) -> io::Result<()> {
        if !placement.valid()
            || !self.characters.get(&id).is_some_and(|character| {
                character.account == account && !character.deleted && character.progression.alive()
            })
        {
            return Err(invalid(
                "relocation requires an owned living character and valid placement",
            ));
        }
        self.touch(id);
        self.positions.insert(id, placement);
        self.dirty.insert(id);
        Ok(())
    }
    pub fn relocate_training_portal(&mut self, account: &str, id: u64, placement: Placement, portal: u64) -> io::Result<()> {
        let character = self.characters.get(&id).filter(|c| c.account == account && !c.deleted && c.progression.alive()).ok_or_else(|| invalid("Travel requires an owned living character"))?;
        if character.quests.journey_phase() != 2 || !crate::quests::is_training_portal(portal) {
            return self.relocate(account, id, placement);
        }
        if !placement.valid() { return Err(invalid("Invalid training portal destination")); }
        let quests = character.quests.apply(&crate::quests::Command::Travelled).map_err(io::Error::other)?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().quests = quests;
        self.positions.insert(id, placement); self.dirty.insert(id);
        Ok(())
    }
    /// Binding must have passed world/cell validation before this transaction.
    pub fn return_to_binding(
        &mut self,
        account: &str,
        id: u64,
        resurrect: bool,
    ) -> io::Result<Placement> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("recall character not owned"))?;
        let placement = character
            .binding
            .ok_or_else(|| invalid("character is not bound to a lifestone"))?;
        let next = if resurrect {
            character
                .progression
                .apply(&crate::progression::Command::Resurrect)
                .map_err(|_| invalid("resurrection requires a dead character"))?
        } else {
            if !character.progression.alive() {
                return Err(invalid("recall requires a living character"));
            }
            character.progression.clone()
        };
        self.touch(id);
        self.characters.get_mut(&id).unwrap().progression = next;
        self.positions.insert(id, placement);
        self.dirty.insert(id);
        Ok(placement)
    }
    pub fn progression_command(
        &mut self,
        account: &str,
        id: u64,
        command: crate::progression::Command,
    ) -> io::Result<()> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("progression character not owned"))?;
        let next = character
            .progression
            .apply(&command)
            .map_err(|error| invalid(&error.to_string()))?;
        if next == character.progression {
            return Ok(());
        }
        self.touch(id);
        self.characters.get_mut(&id).unwrap().progression = next;
        Ok(())
    }
    pub fn raise_skill(&mut self, account: &str, id: u64, skill: u32) -> io::Result<()> {
        let character = self
            .characters
            .get(&id)
            .filter(|character| character.account == account && !character.deleted)
            .ok_or_else(|| invalid("skill advancement character not owned"))?;
        let race = character
            .creation
            .as_ref()
            .map_or(1, |request| request.race);
        let (skills, progression) = character
            .skills
            .advance(skill, &character.progression, race)
            .map_err(io::Error::other)?;
        character
            .progression
            .available_xp
            .checked_sub(progression.available_xp)
            .filter(|cost| *cost > 0)
            .ok_or_else(|| invalid("invalid advancement cost"))?;
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.skills = skills;
        character.progression = progression;
        Ok(())
    }
    pub fn skill_command(
        &mut self,
        account: &str,
        id: u64,
        command: crate::skills::Command,
    ) -> io::Result<()> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("skill character not owned"))?;
        let race = character
            .creation
            .as_ref()
            .map_or(1, |request| request.race);
        let next = character
            .skills
            .apply(command, &character.progression, race)
            .map_err(io::Error::other)?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().skills = next;
        Ok(())
    }
    pub fn debit_untraining(&mut self,id:u64)->io::Result<bool> {
        let c=self.characters.get(&id).filter(|c|!c.deleted).ok_or_else(||invalid("untraining character unavailable"))?;
        let Some((skills,progression))=c.skills.debit_untraining(&c.progression).map_err(io::Error::other)? else{return Ok(false)};
        // Replay uses the persisted earned-XP total; no clock or client quantity.
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();c.skills=skills;c.progression=progression;Ok(true)
    }
    pub fn progression_batch(
        &mut self,
        account: &str,
        id: u64,
        commands: &[crate::progression::Command],
    ) -> io::Result<()> {
        if commands.is_empty() || commands.len() > 8 {
            return Err(invalid("progression batch size"));
        }
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("progression character not owned"))?;
        let mut next = character.progression.clone();
        for command in commands {
            next = next
                .apply(command)
                .map_err(|error| invalid(&error.to_string()))?;
        }
        if next == character.progression {
            return Ok(());
        }
        self.touch(id);
        self.characters.get_mut(&id).unwrap().progression = next;
        Ok(())
    }
    pub fn quest_command(
        &mut self,
        account: &str,
        id: u64,
        command: crate::quests::Command,
    ) -> io::Result<bool> {
        if legacy_shelter_set(command).is_some() || matches!(command, crate::quests::Command::Stage{..} | crate::quests::Command::Count{..} | crate::quests::Command::Kill(_) | crate::quests::Command::ClaimSupplies | crate::quests::Command::CraftedPotion | crate::quests::Command::BoundLifestone | crate::quests::Command::Travelled) {
            return Err(invalid("quest completion requires its authoritative reward transaction"));
        }
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted && c.progression.alive())
            .ok_or_else(|| invalid("quest requires an owned living character"))?;
        let quests = character.quests.apply(&command).map_err(io::Error::other)?;
        if quests == character.quests {
            return Ok(false);
        }
        let mut progression = character.progression.clone();
        for reward in crate::quests::rewards(&command) {
            progression = progression.apply(&reward).map_err(io::Error::other)?;
        }
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.quests = quests;
        character.progression = progression;
        Ok(true)
    }
    /// Version-one NPC/obelisk progression and its XP are one durable event.
    pub fn exploration_conversation(&mut self,account:&str,id:u64,entity:u32,time:u64)->io::Result<()> {
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive())
            .ok_or_else(||invalid("Quest owner unavailable"))?;
        let result=c.quests.exploration.converse(entity,c.progression.level().number,time)?;
        if result.book==c.quests.exploration {return Ok(());}
        let state=if result.xp>0 {c.progression.apply(&crate::progression::Command::AwardXp(result.xp)).map_err(io::Error::other)?}
            else {c.progression.clone()};
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();c.quests.exploration=result.book;c.progression=state;Ok(())
    }
    /// Version-two NPC hunt progression and its XP are one durable event.
    pub fn hunting_conversation(&mut self,account:&str,id:u64,entity:u32,time:u64)->io::Result<()> {
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive())
            .ok_or_else(||invalid("Quest owner unavailable"))?;
        let result=c.quests.hunting.converse(entity,&c.progression,time)?;
        if result.book==c.quests.hunting {return Ok(());}
        let state=if result.xp>0 {c.progression.apply(&crate::progression::Command::AwardXp(result.xp)).map_err(io::Error::other)?}
            else {c.progression.clone()};
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();c.quests.hunting=result.book;c.progression=state;Ok(())
    }
    /// Commit the canonical inferred delivery exchange and its quest journal together.
    pub fn recovered_delivery(&mut self,account:&str,id:u64,npc:u32,time:u64)->io::Result<()> {
        let first=self.next_item_id()?;
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Quest owner unavailable"))?;
        let (items,changes,book)=c.quests.recovered.delivery(&c.inventory,npc,c.progression.level().number,time,first)?;
        let (bag,state)=if items.is_empty()&&changes.is_empty(){(c.inventory.clone(),c.progression.clone())}else{
            crate::player_transaction::candidate(&c.inventory,&c.progression,&items,&changes,Some(first))?};
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();c.inventory=bag;c.progression=state;c.quests.recovered=book;Ok(())
    }
    /// Consume a supported hunt elixir and bestow its quest journal in one step.
    pub fn recovered_elixir(&mut self,account:&str,id:u64,item:u64)->io::Result<()> {
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Quest owner unavailable"))?;
        let owned=c.inventory.items().find(|i|i.id==item&&matches!(i.location,crate::inventory::Location::Bag(_)))
            .ok_or_else(||invalid("Quest starter must be in your backpack"))?;
        let q=crate::recovered_quests::use_profile(owned.definition).ok_or_else(||invalid("Unsupported quest starter"))?;
        let time=crate::recovered_quests::now();
        let book=c.quests.recovered.accept(q.id,c.progression.level().number,time)?;
        let bag=c.inventory.apply(&crate::inventory::Command::ConsumeDense{item}).map_err(io::Error::other)?;
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();c.inventory=bag;c.quests.recovered=book;Ok(())
    }
    /// Moves each character's state of a retired hunt (`hunting_quests::retired`) onto the data
    /// quest of the same journal id: an accepted run keeps its
    /// start time at the stage of its kills (1 + kills; the report stage at the goal), a solved one
    /// is the completed stage with its solve time, and the hunting book drops it. Run at start,
    /// after loading; returns how many characters moved.
    pub fn migrate_retired_hunts(&mut self)->io::Result<usize> {
        let mut moved=0;
        let ids:Vec<u64>=self.characters.keys().copied().collect();
        for id in ids {
            let c=&self.characters[&id];
            if c.deleted {continue;}
            let commands=retired_hunt_commands(&c.quests);
            if commands.is_empty() {continue;}
            let book=commands.iter().try_fold(c.quests.clone(),|b,q|b.apply(q)).map_err(io::Error::other)?;
            self.touch(id);
            self.characters.get_mut(&id).unwrap().quests=book;
            moved+=1;
        }
        Ok(moved)
    }
    /// A quest emote's outcome (`quest_emotes`): its quest stages, items, experience and move are
    /// one step.
    pub fn emote_transaction(&mut self,account:&str,id:u64,outcome:&crate::quest_emotes::Outcome)->io::Result<()> {
        let first=self.next_item_id()?;
        let c=self.characters.get(&id).filter(|c|c.account==account && !c.deleted && c.progression.alive())
            .ok_or_else(||invalid("Quest exchange owner unavailable"))?;
        let next=apply_emote(c,first,&outcome.quests,&outcome.items,&outcome.changes)?;
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();(c.quests,c.inventory,c.progression)=next;
        if let Some(placement)=outcome.moveto {self.positions.insert(id,placement);self.dirty.insert(id);}
        Ok(())
    }
    /// Persist the objective with the normal payout before publishing death.
    pub fn encounter_reward(
        &mut self,
        account: &str,
        id: u64,
        entity: u32,
        commands: &[crate::progression::Command],
    ) -> io::Result<()> {
        if commands.is_empty() || commands.len() > 8 {
            return Err(invalid("encounter reward batch size"));
        }
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted && c.progression.alive())
            .ok_or_else(|| invalid("encounter reward character unavailable"))?;
        let time=crate::recovered_quests::now();
        let (kill_quests,kill_xp)=kill_emotes(character,entity,time);
        let mut quests=character.quests.record_kill(entity);
        let (recovered,xp)=quests.recovered.kill(entity,character.progression.level().number,time);
        quests.recovered=recovered;
        quests.hunting=quests.hunting.kill(entity,time);
        for command in &kill_quests {quests=quests.apply(command).map_err(io::Error::other)?;}
        let mut progression=character.progression.clone();
        for command in commands.iter().chain(&kill_xp) {progression=progression.apply(command).map_err(io::Error::other)?;}
        if xp>0 {progression=progression.apply(&crate::progression::Command::AwardXp(xp)).map_err(io::Error::other)?;}
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.quests = quests;
        character.progression = progression;
        Ok(())
    }
    pub fn encounter_transaction(
        &mut self,
        account: &str,
        id: u64,
        entity: u32,
        items: &[crate::inventory::Command],
        changes: &[crate::progression::Command],
    ) -> io::Result<()> {
        if entity >> 24 != 0x47 || items.is_empty() || changes.is_empty() {
            return Err(invalid("invalid item encounter reward"));
        }
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted && c.progression.alive())
            .ok_or_else(|| invalid("encounter transaction owner unavailable"))?;
        let first = if items.iter().any(|command| command.allocated_id().is_some()) {
            Some(self.next_item_id()?)
        } else {
            None
        };
        let (inventory, progression) = crate::player_transaction::candidate(
            &character.inventory,
            &character.progression,
            items,
            changes,
            first,
        )?;
        // The kill's time is recorded (`ek1`): the quest emotes' time limits and solve times go by it.
        let time=crate::recovered_quests::now();
        let (kill_quests,kill_xp)=kill_emotes(character,entity,time);
        let progression=kill_xp.iter().try_fold(progression,|p,c|p.apply(c)).map_err(io::Error::other)?;
        let quests=kill_quests.iter().try_fold(character.quests.record_kill(entity),|b,c|b.apply(c)).map_err(io::Error::other)?;
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.inventory = inventory;
        character.progression = progression;
        character.quests = quests;
        Ok(())
    }
    /// Items and payment/rewards change together.
    pub fn player_transaction(
        &mut self,
        account: &str,
        id: u64,
        items: &[crate::inventory::Command],
        changes: &[crate::progression::Command],
    ) -> io::Result<()> {
        self.player_transaction_quest(account, id, items, changes, false, false)
    }
    pub fn player_transaction_quest(
        &mut self, account: &str, id: u64,
        items: &[crate::inventory::Command], changes: &[crate::progression::Command],
        supplies: bool, practice: bool,
    ) -> io::Result<()> {
        if supplies && practice { return Err(invalid("Multiple quest exchanges in one transaction")); }
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("transaction character not owned"))?;
        let first = if items.iter().any(|command| command.allocated_id().is_some()) {
            Some(self.next_item_id()?)
        } else {
            None
        };
        let (inventory, progression) = crate::player_transaction::candidate(
            &character.inventory,
            &character.progression,
            items,
            changes,
            first,
        )?;
        if inventory == character.inventory && progression == character.progression {
            return Ok(());
        }
        let quests = if supplies {
            if !character.progression.alive() { return Err(invalid("Supply exchange requires a living player")); }
            crate::quests::validate_supplies_exchange(&character.quests, &character.inventory, items, changes, first.ok_or_else(|| invalid("Missing supply output identity"))?)?
        } else if practice {
            if !character.progression.alive() { return Err(invalid("Crafting requires a living owner")); }
            crate::quests::validate_practice_craft(&character.quests, &character.inventory, items, changes, first.ok_or_else(|| invalid("Missing craft output identity"))?)?
        } else { character.quests.clone() };
        self.touch(id);
        let character = self.characters.get_mut(&id).unwrap();
        character.inventory = inventory;
        character.progression = progression;
        character.quests = quests;
        Ok(())
    }
    /// Inventory, lesson entitlement, quest journal and XP change together.
    pub fn arwic_transaction(&mut self,account:&str,id:u64,action:crate::arwic_mines::Action,time:u64)->io::Result<()> {
        let first=self.next_item_id()?;
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Lesson owner unavailable"))?;
        let plan=crate::arwic_mines::plan(&c.quests,&c.inventory,action,first,time)?;
        if plan.quests==c.quests && plan.items.is_empty() && plan.rewards.is_empty() {return Ok(());}
        let (bag,state)=if plan.items.is_empty()&&plan.rewards.is_empty(){(c.inventory.clone(),c.progression.clone())}else{crate::player_transaction::candidate(&c.inventory,&c.progression,&plan.items,&plan.rewards,Some(first))?};
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();c.inventory=bag;c.progression=state;c.quests=plan.quests;Ok(())
    }
    /// Inventory/cost and profession XP change together.
    pub fn profession_transaction(&mut self,account:&str,id:u64,items:&[crate::inventory::Command],changes:&[crate::progression::Command],change:&crate::professions::Change)->io::Result<()> {
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Crafting owner unavailable"))?;
        let book=c.professions.apply(change)?;
        let first=items.iter().any(|i|i.allocated_id().is_some()).then(||self.next_item_id()).transpose()?;
        let (bag,state)=if items.is_empty()&&changes.is_empty(){(c.inventory.clone(),c.progression.clone())}else{crate::player_transaction::candidate(&c.inventory,&c.progression,items,changes,first)?};
        self.touch(id);
        let c=self.characters.get_mut(&id).unwrap();c.inventory=bag;c.progression=state;c.professions=book;Ok(())
    }
    pub fn inventory_command(
        &mut self,
        account: &str,
        id: u64,
        command: crate::inventory::Command,
    ) -> io::Result<()> {
        if command
            .allocated_id()
            .is_some_and(|allocated| self.next_item_id().ok() != Some(allocated))
        {
            return Err(invalid("nonsequential item identity"));
        }
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("inventory character not owned"))?;
        let next = character
            .inventory
            .apply(&command)
            .map_err(|error| invalid(&error.to_string()))?;
        if next == character.inventory {
            return Ok(());
        }
        self.touch(id);
        self.characters.get_mut(&id).unwrap().inventory = next;
        Ok(())
    }
    pub fn update_ui(
        &mut self,
        account: &str,
        id: u64,
        patch: crate::preferences::UiSettings,
    ) -> io::Result<()> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("UI settings character not owned"))?;
        let settings = match &character.ui {
            Some(previous) => previous
                .merge(&patch)
                .map_err(|_| invalid("UI settings limit exceeded"))?,
            None => patch,
        };
        if character.ui.as_ref() == Some(&settings) {
            return Ok(());
        }
        self.touch(id);
        self.characters.get_mut(&id).unwrap().ui = Some(settings);
        Ok(())
    }
    pub fn update_gameplay_options(
        &mut self,
        account: &str,
        id: u64,
        options: crate::preferences::GameplayOptions,
    ) -> io::Result<()> {
        let character = self.characters.get(&id)
            .filter(|character| character.account == account && !character.deleted)
            .ok_or_else(|| invalid("gameplay settings character not owned"))?;
        if character.gameplay_options.as_ref() == Some(&options) { return Ok(()); }
        self.touch(id);
        self.characters.get_mut(&id).unwrap().gameplay_options = Some(options);
        Ok(())
    }
    pub fn update_hotbar(
        &mut self,
        account: &str,
        id: u64,
        update: crate::preferences::HotbarUpdate,
    ) -> io::Result<()> {
        let character = self
            .characters
            .get(&id)
            .filter(|c| c.account == account && !c.deleted)
            .ok_or_else(|| invalid("hotbar character not owned"))?;
        let next = character
            .hotbar
            .apply(&update)
            .map_err(|_| invalid("hotbar size limit exceeded"))?;
        if next == character.hotbar {
            return Ok(());
        }
        update
            .encode()
            .map_err(|_| invalid("invalid hotbar update"))?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().hotbar = next;
        Ok(())
    }
    pub fn set_placement(&mut self, id: u64, placement: Placement) -> io::Result<()> {
        if self.characters.get(&id).is_none_or(|c| c.deleted) || !placement.valid() {
            return Err(invalid("invalid character placement"));
        }
        if self.positions.get(&id) != Some(&placement) {
            self.positions.insert(id, placement);
            self.dirty.insert(id);
        }
        Ok(())
    }
    /// The saved tradeskill line of an owned, live character.
    pub fn trade_get(&self, id: u64) -> Option<&str> {
        self.characters.get(&id).filter(|c| !c.deleted)?;
        self.trade.get(&id).map(String::as_str)
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
        if !valid_name(account) || !valid_name(name) {
            return Err(invalid(
                "account and character names must contain 1-32 ASCII letters, digits, spaces, hyphens or underscores",
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
mod tests {
    use super::*;
    #[test]
    fn invested_skill_untraining_refunds_earned_xp_and_survives_restart() {
        use crate::{skills::Command as S,progression::Command as P};
        let dir=crate::db::testdb::fresh();
        let partial;
        {
            let mut s=Store::open(&dir).unwrap();let mut creation=request("Skill Hero");creation.race=1;creation.sex=4096;
            s.create_native("local",&creation).unwrap();
            s.progression_command("local",1,P::AwardXp(10000)).unwrap();
            s.ensure_starter_skills("local",1).unwrap();
            for id in [0x59,0x55] {s.skill_command("local",1,S::Train(id)).unwrap();}
            s.raise_skill("local",1,0x55).unwrap();
            let invested=s.characters[&1].skills.get(0x55).unwrap().xp;assert!(invested>1);
            assert!(s.skill_command("local",1,S::Untrain(0x59)).is_err());
            s.skill_command("local",1,S::Untrain(0x55)).unwrap();
            assert!(!s.characters[&1].skills.usable(0x55));
            assert!(s.raise_skill("local",1,0x55).is_err());
            assert!(!s.debit_untraining(1).unwrap());
            let available=s.characters[&1].progression.available_xp;
            s.progression_command("local",1,P::AwardXp(1)).unwrap();
            assert!(s.debit_untraining(1).unwrap());
            assert_eq!(s.characters[&1].skills.get(0x55).unwrap().xp,invested-1);
            assert_eq!(s.characters[&1].progression.available_xp,available+2);
            assert!(!s.debit_untraining(1).unwrap());
            partial=s.characters[&1].clone();
        }
        {
            let mut s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1],partial);
            s.skill_command("local",1,S::CancelUntrain).unwrap();assert!(s.characters[&1].skills.usable(0x55));
            let remaining=s.characters[&1].skills.get(0x55).unwrap().xp;
            s.progression_command("local",1,P::AwardXp(10)).unwrap();assert!(!s.debit_untraining(1).unwrap());
            s.skill_command("local",1,S::Untrain(0x55)).unwrap();
            s.progression_command("local",1,P::AwardXp(remaining+20)).unwrap();s.debit_untraining(1).unwrap();
            assert!(s.characters[&1].skills.get(0x55).is_none());assert_eq!(s.characters[&1].skills.untraining_id(),0);
            assert_eq!(s.characters[&1].progression.available_xp,s.characters[&1].progression.xp);
        }
        {let s=Store::open(&dir).unwrap();assert!(s.characters[&1].skills.get(0x55).is_none());}
    }
    #[test]
    fn quest_emote_exchanges_replay_as_recorded() {
        use crate::{inventory::Command as I,quest_emotes::Trigger as T,shelter};
        let t=crate::quest_emotes::shipped();
        let dir=crate::db::testdb::fresh();
        let room=crate::movement::Placement{cell:0x0299014d,coordinates:[1.0,1.0,1.19],heading:0};
        let final_state;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Shelter Hero")).unwrap();
            let step=|s:&mut Store,trigger,entity,iid,item:Option<u32>|{
                let ch=&s.characters[&c.id];
                let o=t.run(trigger,entity,iid,item,&ch.quests,&ch.inventory,&ch.progression,s.next_item_id().unwrap(),0.0,1_800_000_000,&shelter::item).unwrap().unwrap();
                s.emote_transaction("local",c.id,&o).unwrap();o
            };
            for npc in [0x4700157d,0x4700157e,0x47001581] {step(&mut s,T::Use,npc,1,None);}
            assert!(s.emote_transaction("other",c.id,&crate::quest_emotes::Outcome::default()).is_err());
            let id=s.next_item_id().unwrap();s.inventory_command("local",c.id,I::Grant(shelter::item(shelter::TOOL,id,0).unwrap())).unwrap();
            let xp=s.characters[&c.id].progression.xp;
            step(&mut s,T::Use,0x47001580,1,None);
            assert_eq!(s.characters[&c.id].progression.xp,xp+40);
            let id=s.next_item_id().unwrap();s.inventory_command("local",c.id,I::Grant(shelter::item(shelter::GLYPH,id,s.characters[&c.id].inventory.free_slot().unwrap()).unwrap())).unwrap();
            step(&mut s,T::ItemUsedOn,0x47001451,0x814002990005000d,Some(shelter::GLYPH));
            assert_eq!(s.placement(c.id),Some(room));
            step(&mut s,T::Use,0x4700157f,1,None);
            assert_eq!(s.characters[&c.id].progression.xp,xp+105);
            final_state=s.characters[&c.id].clone();
            assert_eq!(final_state.quests.shelter_phase(),6);
            // The run's start and solve times are part of the records and replay with them.
            assert_eq!(final_state.quests.timer(0x4000ff05),crate::quests::Timer{started:1_800_000_000,completed:1_800_000_000,solves:1});
        }
        {let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1],final_state);assert_eq!(s.placement(1),Some(room));}
    }
    #[test]
    fn a_quest_effect_record_replays() {
        let dir=crate::db::testdb::fresh();
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Cursed One")).unwrap();
            let on=crate::quest_emotes::Outcome{quests:vec![crate::quests::Command::EffectOn{effect:0x6f00020f,at:1000,until:87_400}],..Default::default()};
            s.emote_transaction("local",c.id,&on).unwrap();
            saved=s.characters[&c.id].clone();
            assert_eq!(saved.quests.quest_effects(2000),[(0x6f00020f,87_400)]);
        }
        {let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&saved.id],saved);}
        {
            let mut s=Store::open(&dir).unwrap();
            let off=crate::quest_emotes::Outcome{quests:vec![crate::quests::Command::EffectOff{effect:0x6f00020f}],..Default::default()};
            s.emote_transaction("local",saved.id,&off).unwrap();
        }
        {let s=Store::open(&dir).unwrap();assert!(s.characters[&saved.id].quests.quest_effects(2000).is_empty());}
    }
    #[test]
    fn retired_hunts_migrate_once_and_the_migration_replays() {
        let dir=crate::db::testdb::fresh();
        let marsh=0x4000fc20;
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Hunt Mover")).unwrap();
            // State an older server left for retired hunts.
            s.characters.get_mut(&c.id).unwrap().quests.hunting.0.insert(marsh,crate::hunting_quests::State{count:3,started:500,completed:0,rewarded_xp:0});
            s.characters.get_mut(&c.id).unwrap().quests.hunting.0.insert(0x4000fc23,crate::hunting_quests::State{count:1,started:600,completed:0,rewarded_xp:0});
            assert_eq!(s.migrate_retired_hunts().unwrap(),1);
            assert_eq!(s.migrate_retired_hunts().unwrap(),0,"once");
            saved=s.characters[&c.id].clone();
            assert_eq!((saved.quests.stage(marsh),saved.quests.timer(marsh).started),(4,500));
            assert!(!saved.quests.hunting.0.contains_key(&marsh) && saved.quests.hunting.0.contains_key(&0x4000fc23));
        }
        // The migration marked the character changed, so its whole state was saved.
        let mut s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&saved.id],saved);
        assert_eq!(s.migrate_retired_hunts().unwrap(),0,"once, across a restart");
    }
    #[test]
    fn a_full_bag_refuses_the_scout_without_taking_the_tool() {
        use crate::{inventory::{Command as I,BAG_SLOTS},quest_emotes::Trigger as T,shelter};
        let t=crate::quest_emotes::shipped();
        let dir=crate::db::testdb::fresh();
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Full Pack")).unwrap();
            for slot in 0..BAG_SLOTS {
                let id=s.next_item_id().unwrap();
                s.inventory_command("local",c.id,I::Grant(shelter::item(if slot==0 {shelter::TOOL}else{0x47001583},id,slot).unwrap())).unwrap();
            }
            let ch=&s.characters[&c.id];
            let book=ch.quests.apply(&crate::quests::Command::Stage{quest:0x4000ff05,stage:3}).unwrap();
            assert!(t.run(T::Use,0x47001580,1,None,&book,&ch.inventory,&ch.progression,s.next_item_id().unwrap(),0.0,0,&shelter::item).is_err());
        }
    }
    #[test]
    fn deletion_restoration_keeps_identity_appearance_and_saved_placement() {
        let dir=crate::db::testdb::fresh();
        {
            let mut store = Store::open(&dir).unwrap();
            for i in 1..=5 {
                store
                    .create_native("local", &request(&format!("Hero {i}")))
                    .unwrap();
            }
            store.set_placement(5, Placement::HUMAN_START).unwrap();
            store.save_now().unwrap();
            assert!(store.delete("other", 5).is_err());
            store.delete("local", 5).unwrap();
            assert!(store.set_placement(5, Placement::HUMAN_START).is_err());
            assert!(store.delete("local", 5).is_err());
            assert_eq!(store.create("local", "Sixth Hero").unwrap().id, 6);
            assert!(store.restore("local", 5).is_err());
            store.delete("local", 6).unwrap();
            store.restore("local", 5).unwrap();
            assert!(store.restore("other", 6).is_err());
            assert!(store.create("other", "Sixth Hero").is_err());
        }
        {
            let store = Store::open(&dir).unwrap();
            assert!(!store.characters[&5].deleted);
            assert!(store.characters[&6].deleted);
            assert_eq!(store.characters[&5].creation, Some(request("Hero 5")));
            assert_eq!(store.placement(5), Some(Placement::HUMAN_START));
        }
    }
    fn request(name: &str) -> CharacterCreationRequest {
        CharacterCreationRequest {
            name: name.into(),
            entity_did: 0x10001234,
            start_area: 7,
            race: 2,
            sex: 1,
            class: 3,
            implement: 4,
            physical_values: vec![(9, (-0.0f32).to_bits()), (2, 0.1234567f32.to_bits())],
        }
    }
    #[test]
    fn inferred_hunts_elixirs_and_rolled_armor_survive_restart_atomically() {
        use crate::{inventory::{Item,Roll,Location,Command as I,ITEM_NAMESPACE},progression::Command as P};
        let dir=crate::db::testdb::fresh();
        let q=crate::recovered_quests::profiles().iter().find(|q|q.id==0x40000097).unwrap();
        let armor=Item{roll:Some(Roll{armor:17,tint:731,level:7,..Default::default()}),id:ITEM_NAMESPACE+1,
            definition:0x47000fdc,visual:0x1f00058c,quantity:1,stack_limit:1,equipment_mask:16,location:Location::Bag(0)};
        let before;let finished;let bag;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Recovered Hero").unwrap();
            s.progression_command("local",1,P::AwardXp(10000)).unwrap();
            s.inventory_command("local",1,I::Grant(armor.clone())).unwrap();
            s.inventory_command("local",1,I::Wear{item:armor.id,slot:16}).unwrap();
            assert_eq!(crate::apparel::armor(&s.characters[&1].inventory,1),17);
            assert!(crate::apparel::aspects(&s.characters[&1].inventory,1,0x1000).iter().any(|(_,a,v)|*a==2&&(*v-0.731).abs()<0.0001));
            before=s.characters[&1].progression.xp;
            let normal=[P::AwardXp(1)];
            s.encounter_reward("local",1,q.kill_starters[0],&normal).unwrap();
            for _ in 0..q.goal {s.encounter_reward("local",1,q.targets[0],&normal).unwrap();}
            assert_eq!(s.characters[&1].progression.xp,before+u64::from(q.goal)+1+q.xp);
            finished=s.characters[&1].quests.recovered.0[&q.id].clone();assert!(finished.completed>0);
            s.encounter_reward("local",1,q.kill_starters[0],&normal).unwrap();
            assert_eq!(s.characters[&1].quests.recovered.0[&q.id],finished);
            for offset in 2..=3 {
                let elixir=Item{roll:None,id:ITEM_NAMESPACE+offset,definition:0x47000f43,visual:0x1f0001cf,
                    quantity:1,stack_limit:1,equipment_mask:0,location:Location::Bag((offset-2) as u16)};
                s.inventory_command("local",1,I::Grant(elixir)).unwrap();
            }
            s.save_now().unwrap();
            assert!(s.recovered_elixir("other",1,ITEM_NAMESPACE+2).is_err());assert!(!s.dirty());
            s.recovered_elixir("local",1,ITEM_NAMESPACE+2).unwrap();
            s.save_now().unwrap();
            assert!(s.recovered_elixir("local",1,ITEM_NAMESPACE+3).is_err());assert!(!s.dirty());
            assert!(s.characters[&1].inventory.items().any(|i|i.id==ITEM_NAMESPACE+3));
            bag=s.characters[&1].inventory.clone();
        }
        {
            let s=Store::open(&dir).unwrap();let c=&s.characters[&1];
            assert_eq!(c.inventory,bag);assert_eq!(c.quests.recovered.0[&q.id],finished);
            assert_eq!(c.progression.xp,before+u64::from(q.goal)+2+q.xp);
            assert_eq!(c.quests.recovered.0[&0x4000000a].count,0);
            assert_eq!(crate::apparel::armor(&c.inventory,1),17);
            assert!(!c.inventory.items().any(|i|i.id==ITEM_NAMESPACE+2));
        }
    }
    #[test]
    fn recovered_delivery_consumes_owned_bag_and_replays_without_duplicate_xp() {
        use crate::{recovered_quests::{DEBT,KANTEN,ORVEN,BAG},progression::Command as P};
        let dir=crate::db::testdb::fresh();
        let xp;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Delivery Hero").unwrap();
            s.save_now().unwrap();
            assert!(s.recovered_delivery("local",1,KANTEN,100).is_err());assert!(!s.dirty());
            s.progression_command("local",1,P::AwardXp(100000)).unwrap();
            s.recovered_delivery("local",1,KANTEN,100).unwrap();
            assert_eq!(s.characters[&1].inventory.items().filter(|i|i.definition==BAG).count(),1);
            assert!(s.recovered_delivery("local",1,KANTEN,101).is_err());
            assert!(s.recovered_delivery("other",1,ORVEN,102).is_err());
            assert!(s.recovered_delivery("local",1,ORVEN,7300).is_err());
            // Renewing an expired job reuses the owned bag, never duplicates it.
            s.recovered_delivery("local",1,KANTEN,7300).unwrap();
            assert_eq!(s.characters[&1].inventory.items().filter(|i|i.definition==BAG).count(),1);
            s.recovered_delivery("local",1,ORVEN,7301).unwrap();
            xp=s.characters[&1].progression.xp;assert_eq!(xp,101000);
            assert!(s.recovered_delivery("local",1,ORVEN,7302).is_err());
            assert!(s.recovered_delivery("local",1,KANTEN,7302).is_err());
        }
        {
            let s=Store::open(&dir).unwrap();let c=&s.characters[&1];assert_eq!(c.progression.xp,xp);
            assert_eq!(c.quests.recovered.0[&DEBT].completed,7301);
            assert!(!c.inventory.items().any(|i|i.definition==BAG));
        }
    }
    #[test]
    fn exploration_conversations_replay_partial_and_completed_quests_exactly_once() {
        let dir=crate::db::testdb::fresh();
        let q=&crate::exploration_quests::catalog().quests[0];
        let referral=&crate::exploration_quests::catalog().referral;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Ringway Hero").unwrap();
            assert!(s.exploration_conversation("other",1,q.npc,100).is_err());
            assert!(s.exploration_conversation("local",1,q.objectives[0],100).is_err());
            s.exploration_conversation("local",1,q.npc,100).unwrap();
            s.exploration_conversation("local",1,q.objectives[0],101).unwrap();
            s.save_now().unwrap();
            s.exploration_conversation("local",1,q.objectives[0],102).unwrap();
            assert!(!s.dirty(),"duplicate touch must not change the character");
        }
        {
            let mut s=Store::open(&dir).unwrap();
            assert_eq!(s.characters[&1].quests.exploration.0[&q.id].phase,2);
            for (i,entity) in q.objectives[1..].iter().enumerate() {
                s.exploration_conversation("local",1,*entity,103+i as u64).unwrap();
            }
            s.exploration_conversation("local",1,q.npc,105).unwrap();
            assert_eq!(s.characters[&1].progression.xp,80);
            s.exploration_conversation("local",1,q.npc,106).unwrap();
            s.exploration_conversation("local",1,referral.npc,107).unwrap();
            let book=&s.characters[&1].quests;
            assert!(book.list_graph().unwrap().len()<48000);
            assert_eq!(book.updates().unwrap().len(),2);
        }
        {
            let mut s=Store::open(&dir).unwrap();
            assert_eq!(s.characters[&1].progression.xp,80);
            assert_eq!(s.characters[&1].quests.exploration.0[&referral.id].completed,107);
            s.exploration_conversation("local",1,q.npc,108).unwrap();
            assert_eq!(s.characters[&1].progression.xp,80);
        }
    }
    #[test]
    fn hunting_quests_replay_acceptance_kills_return_and_exactly_one_reward() {
        use crate::progression::Command::AwardXp;
        let dir=crate::db::testdb::fresh();
        let q=crate::hunting_quests::profile(0x4000fc07).unwrap();
        let time=crate::recovered_quests::now();let initial=447949214;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Foundry Hunter").unwrap();
            s.progression_command("local",1,AwardXp(initial)).unwrap();
            assert!(s.hunting_conversation("other",1,q.npc,time).is_err());
            s.hunting_conversation("local",1,q.npc,time).unwrap();
            for _ in 0..4 {s.encounter_reward("local",1,q.targets[0],&[AwardXp(1)]).unwrap();}
        }
        {
            let mut s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1].quests.hunting.0[&q.id].count,4);
            for _ in 4..10 {s.encounter_reward("local",1,q.targets[0],&[AwardXp(1)]).unwrap();}
            assert_eq!(s.characters[&1].progression.xp,initial+10,"kill must not pay quest XP");
            s.hunting_conversation("local",1,q.npc,time+1).unwrap();
            assert_eq!(s.characters[&1].progression.xp,initial+10+q.xp_cap);
            s.save_now().unwrap();
            s.hunting_conversation("local",1,q.npc,time+2).unwrap();
            assert!(!s.dirty());
        }
        {
            let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1].progression.xp,initial+10+q.xp_cap);
            assert_eq!(s.characters[&1].quests.hunting.0[&q.id].completed,time+1);
        }
    }
    #[test]
    fn arwic_lesson_replays_inventory_entitlement_and_reward_atomically() {
        use crate::arwic_mines::{Action,IRON,SWORD,GLYPH,GUIDE};
        let dir=crate::db::testdb::fresh();
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Mine Apprentice").unwrap();
            assert!(s.arwic_transaction("other",1,Action::Talk,100).is_err());
            s.arwic_transaction("local",1,Action::Talk,100).unwrap();
            s.arwic_transaction("local",1,Action::Hammer,101).unwrap();
        }
        {
            let mut s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1].quests.arwic.phase,2);
            s.arwic_transaction("local",1,Action::Talk,102).unwrap();
            let iron=crate::arwic_mines::owned(&s.characters[&1].inventory,IRON).unwrap();
            s.arwic_transaction("local",1,Action::Craft(iron),103).unwrap();
            assert!(s.characters[&1].inventory.items().any(|i|i.definition==SWORD));
        }
        {
            let mut s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1].quests.arwic.phase,4);
            assert!(s.characters[&1].quests.arwic.learned);
            s.arwic_transaction("local",1,Action::Talk,104).unwrap();
            s.save_now().unwrap();
            s.arwic_transaction("local",1,Action::Talk,105).unwrap();
            assert!(!s.dirty());
        }
        {
            let s=Store::open(&dir).unwrap();let c=&s.characters[&1];
            assert_eq!(c.progression.xp,150);assert_eq!(c.quests.arwic.completed,104);
            assert!(c.quests.arwic.learned);
            for definition in [GLYPH,GUIDE] {assert_eq!(c.inventory.items().filter(|i|i.definition==definition).count(),1);}
            assert!(!c.inventory.items().any(|i|i.definition==SWORD));
        }
    }
    #[test]
    fn mixed_versions_retain_exact_native_appearance_after_restart() {
        let dir=crate::db::testdb::fresh();
        let original = request("Native Hero");
        {
            let mut store = Store::open(&dir).unwrap();
            assert_eq!(store.create("local", "Legacy Hero").unwrap().creation, None);
            let created = store.create_native("local", &original).unwrap();
            assert_eq!(created.creation.as_ref(), Some(&original));
            assert!(
                store
                    .create_native("other", &request("native hero"))
                    .is_err()
            );
            for i in 0..3 {
                store
                    .create_native("local", &request(&format!("Extra {i}")))
                    .unwrap();
            }
            assert!(store.create_native("local", &request("Sixth")).is_err());
        }
        {
            let store = Store::open(&dir).unwrap();
            assert_eq!(store.characters.len(), 5);
            assert_eq!(store.characters[&1].creation, None);
            assert_eq!(store.characters[&2].creation.as_ref(), Some(&original));
        }
    }
    #[test]
    fn periodic_saves_run_when_due_and_a_crash_keeps_the_last_save() {
        let dir=crate::db::testdb::fresh();
        let mut s=Store::open(&dir).unwrap();
        let id=s.create("local","Saver").unwrap().id;
        s.progression_command("local",id,crate::progression::Command::AwardXp(100)).unwrap();
        assert!(!s.save_due().unwrap(),"the interval has not passed");
        s.set_save_interval(std::time::Duration::ZERO);
        assert!(s.save_due().unwrap());
        s.writer().drain().unwrap();
        s.progression_command("local",id,crate::progression::Command::AwardXp(50)).unwrap();
        s.abandon();
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&id].progression.xp,100,"the unsaved award is lost with the crash, the saved one is kept");
    }
    #[test]
    fn ground_items_and_both_owners_save_in_one_snapshot() {
        let dir=crate::db::testdb::fresh();
        let (a,b,ground,expected);
        {
            let mut s=Store::open(&dir).unwrap();
            a=s.create("local","Dropper").unwrap().id;
            b=s.create("local","Finder").unwrap().id;
            let item=s.next_item_id().unwrap();
            s.inventory_command("local",a,crate::inventory::Command::Grant(crate::item_services::potion(item,0))).unwrap();
            let dropped=s.drop_item("local",a,item,Placement::HUMAN_START).unwrap();
            ground=dropped.id();
            s.save_now().unwrap();
            {let r=Store::open(&dir);assert!(r.is_err(),"a second server on the same database is refused");}
            s.pickup_item("local",b,ground).unwrap();
            expected=(s.characters[&a].clone(),s.characters[&b].clone());
        }
        let s=Store::open(&dir).unwrap();
        assert!(s.drops.is_empty() && !s.drops.contains_key(&ground));
        assert_eq!((s.characters[&a].clone(),s.characters[&b].clone()),expected);
        assert_eq!(s.characters[&b].inventory.items().count(),1);
        assert_eq!(s.characters[&a].inventory.items().count(),0);
    }
    #[test]
    fn offline_credit_is_saved_where_sql_can_read_it() {
        let dir=crate::db::testdb::fresh();
        let id;
        {
            let mut s=Store::open(&dir).unwrap();
            id=s.create("local","Vassal").unwrap().id;
            s.credit_offline(id,250).unwrap();
            assert!(s.credit_offline(id+1,1).is_err());
        }
        let mut client=dir.connect().unwrap();
        let row=client.query_one("SELECT xp, available_xp FROM characters WHERE id=$1",&[&(id as i64)]).unwrap();
        assert_eq!((row.get::<_,i64>(0),row.get::<_,i64>(1)),(250,250));
        // An external SQL credit of an offline character is read at the next start.
        client.execute("UPDATE characters SET xp=xp+10, available_xp=available_xp+10 WHERE id=$1",&[&(id as i64)]).unwrap();
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&id].progression.xp,260);
    }
    #[test]
    fn names_are_unique_and_creation_requests_are_validated() {
        let dir=crate::db::testdb::fresh();
        {
            let mut s=Store::open(&dir).unwrap();
            assert_eq!(s.create("local","Explorer").unwrap().id,1);
            assert!(s.create("local","explorer").is_err());
            let mut bad=request("Bad Values");
            bad.physical_values.push(bad.physical_values[0]);
            assert!(s.create_native("local",&bad).is_err(),"duplicate physical value");
            bad.physical_values=vec![(1,f32::NAN.to_bits())];
            assert!(s.create_native("local",&bad).is_err(),"non-finite physical value");
            bad.physical_values=(0..257).map(|i|(i,0)).collect();
            assert!(s.create_native("local",&bad).is_err(),"too many physical values");
            assert_eq!(s.characters.len(),1,"refused requests create nothing");
        }
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&1].name,"Explorer");
        assert_eq!(s.characters.len(),1);
    }
    #[test]
    fn placements_save_and_unknown_characters_are_refused() {
        let dir=crate::db::testdb::fresh();
        let mut last=Placement::HUMAN_START;
        {
            let mut s=Store::open(&dir).unwrap();
            s.create("local","Walker").unwrap();
            assert!(s.set_placement(2,last).is_err());
            s.set_placement(1,last).unwrap();
            s.save_now().unwrap();
            last.coordinates[0]+=4.0;
            s.set_placement(1,last).unwrap();
        }
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.placement(1),Some(last));
    }
    #[test]
    fn avatar_lifetimes_survive_restart() {
        let dir=crate::db::testdb::fresh();
        {
            let mut store=Store::open(&dir).unwrap();
            store.create("local","Hero").unwrap();
            assert!(store.begin_instance("other",1).is_err());
            assert_eq!(store.begin_instance("local",1).unwrap(),1);
            store.delete("local",1).unwrap();
            assert!(store.begin_instance("local",1).is_err());
            store.restore("local",1).unwrap();
        }
        let mut store=Store::open(&dir).unwrap();
        assert_eq!(store.begin_instance("local",1).unwrap(),2);
        store.characters.get_mut(&1).unwrap().instance=u16::MAX;
        assert!(store.begin_instance("local",1).is_err());
    }
    #[test]
    fn an_encounter_payout_round_trips() {
        let dir=crate::db::testdb::fresh();
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Kill Time")).unwrap();
            let id=s.next_item_id().unwrap();
            let items=[crate::inventory::Command::Grant(crate::item_services::potion(id,0))];
            s.encounter_transaction("local",c.id,0x4700158b,&items,&[crate::progression::Command::AwardXp(5)]).unwrap();
            saved=s.characters[&c.id].clone();
        }
        let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&saved.id],saved);
    }
}
