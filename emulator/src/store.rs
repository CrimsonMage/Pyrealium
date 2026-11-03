//! Transactional local character journal with checked progression compaction.
//! A write is acknowledged only after fsync.
//! Corruption/torn tails stop startup rather than silently discarding characters.
use crate::movement::Placement;
use crate::world::CharacterCreationRequest;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PHYSICAL_VALUES: usize = 256;
mod compaction;
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
    /// Not journaled: the world-clock second each skill was last used since the server
    /// started, sent as SkillData's last-use stamp (`ui::skill_object`).
    pub skill_used: BTreeMap<u32, f64>,
}

pub struct Store {
    file: File,
    journal_path: PathBuf,
    compact_after: u64,
    lock: PathBuf,
    pub characters: BTreeMap<u64, Character>,
    pub drops: BTreeMap<u64, DroppedItem>,
    poisoned: bool,
    positions_path: PathBuf,
    positions: BTreeMap<u64, Placement>,
    positions_dirty: bool,
    revision: u64,
    /// Tradeskill craft experience and ranks per character (`tradeskills.rs`), one line each in a
    /// side file beside the journal, rewritten atomically on change like the positions snapshot.
    trade_path: PathBuf,
    trade: BTreeMap<u64, String>,
}

fn load_trade(path: &std::path::Path) -> io::Result<BTreeMap<u64, String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let (id, rest) = line.split_once('\t').ok_or_else(|| invalid("tradeskill line"))?;
        map.insert(id.parse().map_err(|_| invalid("tradeskill owner"))?, rest.to_owned());
    }
    Ok(map)
}
fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}
fn hash(bytes: &[u8]) -> u32 {
    bytes.iter().fold(2166136261u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(16777619)
    })
}
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b' ' || b == b'-')
}

impl Store {
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let lock = dir.join("server.lock");
        let mut guard = OpenOptions::new().write(true).create_new(true).open(&lock)
            .map_err(|e| io::Error::new(e.kind(), format!("cannot lock {}: {e}; after a crash, verify no server is running before removing this lock", lock.display())))?;
        let result = (|| {
            writeln!(guard, "{}", std::process::id())?;
            guard.sync_all()?;
            let mut file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(dir.join("characters.journal"))?;
            if file.metadata()?.len() > MAX_JOURNAL_BYTES {
                return Err(invalid("journal exceeds 16 MiB limit"));
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let journal = decode_journal_state(&bytes)?;
            let characters = journal.characters;
            let positions_path = dir.join("positions.snapshot");
            let (position_revision, mut positions) = match File::open(&positions_path) {
                Ok(file) => {
                    let mut bytes = Vec::new();
                    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
                    decode_position_snapshot(&bytes, &characters)?
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (0, BTreeMap::new()),
                Err(e) => return Err(e),
            };
            if position_revision > journal.revision {
                return Err(invalid("position snapshot is newer than character journal"));
            }
            let mut positions_dirty = false;
            for (id, (revision, placement)) in journal.relocations {
                if revision > position_revision {
                    positions.insert(id, placement);
                    positions_dirty = true;
                }
            }
            Ok(Self {
                file,
                journal_path: dir.join("characters.journal"),
                compact_after: MAX_JOURNAL_BYTES / 2,
                lock: lock.clone(),
                characters,
                drops: journal.drops,
                poisoned: false,
                positions_path,
                positions,
                positions_dirty,
                revision: journal.revision,
                trade: load_trade(&dir.join("tradeskills.tsv"))?,
                trade_path: dir.join("tradeskills.tsv"),
            })
        })();
        if result.is_err() {
            drop(guard);
            let _ = fs::remove_file(lock);
        }
        result
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
        self.append_record(&format!("t4\t{id}\t{first}"))?;
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
        // One fsync publishes the item, basic attack and once-per-character marker.
        // t1 retains its original item-only meaning when replaying older stores.
        self.append_record(&format!("t2\t{id}\t{item}"))?;
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
        // One durable record grants all five items and the once-only marker.
        self.append_record(&format!("t3\t{id}\t{first}"))?;
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
        self.append_record(&format!("k4\t{id}"))?;
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
        self.append_record(&format!("k5\t{id}"))?;
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
        let revision = if quests != character.quests { "l3" } else { "l2" };
        self.append_record(&format!("{revision}\t{id}\t{}", encode_placement(placement)))?;
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
        self.append_record(&format!("k3\t{id}"))?;
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
        self.append_record(&format!("p2\t{id}\t{}", encode_placement(placement)))?;
        self.positions.insert(id, placement);
        self.positions_dirty = true;
        Ok(())
    }
    pub fn relocate_training_portal(&mut self, account: &str, id: u64, placement: Placement, portal: u64) -> io::Result<()> {
        let character = self.characters.get(&id).filter(|c| c.account == account && !c.deleted && c.progression.alive()).ok_or_else(|| invalid("Travel requires an owned living character"))?;
        if character.quests.journey_phase() != 2 || !crate::quests::is_training_portal(portal) {
            return self.relocate(account, id, placement);
        }
        if !placement.valid() { return Err(invalid("Invalid training portal destination")); }
        let quests = character.quests.apply(&crate::quests::Command::Travelled).map_err(io::Error::other)?;
        self.append_record(&format!("p3\t{id}\t{portal:016x}\t{}", encode_placement(placement)))?;
        self.characters.get_mut(&id).unwrap().quests = quests;
        self.positions.insert(id, placement); self.positions_dirty = true;
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
        self.append_record(&format!(
            "{}\t{id}\t{}",
            if resurrect { "r2" } else { "p1" },
            encode_placement(placement)
        ))?;
        self.characters.get_mut(&id).unwrap().progression = next;
        self.positions.insert(id, placement);
        self.positions_dirty = true;
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
        self.append_record(&format!("g1\t{id}\t{}", command.encode()))?;
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
        let cost = character
            .progression
            .available_xp
            .checked_sub(progression.available_xp)
            .filter(|cost| *cost > 0)
            .ok_or_else(|| invalid("invalid advancement cost"))?;
        self.append_record(&format!("k2\t{id}\t{skill:x}\t{cost}"))?;
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
        self.append_record(&format!("k1\t{id}\t{}", command.encode()))?;
        self.characters.get_mut(&id).unwrap().skills = next;
        Ok(())
    }
    pub fn debit_untraining(&mut self,id:u64)->io::Result<bool> {
        let c=self.characters.get(&id).filter(|c|!c.deleted).ok_or_else(||invalid("untraining character unavailable"))?;
        let Some((skills,progression))=c.skills.debit_untraining(&c.progression).map_err(io::Error::other)? else{return Ok(false)};
        // Replay uses the persisted earned-XP total; no clock or client quantity.
        self.append_record(&format!("ku1\t{id}"))?;
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
        let encoded = commands
            .iter()
            .map(crate::progression::Command::encode)
            .collect::<Vec<_>>()
            .join(";");
        self.append_record(&format!("g2\t{id}\t{encoded}"))?;
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
        let revision = match command {
            crate::quests::Command::ShelterGuard | crate::quests::Command::ShelterLieutenant | crate::quests::Command::ShelterSentry => "q5",
            crate::quests::Command::Scene(_) => "q6",
            crate::quests::Command::AcceptSupplies => "q3",
            crate::quests::Command::AcceptPotion | crate::quests::Command::ClaimPotion | crate::quests::Command::AcceptJourney | crate::quests::Command::ClaimJourney => "q4",
            _ => "q1",
        };
        self.append_record(&format!("{revision}\t{id}\t{}", command.encode()))?;
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
        self.append_record(&format!("q9\t{id}\t{entity}\t{time}"))?;
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
        self.append_record(&format!("qh{}\t{id}\t{entity}\t{time}",crate::hunting_quests::VERSION))?;
        let c=self.characters.get_mut(&id).unwrap();c.quests.hunting=result.book;c.progression=state;Ok(())
    }
    /// Commit the canonical inferred delivery exchange and journal together.
    pub fn recovered_delivery(&mut self,account:&str,id:u64,npc:u32,time:u64)->io::Result<()> {
        let first=self.next_item_id()?;
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Quest owner unavailable"))?;
        let (items,changes,book)=c.quests.recovered.delivery(&c.inventory,npc,c.progression.level().number,time,first)?;
        let (bag,state)=if items.is_empty()&&changes.is_empty(){(c.inventory.clone(),c.progression.clone())}else{
            crate::player_transaction::candidate(&c.inventory,&c.progression,&items,&changes,Some(first))?};
        self.append_record(&format!("q8\t{id}\t{npc}\t{time}\t{first}"))?;
        let c=self.characters.get_mut(&id).unwrap();c.inventory=bag;c.progression=state;c.quests.recovered=book;Ok(())
    }
    /// Consume a supported hunt elixir and bestow its journal in one durable event.
    pub fn recovered_elixir(&mut self,account:&str,id:u64,item:u64)->io::Result<()> {
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Quest owner unavailable"))?;
        let owned=c.inventory.items().find(|i|i.id==item&&matches!(i.location,crate::inventory::Location::Bag(_)))
            .ok_or_else(||invalid("Quest starter must be in your backpack"))?;
        let q=crate::recovered_quests::use_profile(owned.definition).ok_or_else(||invalid("Unsupported quest starter"))?;
        let time=crate::recovered_quests::now();
        let book=c.quests.recovered.accept(q.id,c.progression.level().number,time)?;
        let bag=c.inventory.apply(&crate::inventory::Command::ConsumeDense{item}).map_err(io::Error::other)?;
        self.append_record(&format!("q7\t{id}\t{item}\t{}\t{time}",q.id))?;
        let c=self.characters.get_mut(&id).unwrap();c.inventory=bag;c.quests.recovered=book;Ok(())
    }
    /// Moves each character's state of a retired hunt (`hunting_quests::retired`) onto the data
    /// quest of the same journal id, as one record per character (`qm1`): an accepted run keeps its
    /// start time at the stage of its kills (1 + kills; the report stage at the goal), a solved one
    /// is the completed stage with its solve time, and the hunting book drops it. Run at start,
    /// after the replay; returns how many characters moved.
    pub fn migrate_retired_hunts(&mut self)->io::Result<usize> {
        let mut moved=0;
        let ids:Vec<u64>=self.characters.keys().copied().collect();
        for id in ids {
            let c=&self.characters[&id];
            if c.deleted {continue;}
            let commands=retired_hunt_commands(&c.quests);
            if commands.is_empty() {continue;}
            let book=commands.iter().try_fold(c.quests.clone(),|b,q|b.apply(q)).map_err(io::Error::other)?;
            self.append_record(&format!("qm1\t{id}\t{}",commands.iter().map(|q|q.encode()).collect::<Vec<_>>().join(";")))?;
            self.characters.get_mut(&id).unwrap().quests=book;
            moved+=1;
        }
        Ok(moved)
    }
    /// A quest emote's outcome (`quest_emotes`): its quest stages, items, experience and move are
    /// one fsynced record (`e1`), replayed as recorded.
    pub fn emote_transaction(&mut self,account:&str,id:u64,outcome:&crate::quest_emotes::Outcome)->io::Result<()> {
        let first=self.next_item_id()?;
        let c=self.characters.get(&id).filter(|c|c.account==account && !c.deleted && c.progression.alive())
            .ok_or_else(||invalid("Quest exchange owner unavailable"))?;
        let next=apply_emote(c,first,&outcome.quests,&outcome.items,&outcome.changes)?;
        let quests=outcome.quests.iter().map(|q|q.encode()).collect::<Vec<_>>().join(";");
        let placement=outcome.moveto.map_or("-".into(),encode_placement);
        self.append_record(&format!("e1\t{id}\t{first}\t{quests}\t{}\t{}\t{placement}",
            crate::player_transaction::encode_items(&outcome.items),crate::player_transaction::encode_changes(&outcome.changes)))?;
        let c=self.characters.get_mut(&id).unwrap();(c.quests,c.inventory,c.progression)=next;
        if let Some(placement)=outcome.moveto {self.positions.insert(id,placement);self.positions_dirty=true;}
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
        self.append_record(&format!("qk2\t{id}\t{entity:08x}\t{time}\t{}",crate::player_transaction::encode_changes(commands)))?;
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
        self.append_record(&format!(
            "ek1\t{id}\t{entity:08x}\t{}\t{}\t{time}",
            crate::player_transaction::encode_items(items),
            crate::player_transaction::encode_changes(changes)
        ))?;
        let character = self.characters.get_mut(&id).unwrap();
        character.inventory = inventory;
        character.progression = progression;
        character.quests = quests;
        Ok(())
    }
    /// Items and payment/rewards become visible only after one durable record.
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
        let revision = if supplies { "a2" } else if practice { "a3" } else { "a1" };
        self.append_record(&format!(
            "{revision}\t{id}\t{}\t{}",
            crate::player_transaction::encode_items(items),
            crate::player_transaction::encode_changes(changes)
        ))?;
        let character = self.characters.get_mut(&id).unwrap();
        character.inventory = inventory;
        character.progression = progression;
        character.quests = quests;
        Ok(())
    }
    /// Inventory, lesson entitlement, journal and XP are committed together.
    pub fn arwic_transaction(&mut self,account:&str,id:u64,action:crate::arwic_mines::Action,time:u64)->io::Result<()> {
        let first=self.next_item_id()?;
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Lesson owner unavailable"))?;
        let plan=crate::arwic_mines::plan(&c.quests,&c.inventory,action,first,time)?;
        if plan.quests==c.quests && plan.items.is_empty() && plan.rewards.is_empty() {return Ok(());}
        let (bag,state)=if plan.items.is_empty()&&plan.rewards.is_empty(){(c.inventory.clone(),c.progression.clone())}else{crate::player_transaction::candidate(&c.inventory,&c.progression,&plan.items,&plan.rewards,Some(first))?};
        self.append_record(&format!("m1\t{id}\t{}\t{time}\t{first}",action.encode()))?;
        let c=self.characters.get_mut(&id).unwrap();c.inventory=bag;c.progression=state;c.quests=plan.quests;Ok(())
    }
    /// Inventory/cost and profession XP share one fsynced journal record.
    pub fn profession_transaction(&mut self,account:&str,id:u64,items:&[crate::inventory::Command],changes:&[crate::progression::Command],change:&crate::professions::Change)->io::Result<()> {
        let c=self.characters.get(&id).filter(|c|c.account==account&&!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Crafting owner unavailable"))?;
        let book=c.professions.apply(change)?;
        let first=items.iter().any(|i|i.allocated_id().is_some()).then(||self.next_item_id()).transpose()?;
        let (bag,state)=if items.is_empty()&&changes.is_empty(){(c.inventory.clone(),c.progression.clone())}else{crate::player_transaction::candidate(&c.inventory,&c.progression,items,changes,first)?};
        self.append_record(&format!("c1\t{id}\t{}\t{}\t{}",crate::player_transaction::encode_items(items),crate::player_transaction::encode_changes(changes),serde_json::to_string(change).map_err(io::Error::other)?))?;
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
        self.append_record(&format!("i1\t{id}\t{}", command.encode()))?;
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
        let bytes = settings.encode();
        let mut hex = String::with_capacity(bytes.len() * 2);
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            hex.push(DIGITS[(byte >> 4) as usize] as char);
            hex.push(DIGITS[(byte & 15) as usize] as char);
        }
        self.append_record(&format!("u1\t{id}\t{hex}"))?;
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
        let hex = options.encode().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        self.append_record(&format!("o1\t{id}\t{hex}"))?;
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
        let bytes = update
            .encode()
            .map_err(|_| invalid("invalid hotbar update"))?;
        let mut hex = String::with_capacity(bytes.len() * 2);
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            hex.push(DIGITS[(byte >> 4) as usize] as char);
            hex.push(DIGITS[(byte & 15) as usize] as char);
        }
        self.append_record(&format!("h1\t{id}\t{hex}"))?;
        self.characters.get_mut(&id).unwrap().hotbar = next;
        Ok(())
    }
    pub fn set_placement(&mut self, id: u64, placement: Placement) -> io::Result<()> {
        if self.characters.get(&id).is_none_or(|c| c.deleted) || !placement.valid() {
            return Err(invalid("invalid character placement"));
        }
        if self.positions.get(&id) != Some(&placement) {
            self.positions.insert(id, placement);
            self.positions_dirty = true;
        }
        Ok(())
    }
    /// One bounded snapshot per checkpoint, not an fsync for every movement packet.
    /// The old snapshot remains authoritative until the flushed new file is renamed.
    /// The saved tradeskill line of an owned, live character.
    pub fn trade_get(&self, id: u64) -> Option<&str> {
        self.characters.get(&id).filter(|c| !c.deleted)?;
        self.trade.get(&id).map(String::as_str)
    }
    pub fn trade_put(&mut self, id: u64, line: String) -> io::Result<()> {
        if self.characters.get(&id).is_none_or(|c| c.deleted) || line.len() > 4096 || line.contains(['\n', '\t']) {
            return Err(invalid("tradeskill state for an unavailable character"));
        }
        self.trade.insert(id, line);
        let mut text = String::new();
        for (id, line) in &self.trade {
            text.push_str(&format!("{id}\t{line}\n"));
        }
        let temporary = self.trade_path.with_extension("tsv.tmp");
        fs::write(&temporary, text)?;
        fs::rename(temporary, &self.trade_path)
    }
    pub fn flush_positions(&mut self) -> io::Result<()> {
        if !self.positions_dirty {
            return Ok(());
        }
        let size = 24usize
            .checked_add(
                self.positions
                    .len()
                    .checked_mul(25)
                    .ok_or_else(|| invalid("too many placements"))?,
            )
            .filter(|size| *size <= 1024 * 1024)
            .ok_or_else(|| invalid("placement snapshot limit"))?;
        let mut bytes = Vec::with_capacity(size);
        bytes.extend(b"AC2POS02");
        bytes.extend(self.revision.to_le_bytes());
        bytes.extend((self.positions.len() as u32).to_le_bytes());
        for (id, placement) in &self.positions {
            bytes.extend(id.to_le_bytes());
            bytes.extend(placement.cell.to_le_bytes());
            for value in placement.coordinates {
                bytes.extend(value.to_le_bytes());
            }
            bytes.push(placement.heading);
        }
        bytes.extend(hash(&bytes).to_le_bytes());
        let temporary = self.positions_path.with_extension("snapshot.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, &self.positions_path)?;
        self.positions_dirty = false;
        Ok(())
    }
    /// Persists structurally valid input. The caller must validate entity/world authority.
    pub fn create_native(
        &mut self,
        account: &str,
        request: &CharacterCreationRequest,
    ) -> io::Result<Character> {
        self.create_record(account, &request.name, Some(request))
    }
    fn create_record(
        &mut self,
        account: &str,
        name: &str,
        creation: Option<&CharacterCreationRequest>,
    ) -> io::Result<Character> {
        if self.poisoned {
            return Err(invalid(
                "journal write failed previously; restart and inspect journal",
            ));
        }
        if !valid_name(account) || !valid_name(name) {
            return Err(invalid(
                "account and character names must contain 1-32 ASCII letters, digits, spaces, hyphens or underscores",
            ));
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
        let line = match creation {
            None => format!("v1\t{}\t{}\t{}", c.id, c.account, c.name),
            Some(request) => format!(
                "v2\t{}\t{}\t{}\t{}",
                c.id,
                c.account,
                c.name,
                encode_creation(request)?
            ),
        };
        self.append_record(&line)?;
        self.characters.insert(id, c.clone());
        Ok(c)
    }
    pub fn delete(&mut self, account: &str, id: u64) -> io::Result<()> {
        if !self
            .characters
            .get(&id)
            .is_some_and(|c| c.account == account && !c.deleted)
        {
            return Err(invalid("character not owned or already deleted"));
        }
        self.append_record(&format!("d1\t{id}"))?;
        self.characters.get_mut(&id).unwrap().deleted = true;
        Ok(())
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
        self.append_record(&format!("r1\t{id}"))?;
        self.characters.get_mut(&id).unwrap().deleted = false;
        Ok(())
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
        self.append_record(&format!("e1\t{id}\t{instance}"))?;
        self.characters.get_mut(&id).unwrap().instance = instance;
        Ok(instance)
    }
    fn append_record(&mut self, line: &str) -> io::Result<()> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("journal revision exhausted"))?;
        if self.poisoned {
            return Err(invalid(
                "journal write failed previously; restart and inspect journal",
            ));
        }
        let record = format!("{line}\t{:08x}\n", hash(line.as_bytes()));
        if self
            .file
            .metadata()?
            .len()
            .saturating_add(record.len() as u64)
            > self.compact_after
        {
            self.compact_journal()?;
        }
        if self
            .file
            .metadata()?
            .len()
            .saturating_add(record.len() as u64)
            > MAX_JOURNAL_BYTES
        {
            return Err(invalid("journal exceeds 16 MiB limit"));
        }
        if let Err(e) = self
            .file
            .write_all(record.as_bytes())
            .and_then(|()| self.file.sync_all())
        {
            self.poisoned = true;
            return Err(e);
        }
        self.revision = revision;
        Ok(())
    }
    /// Bound repetitive progression history while retaining every non-progression
    /// transaction and verifying complete replay before replacing the file.
    pub fn compact_journal(&mut self) -> io::Result<bool> {
        if self.poisoned {
            return Err(invalid("cannot compact a poisoned journal"));
        }
        let mut raw = Vec::new();
        File::open(&self.journal_path)?
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_end(&mut raw)?;
        let mut lines = Vec::new();
        let before = match decode_journal_collect(&raw, Some(&mut lines)) {
            Ok(state) => state,
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        if before.characters != self.characters || before.drops != self.drops || before.revision != self.revision {
            self.poisoned = true;
            return Err(invalid("journal differs from active character state"));
        }
        let base = self
            .revision
            .checked_sub(lines.len() as u64)
            .ok_or_else(|| invalid("compaction revision underflow"))?;
        let header = format!("j1\t{base}");
        let mut bytes = format!("{header}\t{:08x}\n", hash(header.as_bytes())).into_bytes();
        for line in lines {
            bytes.extend(line.as_bytes());
            bytes.push(b'\n');
        }
        let next_attempt = (raw.len() as u64)
            .saturating_add(1024 * 1024)
            .clamp(MAX_JOURNAL_BYTES / 2, MAX_JOURNAL_BYTES);
        if bytes.len() >= raw.len() {
            self.compact_after = next_attempt;
            return Ok(false);
        }
        let after = match decode_journal_state(&bytes) {
            Ok(state) => state,
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        if after.characters != before.characters
            || after.drops != before.drops
            || after.revision != before.revision
            || after.relocations.len() != before.relocations.len()
            || before.relocations.iter().any(|(id, (_, position))| {
                after
                    .relocations
                    .get(id)
                    .is_none_or(|(_, other)| position != other)
            })
        {
            self.poisoned = true;
            return Err(invalid("compaction replay differs from original state"));
        }
        // Anchor all old relocation revisions before rebasing their replay order.
        // Both the old and compacted journals end at this same revision.
        self.positions_dirty = true;
        self.flush_positions()?;
        let temporary = self.journal_path.with_extension("journal.compact.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        let backup = self.journal_path.with_extension("journal.previous");
        fs::copy(&self.journal_path, &backup)?;
        OpenOptions::new().write(true).open(&backup)?.sync_all()?;
        fs::rename(&temporary, &self.journal_path)?;
        match OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.journal_path)
        {
            Ok(file) => self.file = file,
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        }
        self.compact_after = (bytes.len() as u64)
            .saturating_add(1024 * 1024)
            .clamp(MAX_JOURNAL_BYTES / 2, MAX_JOURNAL_BYTES);
        eprintln!(
            "Compacted character journal: {} -> {} bytes at revision {}.",
            raw.len(),
            bytes.len(),
            self.revision
        );
        Ok(true)
    }
}

#[cfg(test)]
fn decode_positions(
    bytes: &[u8],
    characters: &BTreeMap<u64, Character>,
) -> io::Result<BTreeMap<u64, Placement>> {
    decode_position_snapshot(bytes, characters).map(|(_, positions)| positions)
}
fn decode_position_snapshot(
    bytes: &[u8],
    characters: &BTreeMap<u64, Character>,
) -> io::Result<(u64, BTreeMap<u64, Placement>)> {
    if bytes.len() < 16 || bytes.len() > 1024 * 1024 {
        return Err(invalid("invalid placement snapshot header/size"));
    }
    let (revision, start, base) = match &bytes[..8] {
        b"AC2POS01" => (0, 12, 16),
        b"AC2POS02" if bytes.len() >= 24 => {
            (u64::from_le_bytes(bytes[8..16].try_into().unwrap()), 20, 24)
        }
        _ => return Err(invalid("invalid placement snapshot version")),
    };
    let count = u32::from_le_bytes(bytes[start - 4..start].try_into().unwrap()) as usize;
    if count.checked_mul(25).and_then(|n| n.checked_add(base)) != Some(bytes.len()) {
        return Err(invalid("invalid placement snapshot length"));
    }
    let end = bytes.len() - 4;
    if hash(&bytes[..end]) != u32::from_le_bytes(bytes[end..].try_into().unwrap()) {
        return Err(invalid("placement snapshot checksum mismatch"));
    }
    let mut positions = BTreeMap::new();
    for record in bytes[start..end].chunks_exact(25) {
        let id = u64::from_le_bytes(record[..8].try_into().unwrap());
        let placement = Placement {
            cell: u32::from_le_bytes(record[8..12].try_into().unwrap()),
            coordinates: std::array::from_fn(|i| {
                f32::from_le_bytes(record[12 + i * 4..16 + i * 4].try_into().unwrap())
            }),
            heading: record[24],
        };
        if !characters.contains_key(&id)
            || !placement.valid()
            || positions.insert(id, placement).is_some()
        {
            return Err(invalid("unknown, duplicate or invalid saved placement"));
        }
    }
    Ok((revision, positions))
}
impl Drop for Store {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock);
    }
}

pub fn decode_journal(bytes: &[u8]) -> io::Result<BTreeMap<u64, Character>> {
    decode_journal_state(bytes).map(|journal| journal.characters)
}
struct JournalState {
    characters: BTreeMap<u64, Character>,
    drops: BTreeMap<u64, DroppedItem>,
    relocations: BTreeMap<u64, (u64, Placement)>,
    revision: u64,
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
fn encode_placement(placement: Placement) -> String {
    format!(
        "{:08x},{:08x},{:08x},{:08x},{}",
        placement.cell,
        placement.coordinates[0].to_bits(),
        placement.coordinates[1].to_bits(),
        placement.coordinates[2].to_bits(),
        placement.heading
    )
}
fn decode_placement(text: &str) -> io::Result<Placement> {
    if text.len() > 64 {
        return Err(invalid("placement field limit"));
    }
    let fields: Vec<_> = text.split(',').collect();
    if fields.len() != 5 {
        return Err(invalid("placement field count"));
    }
    let word = |index: usize| {
        u32::from_str_radix(fields[index], 16).map_err(|_| invalid("invalid placement word"))
    };
    let placement = Placement {
        cell: word(0)?,
        coordinates: [
            f32::from_bits(word(1)?),
            f32::from_bits(word(2)?),
            f32::from_bits(word(3)?),
        ],
        heading: fields[4]
            .parse()
            .map_err(|_| invalid("invalid placement heading"))?,
    };
    if !placement.valid() {
        return Err(invalid("invalid persisted placement"));
    }
    Ok(placement)
}
fn decode_journal_state(bytes: &[u8]) -> io::Result<JournalState> {
    decode_journal_collect(bytes, None)
}
fn decode_journal_collect(
    bytes: &[u8],
    mut compacted: Option<&mut Vec<String>>,
) -> io::Result<JournalState> {
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(invalid("journal exceeds 16 MiB limit"));
    }
    if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
        return Err(invalid("incomplete character journal tail"));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("invalid character journal encoding"))?;
    let mut records: BTreeMap<u64, Character> = BTreeMap::new();
    let mut drops = BTreeMap::new();
    let mut relocations = BTreeMap::new();
    let mut revision = 0u64;
    let mut run = compaction::Run::default();
    for (index, line) in text.lines().enumerate() {
        revision = revision
            .checked_add(1)
            .ok_or_else(|| invalid("journal revision exhausted"))?;
        let (data, sum) = line
            .rsplit_once('\t')
            .ok_or_else(|| invalid("missing journal checksum"))?;
        if u32::from_str_radix(sum, 16).ok() != Some(hash(data.as_bytes())) {
            return Err(invalid("character journal checksum mismatch"));
        }
        let fields: Vec<_> = data.split('\t').collect();
        if fields.len() == 2 && fields[0] == "j1" {
            if index != 0 {
                return Err(invalid("journal revision base must be first"));
            }
            revision = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid journal revision base"))?;
            continue;
        }
        if let Some(output) = compacted.as_mut() {
            if !(fields.len() == 3 && matches!(fields[0], "g1" | "g2" | "g3")) {
                run.flush(output);
                output.push(line.to_owned());
            }
        }
        if fields.len() == 3 && matches!(fields[0], "q1" | "q3" | "q4" | "q5" | "q6") {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid quest owner"))?;
            let command = crate::quests::Command::decode(fields[2]).map_err(io::Error::other)?;
            if !(fields[0] == "q5" && matches!(command, crate::quests::Command::ShelterGuard | crate::quests::Command::ShelterLieutenant | crate::quests::Command::ShelterSentry)
                || fields[0] == "q1" && matches!(command, crate::quests::Command::Accept | crate::quests::Command::Claim)
                || fields[0] == "q6" && matches!(command, crate::quests::Command::Scene(_))
                || fields[0] == "q3" && matches!(command, crate::quests::Command::AcceptSupplies)
                || fields[0] == "q4" && matches!(command, crate::quests::Command::AcceptPotion | crate::quests::Command::ClaimPotion | crate::quests::Command::AcceptJourney | crate::quests::Command::ClaimJourney)) {
                return Err(invalid("quest command does not match its journal revision"));
            }
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted && c.progression.alive())
                .ok_or_else(|| invalid("unavailable quest owner"))?;
            character.quests = character.quests.apply(&command).map_err(io::Error::other)?;
            for reward in crate::quests::rewards(&command) {
                character.progression = character
                    .progression
                    .apply(&reward)
                    .map_err(io::Error::other)?;
            }
            continue;
        }
        if fields.len()==4 && fields[0]=="q9" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("exploration owner"))?;
            let entity=fields[2].parse::<u32>().map_err(|_|invalid("exploration NPC"))?;
            let time=fields[3].parse::<u64>().map_err(|_|invalid("exploration time"))?;
            let c=records.get_mut(&id).filter(|c|!c.deleted&&c.progression.alive()).ok_or_else(||invalid("exploration owner unavailable"))?;
            let result=c.quests.exploration.converse(entity,c.progression.level().number,time)?;
            if result.book==c.quests.exploration {return Err(invalid("exploration event did not advance"));}
            if result.xp>0 {c.progression=c.progression.apply(&crate::progression::Command::AwardXp(result.xp)).map_err(io::Error::other)?;}
            c.quests.exploration=result.book;continue;
        }
        if fields.len()==4 && matches!(fields[0],"qh1"|"qh2"|"qh3") {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("hunting owner"))?;
            let entity=fields[2].parse::<u32>().map_err(|_|invalid("hunting NPC"))?;
            let time=fields[3].parse::<u64>().map_err(|_|invalid("hunting time"))?;
            let c=records.get_mut(&id).filter(|c|!c.deleted&&c.progression.alive()).ok_or_else(||invalid("hunting owner unavailable"))?;
            let result=c.quests.hunting.converse_version(entity,&c.progression,time,match fields[0] {"qh1"=>1,"qh2"=>2,_=>3})?;
            if result.book==c.quests.hunting {return Err(invalid("hunting event did not advance"));}
            if result.xp>0 {c.progression=c.progression.apply(&crate::progression::Command::AwardXp(result.xp)).map_err(io::Error::other)?;}
            c.quests.hunting=result.book;continue;
        }
        if fields.len()==5 && fields[0]=="q8" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("delivery owner"))?;
            let npc=fields[2].parse::<u32>().map_err(|_|invalid("delivery NPC"))?;
            let time=fields[3].parse::<u64>().map_err(|_|invalid("delivery time"))?;
            let first=fields[4].parse::<u64>().map_err(|_|invalid("delivery item"))?;
            let expected=records.values().map(|c|c.inventory.high_water()).max().unwrap_or(crate::inventory::ITEM_NAMESPACE).checked_add(1).ok_or_else(||invalid("Item identity exhausted"))?;
            if first!=expected{return Err(invalid("delivery item sequence"));}
            let c=records.get_mut(&id).filter(|c|!c.deleted&&c.progression.alive()).ok_or_else(||invalid("delivery owner unavailable"))?;
            let (items,changes,book)=c.quests.recovered.delivery(&c.inventory,npc,c.progression.level().number,time,first)?;
            if !items.is_empty()||!changes.is_empty(){(c.inventory,c.progression)=crate::player_transaction::candidate(&c.inventory,&c.progression,&items,&changes,Some(first))?;}
            c.quests.recovered=book;continue;
        }
        if fields.len()==5 && matches!(fields[0],"q6"|"qk1"|"qk2") {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("recovered quest owner"))?;
            let entity=u32::from_str_radix(fields[2],16).map_err(|_|invalid("recovered quest entity"))?;
            let time=fields[3].parse::<u64>().map_err(|_|invalid("recovered quest time"))?;
            if entity>>24!=0x47||time==0 {return Err(invalid("recovered quest event"));}
            let changes=crate::player_transaction::decode_changes(fields[4])?;
            if changes.is_empty(){return Err(invalid("empty recovered kill payout"));}
            let c=records.get_mut(&id).filter(|c|!c.deleted&&c.progression.alive()).ok_or_else(||invalid("recovered kill owner"))?;
            let (kill_quests,kill_xp)=kill_emotes(c,entity,time);
            let (book,xp)=c.quests.recovered.kill(entity,c.progression.level().number,time);
            let mut state=c.progression.clone();
            for change in changes.iter().chain(&kill_xp) {state=state.apply(change).map_err(io::Error::other)?;}
            if xp>0 {state=state.apply(&crate::progression::Command::AwardXp(xp)).map_err(io::Error::other)?;}
            c.quests=c.quests.record_kill(entity);c.quests.recovered=book;
            if fields[0]!="q6" {c.quests.hunting=c.quests.hunting.kill_version(entity,time,if fields[0]=="qk1" {1}else{2});}
            for command in &kill_quests {c.quests=c.quests.apply(command).map_err(io::Error::other)?;}
            c.progression=state;continue;
        }
        if fields.len()==5 && fields[0]=="q7" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("quest use owner"))?;
            let item=fields[2].parse::<u64>().map_err(|_|invalid("quest use item"))?;
            let quest=fields[3].parse::<u32>().map_err(|_|invalid("quest use id"))?;
            let time=fields[4].parse::<u64>().map_err(|_|invalid("quest use time"))?;
            let c=records.get_mut(&id).filter(|c|!c.deleted&&c.progression.alive()).ok_or_else(||invalid("quest use owner"))?;
            let owned=c.inventory.items().find(|i|i.id==item&&matches!(i.location,crate::inventory::Location::Bag(_)))
                .ok_or_else(||invalid("quest starter not owned"))?;
            if crate::recovered_quests::use_profile(owned.definition).is_none_or(|q|q.id!=quest){return Err(invalid("quest starter mismatch"));}
            let book=c.quests.recovered.accept(quest,c.progression.level().number,time)?;
            let bag=c.inventory.apply(&crate::inventory::Command::ConsumeDense{item}).map_err(io::Error::other)?;
            c.inventory=bag;c.quests.recovered=book;continue;
        }
        if fields.len() == 4 && fields[0] == "q2" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid quest reward owner"))?;
            let command = crate::quests::Command::decode(&format!("k,{}", fields[2]))
                .map_err(io::Error::other)?;
            let changes = crate::player_transaction::decode_changes(fields[3])?;
            if changes.is_empty() {
                return Err(invalid("quest kill payout is empty"));
            }
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted && c.progression.alive())
                .ok_or_else(|| invalid("unavailable quest kill owner"))?;
            let quests = character.quests.apply(&command).map_err(io::Error::other)?;
            if quests == character.quests {
                return Err(invalid("quest kill did not advance an objective"));
            }
            for change in changes {
                character.progression = character
                    .progression
                    .apply(&change)
                    .map_err(io::Error::other)?;
            }
            character.quests = quests;
            continue;
        }
        if fields.len() == 3 && fields[0] == "g3" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid progression checkpoint character"))?;
            let character = records
                .get_mut(&id)
                .filter(|character| !character.deleted)
                .ok_or_else(|| invalid("unavailable progression checkpoint character"))?;
            character.progression = compaction::progression(fields[2], &character.progression)?;
            if compacted.is_some() {
                run.record(id, &character.progression, line);
            }
            continue;
        }
        if fields.len() == 2 && fields[0] == "k4" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid training attack character"))?;
            let character = records
                .get_mut(&id)
                .filter(|c| {
                    !c.deleted
                        && c.training_kit
                        && c.skills.get(crate::skills::TRAINING_ATTACK).is_none()
                })
                .ok_or_else(|| invalid("unavailable training attack character"))?;
            character.skills = character
                .skills
                .with_training_attack(
                    &character.progression,
                    character
                        .creation
                        .as_ref()
                        .map_or(1, |request| request.race),
                )
                .map_err(|_| invalid("invalid training attack grant"))?;
            continue;
        }
        if fields.len() == 2 && fields[0] == "k3" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid recall grant character"))?;
            let character = records
                .get_mut(&id)
                .filter(|character| {
                    !character.deleted
                        && character.binding.is_some()
                        && character
                            .skills
                            .get(crate::skills::LIFESTONE_RECALL)
                            .is_none()
                })
                .ok_or_else(|| invalid("unavailable recall grant character"))?;
            character.skills = character
                .skills
                .with_lifestone_recall(
                    &character.progression,
                    character
                        .creation
                        .as_ref()
                        .map_or(1, |request| request.race),
                )
                .map_err(|_| invalid("invalid recall grant"))?;
            continue;
        }
        if fields.len() == 4 && fields[0] == "k2" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid advancement character"))?;
            let skill = u32::from_str_radix(fields[2], 16)
                .map_err(|_| invalid("invalid advancement skill"))?;
            let paid = fields[3]
                .parse::<u64>()
                .map_err(|_| invalid("invalid advancement payment"))?;
            let character = records
                .get_mut(&id)
                .filter(|character| !character.deleted)
                .ok_or_else(|| invalid("unavailable advancement character"))?;
            let race = character
                .creation
                .as_ref()
                .map_or(1, |request| request.race);
            let (skills, progression) = character
                .skills
                .advance(skill, &character.progression, race)
                .map_err(|_| invalid("invalid skill advancement transition"))?;
            if character
                .progression
                .available_xp
                .checked_sub(progression.available_xp)
                != Some(paid)
                || paid == 0
            {
                return Err(invalid("advancement payment differs from immutable curve"));
            }
            character.skills = skills;
            character.progression = progression;
            continue;
        }
        if fields.len()==4 && fields[0]=="s1" {
            // Rescue the Shelter Hold before it became data: the legacy command still moves the
            // stage, and its rewards are the matching emote set's (`legacy_shelter_set`).
            let id=fields[1].parse::<u64>().map_err(|_|invalid("Shelter owner"))?;
            let command=crate::quests::Command::decode(fields[2]).map_err(io::Error::other)?;
            let first=fields[3].parse::<u64>().map_err(|_|invalid("Shelter item identity"))?;
            let expected=records.values().map(|c|c.inventory.high_water()).max().unwrap_or(crate::inventory::ITEM_NAMESPACE).checked_add(1).ok_or_else(||invalid("Item identity exhausted"))?;
            if first!=expected {return Err(invalid("Shelter item sequence"));}
            let c=records.get_mut(&id).filter(|c|!c.deleted && c.progression.alive()).ok_or_else(||invalid("Shelter owner unavailable"))?;
            let set=legacy_shelter_set(command).ok_or_else(||invalid("Unknown Shelter exchange"))?;
            let o=crate::quest_emotes::installed()?.perform_set(set,&c.quests,&c.inventory,&c.progression,first,&crate::shelter::item)?;
            (c.quests,c.inventory,c.progression)=apply_emote(c,first,&[command],&o.items,&o.changes)?;
            let relocation=match command {
                crate::quests::Command::ShelterGlyph=>Some(crate::shelter::VAULT),
                crate::quests::Command::ShelterMenhir(block)=>Some(Placement{cell:u32::from(block)<<16|0x014d,coordinates:[1.0,1.0,1.19],heading:0}),
                _=>None,
            };
            if let Some(placement)=relocation {relocations.insert(id,(revision,placement));}
            continue;
        }
        if fields.len()==3 && fields[0]=="qm1" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("hunt migration owner"))?;
            let c=records.get_mut(&id).ok_or_else(||invalid("hunt migration owner unavailable"))?;
            for text in fields[2].split(';') {
                let command=crate::quests::Command::decode(text).map_err(io::Error::other)?;
                c.quests=c.quests.apply(&command).map_err(io::Error::other)?;
            }
            continue;
        }
        if fields.len()==7 && fields[0]=="e1" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("quest exchange owner"))?;
            let first=fields[2].parse::<u64>().map_err(|_|invalid("quest exchange item identity"))?;
            let expected=records.values().map(|c|c.inventory.high_water()).max().unwrap_or(crate::inventory::ITEM_NAMESPACE).checked_add(1).ok_or_else(||invalid("Item identity exhausted"))?;
            if first!=expected {return Err(invalid("quest exchange item sequence"));}
            let quests=fields[3].split(';').filter(|q|!q.is_empty()).map(crate::quests::Command::decode).collect::<Result<Vec<_>,_>>().map_err(io::Error::other)?;
            let items=crate::player_transaction::decode_items(fields[4])?;
            let changes=crate::player_transaction::decode_changes(fields[5])?;
            let c=records.get_mut(&id).filter(|c|!c.deleted && c.progression.alive()).ok_or_else(||invalid("quest exchange owner unavailable"))?;
            (c.quests,c.inventory,c.progression)=apply_emote(c,first,&quests,&items,&changes)?;
            if fields[6]!="-" {relocations.insert(id,(revision,decode_placement(fields[6])?));}
            continue;
        }
        if fields.len() == 4 && fields[0] == "p3" {
            let id = fields[1].parse::<u64>().map_err(|_| invalid("Invalid travel lesson owner"))?;
            let portal = u64::from_str_radix(fields[2], 16).map_err(|_| invalid("Invalid training portal identity"))?;
            if !crate::quests::is_training_portal(portal) { return Err(invalid("Travel lesson portal is not enabled")); }
            let placement = decode_placement(fields[3])?;
            let character = records.get_mut(&id).filter(|c| !c.deleted && c.progression.alive()).ok_or_else(|| invalid("Unavailable travel lesson owner"))?;
            character.quests = character.quests.apply(&crate::quests::Command::Travelled).map_err(io::Error::other)?;
            relocations.insert(id, (revision, placement));
            continue;
        }
        if fields.len() == 3 && matches!(fields[0], "l1" | "l2" | "l3" | "p1" | "p2" | "r2") {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid lifestone character"))?;
            let placement = decode_placement(fields[2])?;
            let character = records
                .get_mut(&id)
                .filter(|character| !character.deleted)
                .ok_or_else(|| invalid("unavailable lifestone character"))?;
            if matches!(fields[0], "l1" | "l2" | "l3") {
                if !character.progression.alive() {
                    return Err(invalid("dead character binding"));
                }
                if fields[0] == "l3" { character.quests = character.quests.apply(&crate::quests::Command::BoundLifestone).map_err(io::Error::other)?; }
                character.binding = Some(placement);
                if matches!(fields[0], "l2" | "l3") {
                    character.skills = character
                        .skills
                        .with_lifestone_recall(
                            &character.progression,
                            character
                                .creation
                                .as_ref()
                                .map_or(1, |request| request.race),
                        )
                        .map_err(|_| invalid("invalid bound recall skill"))?;
                }
            } else {
                if fields[0] != "p2" && character.binding != Some(placement) {
                    return Err(invalid("relocation differs from saved binding"));
                }
                if fields[0] == "r2" {
                    character.progression = character
                        .progression
                        .apply(&crate::progression::Command::Resurrect)
                        .map_err(|_| invalid("invalid bound resurrection"))?;
                } else if !character.progression.alive() {
                    return Err(invalid("dead character recall"));
                }
                relocations.insert(id, (revision, placement));
            }
            continue;
        }
        if fields.len() == 2 && fields[0] == "k5" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid starter skill character"))?;
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("unavailable starter skill character"))?;
            let request = character
                .creation
                .as_ref()
                .ok_or_else(|| invalid("starter skills without a creation profile"))?;
            character.skills = character
                .skills
                .with_starter_skills(&character.progression, request.race, request.sex)
                .map_err(|_| invalid("invalid starter skill grant"))?;
            continue;
        }
        if fields.len() == 3 && fields[0] == "t4" {
            let id = fields[1].parse::<u64>().map_err(|_| invalid("invalid starter owner"))?;
            let first = fields[2].parse::<u64>().map_err(|_| invalid("invalid starter identity"))?;
            let high = records.values().map(|character| character.inventory.high_water()).max()
                .unwrap_or(crate::inventory::ITEM_NAMESPACE);
            if high.checked_add(1) != Some(first) { return Err(invalid("nonsequential starter identity")); }
            let character = records.get_mut(&id)
                .filter(|character| !character.deleted && !character.starter_gear)
                .ok_or_else(|| invalid("invalid repeated starter grant"))?;
            let request = character.creation.as_ref().ok_or_else(|| invalid("starter grant without creation"))?;
            character.inventory = crate::starter_gear::candidate(request, &character.inventory, first)?;
            character.starter_gear = true;
            character.training_kit = true;
            continue;
        }
        if fields.len() == 3 && fields[0] == "t3" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("training supplies owner"))?;
            let first = fields[2]
                .parse::<u64>()
                .map_err(|_| invalid("training supplies identity"))?;
            let high = records
                .values()
                .map(|c| c.inventory.high_water())
                .max()
                .unwrap_or(crate::inventory::ITEM_NAMESPACE);
            if high.checked_add(1) != Some(first) {
                return Err(invalid("nonsequential training supplies identity"));
            }
            let character = records
                .get_mut(&id)
                .filter(|c| {
                    !c.deleted && c.training_kit && !c.training_supplies && c.progression.alive()
                })
                .ok_or_else(|| invalid("invalid repeated training supplies grant"))?;
            character.inventory =
                crate::training_kit::supply_inventory(&character.inventory, first)
                    .map_err(io::Error::other)?;
            character.training_supplies = true;
            continue;
        }
        if fields.len() == 3 && matches!(fields[0], "t1" | "t2") {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid training kit owner"))?;
            let item = fields[2]
                .parse::<u64>()
                .map_err(|_| invalid("invalid training kit item"))?;
            let high = records
                .values()
                .map(|character| character.inventory.high_water())
                .max()
                .unwrap_or(crate::inventory::ITEM_NAMESPACE);
            if high.checked_add(1) != Some(item) {
                return Err(invalid("nonsequential training kit identity"));
            }
            let character = records
                .get_mut(&id)
                .filter(|character| !character.deleted && !character.training_kit)
                .ok_or_else(|| invalid("invalid repeated training kit grant"))?;
            if !character.creation.as_ref().is_some_and(|request| {
                request.race == 1 && request.entity_did == crate::bootstrap::PLAYER_ENTITY
            }) {
                return Err(invalid("unsupported training kit character profile"));
            }
            let slot = character
                .inventory
                .free_slot()
                .ok_or_else(|| invalid("training kit bag full"))?;
            character.inventory = character
                .inventory
                .apply(&crate::inventory::Command::Grant(
                    crate::training_kit::item(item, slot),
                ))
                .map_err(|_| invalid("invalid training kit inventory transition"))?;
            if fields[0] == "t2" {
                character.skills = character
                    .skills
                    .with_training_attack(
                        &character.progression,
                        character.creation.as_ref().unwrap().race,
                    )
                    .map_err(|_| invalid("invalid training kit skill transition"))?;
            }
            character.training_kit = true;
            continue;
        }
        if fields.len() == 3 && fields[0] == "k1" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid skill owner"))?;
            let command = crate::skills::Command::decode(fields[2])
                .map_err(|_| invalid("invalid skill mutation"))?;
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("skill owner unavailable"))?;
            let race = character
                .creation
                .as_ref()
                .map_or(1, |request| request.race);
            character.skills = character
                .skills
                .apply(command, &character.progression, race)
                .map_err(|_| invalid("invalid saved skill transition"))?;
            continue;
        }
        if fields.len()==2 && fields[0]=="ku1" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("untraining owner"))?;
            let c=records.get_mut(&id).filter(|c|!c.deleted).ok_or_else(||invalid("untraining character unavailable"))?;
            let (skills,progression)=c.skills.debit_untraining(&c.progression).map_err(io::Error::other)?.ok_or_else(||invalid("untraining without earned XP"))?;
            c.skills=skills;c.progression=progression;continue;
        }
        if fields.len() == 3 && fields[0] == "g2" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid progression batch owner"))?;
            if fields[2].len() > 256 {
                return Err(invalid("progression batch limit"));
            }
            let commands = fields[2].split(';').collect::<Vec<_>>();
            if commands.is_empty() || commands.len() > 8 {
                return Err(invalid("progression batch size"));
            }
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("progression batch owner unavailable"))?;
            let mut next = character.progression.clone();
            for text in commands {
                let command = crate::progression::Command::decode(text)
                    .map_err(|_| invalid("invalid progression batch event"))?;
                next = next
                    .apply(&command)
                    .map_err(|_| invalid("invalid progression batch transition"))?;
            }
            character.progression = next;
            if compacted.is_some() {
                run.record(id, &character.progression, line);
            }
            continue;
        }
        if fields.len() == 3 && fields[0] == "g1" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid progression character ID"))?;
            let command = crate::progression::Command::decode(fields[2])
                .map_err(|_| invalid("invalid progression event"))?;
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("progression event references unavailable character"))?;
            character.progression = character
                .progression
                .apply(&command)
                .map_err(|_| invalid("invalid progression transition"))?;
            if compacted.is_some() {
                run.record(id, &character.progression, line);
            }
            continue;
        }
        // `a2`: an encounter's item payout before its time was recorded (its quest emotes replay
        // at time 0: no time limit had run out); `ek1` adds the time.
        if (fields.len() == 5 && fields[0] == "a2") || (fields.len() == 6 && fields[0] == "ek1") {
            let time = if fields[0] == "ek1" { fields[5].parse::<u64>().map_err(|_| invalid("invalid encounter time"))? } else { 0 };
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid encounter transaction owner"))?;
            if fields[2].len() != 8 {
                return Err(invalid("invalid encounter entity encoding"));
            }
            let entity = u32::from_str_radix(fields[2], 16)
                .map_err(|_| invalid("invalid encounter entity"))?;
            if entity >> 24 != 0x47 {
                return Err(invalid("invalid encounter entity type"));
            }
            let items = crate::player_transaction::decode_items(fields[3])?;
            let changes = crate::player_transaction::decode_changes(fields[4])?;
            if items.is_empty() || changes.is_empty() {
                return Err(invalid("empty encounter item payout"));
            }
            let first = if items.iter().any(|command| command.allocated_id().is_some()) {
                Some(
                    records
                        .values()
                        .map(|c| c.inventory.high_water())
                        .max()
                        .unwrap_or(crate::inventory::ITEM_NAMESPACE)
                        .checked_add(1)
                        .ok_or_else(|| invalid("encounter item identity exhausted"))?,
                )
            } else {
                None
            };
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted && c.progression.alive())
                .ok_or_else(|| invalid("unavailable encounter payout owner"))?;
            let (kill_quests,kill_xp)=kill_emotes(character,entity,time);
            let (inventory, progression) = crate::player_transaction::candidate(
                &character.inventory,
                &character.progression,
                &items,
                &changes,
                first,
            )?;
            character.quests = kill_quests.iter().try_fold(character.quests.record_kill(entity),|b,c|b.apply(c)).map_err(io::Error::other)?;
            character.inventory = inventory;
            character.progression = kill_xp.iter().try_fold(progression,|p,c|p.apply(c)).map_err(io::Error::other)?;
            continue;
        }
        if fields.len()==5 && fields[0]=="m1" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("Invalid lesson owner"))?;
            let action=crate::arwic_mines::Action::decode(fields[2])?;
            let time=fields[3].parse::<u64>().map_err(|_|invalid("Invalid lesson time"))?;
            let first=fields[4].parse::<u64>().map_err(|_|invalid("Invalid lesson identity"))?;
            let expected=records.values().map(|c|c.inventory.high_water()).max().unwrap_or(crate::inventory::ITEM_NAMESPACE).checked_add(1).ok_or_else(||invalid("Lesson identity overflow"))?;
            if first!=expected {return Err(invalid("Lesson item identity mismatch"));}
            let c=records.get_mut(&id).filter(|c|!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Lesson owner unavailable"))?;
            let plan=crate::arwic_mines::plan(&c.quests,&c.inventory,action,first,time)?;
            let (bag,state)=if plan.items.is_empty()&&plan.rewards.is_empty(){(c.inventory.clone(),c.progression.clone())}else{crate::player_transaction::candidate(&c.inventory,&c.progression,&plan.items,&plan.rewards,Some(first))?};
            c.inventory=bag;c.progression=state;c.quests=plan.quests;continue;
        }
        if fields.len()==5&&fields[0]=="c1" {
            let id=fields[1].parse::<u64>().map_err(|_|invalid("Invalid crafting owner"))?;
            let items=crate::player_transaction::decode_items(fields[2])?;
            let changes=crate::player_transaction::decode_changes(fields[3])?;
            let change:crate::professions::Change=serde_json::from_str(fields[4]).map_err(io::Error::other)?;
            let first=if items.iter().any(|i|i.allocated_id().is_some()){Some(records.values().map(|c|c.inventory.high_water()).max().unwrap_or(crate::inventory::ITEM_NAMESPACE).checked_add(1).ok_or_else(||invalid("Craft identity overflow"))?)}else{None};
            let c=records.get_mut(&id).filter(|c|!c.deleted&&c.progression.alive()).ok_or_else(||invalid("Crafting owner unavailable"))?;
            let book=c.professions.apply(&change)?;
            let (bag,state)=if items.is_empty()&&changes.is_empty(){(c.inventory.clone(),c.progression.clone())}else{crate::player_transaction::candidate(&c.inventory,&c.progression,&items,&changes,first)?};
            c.inventory=bag;c.progression=state;c.professions=book;continue;
        }
        if fields.len() == 4 && matches!(fields[0], "a1" | "a2" | "a3") {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid transaction owner"))?;
            let items = crate::player_transaction::decode_items(fields[2])?;
            let changes = crate::player_transaction::decode_changes(fields[3])?;
            let first = if items.iter().any(|command| command.allocated_id().is_some()) {
                Some(
                    records
                        .values()
                        .map(|c| c.inventory.high_water())
                        .max()
                        .unwrap_or(crate::inventory::ITEM_NAMESPACE)
                        .checked_add(1)
                        .ok_or_else(|| invalid("transaction item identity exhausted"))?,
                )
            } else {
                None
            };
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("unavailable transaction owner"))?;
            let (inventory, progression) = crate::player_transaction::candidate(
                &character.inventory,
                &character.progression,
                &items,
                &changes,
                first,
            )?;
            if fields[0] == "a2" {
                if !character.progression.alive() { return Err(invalid("Dead supply exchange owner")); }
                character.quests = crate::quests::validate_supplies_exchange(&character.quests, &character.inventory, &items, &changes, first.ok_or_else(|| invalid("Missing supply output identity"))?)?;
            }
            if fields[0] == "a3" {
                if !character.progression.alive() { return Err(invalid("Dead practice craft owner")); }
                character.quests = crate::quests::validate_practice_craft(&character.quests, &character.inventory, &items, &changes, first.ok_or_else(|| invalid("Missing craft output identity"))?)?;
            }
            character.inventory = inventory;
            character.progression = progression;
            continue;
        }
        if fields.len() == 4 && matches!(fields[0], "wd1" | "wp1") {
            drops::replay(&fields,&mut records,&mut drops)?;
            continue;
        }
        if fields.len() == 3 && fields[0] == "i1" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid inventory character"))?;
            let command = crate::inventory::Command::decode(fields[2])
                .map_err(|_| invalid("invalid inventory event"))?;
            if let Some(allocated) = command.allocated_id() {
                let high = records
                    .values()
                    .map(|c| c.inventory.high_water())
                    .max()
                    .unwrap_or(crate::inventory::ITEM_NAMESPACE);
                if high.checked_add(1) != Some(allocated) {
                    return Err(invalid("invalid global item identity progression"));
                }
            }
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("inventory event references unavailable character"))?;
            character.inventory = character
                .inventory
                .apply(&command)
                .map_err(|_| invalid("invalid inventory transition"))?;
            continue;
        }
        if fields.len() == 3 && matches!(fields[0], "u1" | "h1" | "o1") {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid UI character ID"))?;
            let character = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("UI event references unavailable character"))?;
            let hex = fields[2].as_bytes();
            if hex.len() > crate::preferences::MAX_BYTES * 2 || hex.len() % 2 != 0 {
                return Err(invalid("invalid UI settings encoding size"));
            }
            let mut bytes = Vec::with_capacity(hex.len() / 2);
            for pair in hex.chunks_exact(2) {
                let high = (pair[0] as char)
                    .to_digit(16)
                    .ok_or_else(|| invalid("invalid UI hex"))?;
                let low = (pair[1] as char)
                    .to_digit(16)
                    .ok_or_else(|| invalid("invalid UI hex"))?;
                bytes.push(((high << 4) | low) as u8);
            }
            if fields[0] == "u1" {
                character.ui = Some(
                    crate::preferences::UiSettings::decode(&bytes)
                        .map_err(|_| invalid("invalid saved UI settings"))?,
                );
            } else if fields[0] == "o1" {
                character.gameplay_options = Some(
                    crate::preferences::GameplayOptions::decode(&bytes)
                        .map_err(|_| invalid("invalid saved gameplay options"))?,
                );
            } else {
                let update = crate::preferences::HotbarUpdate::decode(&bytes)
                    .map_err(|_| invalid("invalid saved hotbar update"))?;
                character.hotbar = character
                    .hotbar
                    .apply(&update)
                    .map_err(|_| invalid("saved hotbar exceeds limit"))?;
            }
            continue;
        }
        if fields.len() == 3 && fields[0] == "e1" {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid instance character ID"))?;
            let instance = fields[2]
                .parse::<u16>()
                .map_err(|_| invalid("invalid instance stamp"))?;
            let c = records
                .get_mut(&id)
                .filter(|c| !c.deleted)
                .ok_or_else(|| invalid("instance event references unavailable character"))?;
            if c.instance.checked_add(1) != Some(instance) {
                return Err(invalid("invalid instance stamp progression"));
            }
            c.instance = instance;
            continue;
        }
        if fields.len() == 2 && matches!(fields[0], "d1" | "r1") {
            let id = fields[1]
                .parse::<u64>()
                .map_err(|_| invalid("invalid character lifecycle ID"))?;
            let c = records
                .get(&id)
                .ok_or_else(|| invalid("lifecycle event references unknown character"))?;
            let deleting = fields[0] == "d1";
            if c.deleted == deleting {
                return Err(invalid("invalid character lifecycle transition"));
            }
            if !deleting
                && records
                    .values()
                    .filter(|other| other.account == c.account && !other.deleted)
                    .count()
                    >= 5
            {
                return Err(invalid("restoration exceeds local character limit"));
            }
            records.get_mut(&id).unwrap().deleted = deleting;
            continue;
        }
        if !((fields.len() == 4 && fields[0] == "v1") || (fields.len() == 5 && fields[0] == "v2"))
            || !valid_name(fields[2])
            || !valid_name(fields[3])
        {
            return Err(invalid("invalid journal record"));
        }
        let id = fields[1]
            .parse::<u64>()
            .map_err(|_| invalid("invalid character ID"))?;
        if id == 0 || records.contains_key(&id) {
            return Err(invalid("duplicate or zero character ID"));
        }
        if records
            .values()
            .any(|c: &Character| c.name.eq_ignore_ascii_case(fields[3]))
        {
            return Err(invalid("duplicate character name in journal"));
        }
        if records
            .values()
            .filter(|c| c.account == fields[2] && !c.deleted)
            .count()
            >= 5
        {
            return Err(invalid("account exceeds local character limit"));
        }
        let creation = if fields[0] == "v2" {
            Some(decode_creation(fields[3], fields[4])?)
        } else {
            None
        };
        records.insert(
            id,
            Character {
                id,
                account: fields[2].into(),
                name: fields[3].into(),
                creation,
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
            },
        );
    }
    if let Some(output) = compacted {
        run.flush(output);
    }
    Ok(JournalState {
        characters: records,
        drops,
        relocations,
        revision,
    })
}

// Journal v2 uses canonical little-endian words encoded as hex, independent of network
// string padding. Physical tuple order and float bits survive without normalization.
fn encode_creation(request: &CharacterCreationRequest) -> io::Result<String> {
    if request.physical_values.len() > MAX_PHYSICAL_VALUES {
        return Err(invalid("too many physical values"));
    }
    let mut keys = std::collections::BTreeSet::new();
    if request
        .physical_values
        .iter()
        .any(|(key, bits)| !keys.insert(*key) || !f32::from_bits(*bits).is_finite())
    {
        return Err(invalid("invalid or duplicate physical value"));
    }
    let mut bytes = Vec::with_capacity(28 + request.physical_values.len() * 8);
    for word in [
        request.entity_did,
        request.start_area,
        request.race,
        request.sex,
        request.class,
        request.implement,
        request.physical_values.len() as u32,
    ] {
        bytes.extend(word.to_le_bytes());
    }
    for (key, bits) in &request.physical_values {
        bytes.extend(key.to_le_bytes());
        bytes.extend(bits.to_le_bytes());
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 15) as usize] as char);
    }
    Ok(out)
}
fn decode_creation(name: &str, hex: &str) -> io::Result<CharacterCreationRequest> {
    if hex.len() < 56 || hex.len() > (28 + MAX_PHYSICAL_VALUES * 8) * 2 || hex.len() % 8 != 0 {
        return Err(invalid("invalid native character data length"));
    }
    fn nibble(b: u8) -> io::Result<u8> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err(invalid("invalid native character hex")),
        }
    }
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|p| Ok((nibble(p[0])? << 4) | nibble(p[1])?))
        .collect::<io::Result<_>>()?;
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect();
    let count = words[6] as usize;
    if count > MAX_PHYSICAL_VALUES || words.len() != 7 + count * 2 {
        return Err(invalid("invalid native character physical count"));
    }
    let request = CharacterCreationRequest {
        name: name.into(),
        entity_did: words[0],
        start_area: words[1],
        race: words[2],
        sex: words[3],
        class: words[4],
        implement: words[5],
        physical_values: words[7..].chunks_exact(2).map(|w| (w[0], w[1])).collect(),
    };
    encode_creation(&request)?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invested_skill_untraining_refunds_earned_xp_and_survives_restart() {
        use crate::{skills::Command as S,progression::Command as P};
        let dir=std::env::temp_dir().join(format!("ac2-untrain-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
            s.compact_journal().unwrap();
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
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn quest_emote_exchanges_replay_as_recorded() {
        use crate::{inventory::Command as I,quest_emotes::Trigger as T,shelter};
        let t=crate::quest_emotes::shipped();
        let dir=std::env::temp_dir().join(format!("ac2-emote-{}",std::process::id()));
        let _=fs::remove_dir_all(&dir);
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
        let _=fs::remove_dir_all(&dir);
    }
    #[test]
    fn a_quest_effect_record_replays() {
        let dir=std::env::temp_dir().join(format!("ac2-quest-effect-{}",std::process::id()));
        let _=fs::remove_dir_all(&dir);
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
        let _=fs::remove_dir_all(&dir);
    }
    #[test]
    fn retired_hunts_migrate_once_and_the_migration_replays() {
        let dir=std::env::temp_dir().join(format!("ac2-hunt-migrate-{}",std::process::id()));
        let _=fs::remove_dir_all(&dir);
        let marsh=0x4000fc20;
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Hunt Mover")).unwrap();
            // State a version 2 hunt record left (the journal replays it before the migration).
            s.characters.get_mut(&c.id).unwrap().quests.hunting.0.insert(marsh,crate::hunting_quests::State{count:3,started:500,completed:0,rewarded_xp:0});
            s.characters.get_mut(&c.id).unwrap().quests.hunting.0.insert(0x4000fc23,crate::hunting_quests::State{count:1,started:600,completed:0,rewarded_xp:0});
            assert_eq!(s.migrate_retired_hunts().unwrap(),1);
            assert_eq!(s.migrate_retired_hunts().unwrap(),0,"once");
            saved=s.characters[&c.id].clone();
            assert_eq!((saved.quests.stage(marsh),saved.quests.timer(marsh).started),(4,500));
            assert!(!saved.quests.hunting.0.contains_key(&marsh) && saved.quests.hunting.0.contains_key(&0x4000fc23));
        }
        let text=fs::read_to_string(dir.join("characters.journal")).unwrap();
        assert!(text.lines().any(|l|l.starts_with("qm1\t") && l.contains("retire,4000fc20")),"{text}");
        // The hunting entries were set in memory only, so the replay rebuilds the moved quest alone.
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&saved.id].quests.stage(marsh),4);
        assert_eq!(s.characters[&saved.id].quests.timer(marsh),saved.quests.timer(marsh));
        let _=fs::remove_dir_all(&dir);
    }
    #[test]
    fn an_encounter_payout_records_its_time_and_an_older_record_replays_without_one() {
        let dir=std::env::temp_dir().join(format!("ac2-encounter-time-{}",std::process::id()));
        let _=fs::remove_dir_all(&dir);
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Kill Time")).unwrap();
            let id=s.next_item_id().unwrap();
            let items=[crate::inventory::Command::Grant(crate::item_services::potion(id,0))];
            s.encounter_transaction("local",c.id,0x4700158b,&items,&[crate::progression::Command::AwardXp(5)]).unwrap();
            saved=s.characters[&c.id].clone();
        }
        let path=dir.join("characters.journal");
        let text=fs::read_to_string(&path).unwrap();
        let line=text.lines().find(|l|l.starts_with("ek1\t")).expect("an ek1 record").to_owned();
        let f:Vec<&str>=line.split('\t').collect();
        assert_eq!(f.len(),7,"ek1, owner, entity, items, changes, time, hash");
        assert!(f[5].parse::<u64>().unwrap()>1_700_000_000);
        {let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&saved.id],saved);}
        // The same payout as an `a2` record (no time) still replays to the same character.
        let old=f[..5].join("\t").replacen("ek1","a2",1);
        let old=format!("{old}\t{:08x}",hash(old.as_bytes()));
        fs::write(&path,text.replace(&line,&old)).unwrap();
        {let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&saved.id],saved);}
        let _=fs::remove_dir_all(&dir);
    }
    #[test]
    fn a_full_bag_refuses_the_scout_without_taking_the_tool() {
        use crate::{inventory::{Command as I,BAG_SLOTS},quest_emotes::Trigger as T,shelter};
        let t=crate::quest_emotes::shipped();
        let dir=std::env::temp_dir().join(format!("ac2-emote-full-{}",std::process::id()));
        let _=fs::remove_dir_all(&dir);
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
        let _=fs::remove_dir_all(&dir);
    }
    #[test]
    fn deletion_restoration_keeps_identity_appearance_and_saved_placement() {
        let dir = std::env::temp_dir().join(format!("ac2-lifecycle-{}", std::process::id()));
        {
            let mut store = Store::open(&dir).unwrap();
            for i in 1..=5 {
                store
                    .create_native("local", &request(&format!("Hero {i}")))
                    .unwrap();
            }
            store.set_placement(5, Placement::HUMAN_START).unwrap();
            store.flush_positions().unwrap();
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
        for line in ["d1\t1", "r1\t1"] {
            assert!(decode_journal(&record(line)).is_err());
        }
        let mut bad = record("v1\t1\tlocal\tHero");
        bad.extend(record("r1\t1"));
        assert!(decode_journal(&bad).is_err());
        for file in ["characters.journal", "positions.snapshot"] {
            fs::remove_file(dir.join(file)).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn placement_snapshots_replace_durably_and_reject_corruption() {
        let dir = std::env::temp_dir().join(format!("ac2-positions-{}", std::process::id()));
        let mut final_position = Placement::HUMAN_START;
        {
            let mut store = Store::open(&dir).unwrap();
            store.create("local", "Explorer").unwrap();
            assert!(store.set_placement(2, final_position).is_err());
            store.set_placement(1, final_position).unwrap();
            store.flush_positions().unwrap();
            final_position.coordinates[0] += 4.0;
            store.set_placement(1, final_position).unwrap();
            store.flush_positions().unwrap();
        }
        {
            let store = Store::open(&dir).unwrap();
            assert_eq!(store.placement(1), Some(final_position));
            let bytes = fs::read(dir.join("positions.snapshot")).unwrap();
            for end in 0..bytes.len() {
                assert!(decode_positions(&bytes[..end], &store.characters).is_err());
            }
            let mut corrupt = bytes;
            corrupt[20] ^= 1;
            assert!(decode_positions(&corrupt, &store.characters).is_err());
            // A valid checksum is insufficient when a placement references no character.
            assert!(
                decode_positions(
                    &fs::read(dir.join("positions.snapshot")).unwrap(),
                    &BTreeMap::new()
                )
                .is_err()
            );
        }
        fs::remove_file(dir.join("characters.journal")).unwrap();
        fs::remove_file(dir.join("positions.snapshot")).unwrap();
        fs::remove_dir(dir).unwrap();
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
        let dir=std::env::temp_dir().join(format!("ac2-recovered-gameplay-{}",std::process::id()));
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
            let length=s.file.metadata().unwrap().len();
            assert!(s.recovered_elixir("other",1,ITEM_NAMESPACE+2).is_err());assert_eq!(s.file.metadata().unwrap().len(),length);
            s.recovered_elixir("local",1,ITEM_NAMESPACE+2).unwrap();
            let length=s.file.metadata().unwrap().len();
            assert!(s.recovered_elixir("local",1,ITEM_NAMESPACE+3).is_err());assert_eq!(s.file.metadata().unwrap().len(),length);
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
        std::fs::remove_file(dir.join("characters.journal")).unwrap();std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn recovered_delivery_consumes_owned_bag_and_replays_without_duplicate_xp() {
        use crate::{recovered_quests::{DEBT,KANTEN,ORVEN,BAG},progression::Command as P};
        let dir=std::env::temp_dir().join(format!("ac2-recovered-delivery-{}",std::process::id()));
        let xp;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Delivery Hero").unwrap();
            let length=s.file.metadata().unwrap().len();
            assert!(s.recovered_delivery("local",1,KANTEN,100).is_err());assert_eq!(s.file.metadata().unwrap().len(),length);
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
        std::fs::remove_file(dir.join("characters.journal")).unwrap();std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn exploration_conversations_replay_partial_and_completed_quests_exactly_once() {
        let dir=std::env::temp_dir().join(format!("ac2-exploration-{}-{}",std::process::id(),crate::recovered_quests::now()));
        let q=&crate::exploration_quests::catalog().quests[0];
        let referral=&crate::exploration_quests::catalog().referral;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Ringway Hero").unwrap();
            assert!(s.exploration_conversation("other",1,q.npc,100).is_err());
            assert!(s.exploration_conversation("local",1,q.objectives[0],100).is_err());
            s.exploration_conversation("local",1,q.npc,100).unwrap();
            s.exploration_conversation("local",1,q.objectives[0],101).unwrap();
            let length=s.file.metadata().unwrap().len();
            s.exploration_conversation("local",1,q.objectives[0],102).unwrap();
            assert_eq!(s.file.metadata().unwrap().len(),length,"duplicate touch must not append");
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
        std::fs::remove_file(dir.join("characters.journal")).unwrap();std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn hunting_quests_replay_acceptance_kills_return_and_exactly_one_reward() {
        use crate::progression::Command::AwardXp;
        let dir=std::env::temp_dir().join(format!("ac2-hunting-{}-{}",std::process::id(),crate::recovered_quests::now()));
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
            let length=s.file.metadata().unwrap().len();
            s.hunting_conversation("local",1,q.npc,time+2).unwrap();
            assert_eq!(s.file.metadata().unwrap().len(),length);
        }
        {
            let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1].progression.xp,initial+10+q.xp_cap);
            assert_eq!(s.characters[&1].quests.hunting.0[&q.id].completed,time+1);
        }
        std::fs::remove_file(dir.join("characters.journal")).unwrap();std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn hunting_v1_journal_replays_before_new_gregor_chain_and_fixed_rewards() {
        use crate::progression::Command::AwardXp;
        let dir=std::env::temp_dir().join(format!("ac2-hunting-versions-{}-{}",std::process::id(),crate::recovered_quests::now()));
        let q=crate::hunting_quests::profile(0x4000fc04).unwrap();
        let initial=113726634933;let repeat=120+q.repeat_seconds;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Version Hunter").unwrap();
            s.progression_command("local",1,AwardXp(initial)).unwrap();
            s.append_record(&format!("qh1\t1\t{}\t100",q.npc)).unwrap();
            for n in 0..10 {s.append_record(&format!("qk1\t1\t{:08x}\t{}\tx,1",q.targets[0],101+n)).unwrap();}
            s.append_record(&format!("qh1\t1\t{}\t120",q.npc)).unwrap();
            s.append_record(&format!("qh1\t1\t{}\t{repeat}",q.npc)).unwrap();
        }
        {
            let mut s=Store::open(&dir).unwrap();let c=&s.characters[&1];
            assert_eq!(c.quests.hunting.0.len(),1);
            assert_eq!(c.quests.hunting.0[&q.id].started,repeat);
            assert_eq!(c.progression.xp,initial+10+q.xp_cap);
            let fixed=crate::hunting_quests::profile(0x4000fc23).unwrap();
            let now=crate::recovered_quests::now();
            s.hunting_conversation("local",1,fixed.npc,now).unwrap();
            for _ in 0..10 {s.encounter_reward("local",1,fixed.targets[0],&[AwardXp(1)]).unwrap();}
            s.hunting_conversation("local",1,fixed.npc,now+1).unwrap();
            s.compact_journal().unwrap();
        }
        {
            let s=Store::open(&dir).unwrap();
            assert_eq!(s.characters[&1].progression.xp,initial+20+q.xp_cap+55000);
            assert_eq!(s.characters[&1].quests.hunting.0[&0x4000fc23].rewarded_xp,55000);
        }
        std::fs::remove_file(dir.join("characters.journal")).unwrap();std::fs::remove_dir(dir).unwrap();
    }
    fn record(data: &str) -> Vec<u8> {
        format!("{data}\t{:08x}\n", hash(data.as_bytes())).into_bytes()
    }
    #[test]
    fn arwic_lesson_replays_inventory_entitlement_and_reward_atomically() {
        use crate::arwic_mines::{Action,IRON,SWORD,GLYPH,GUIDE};
        let dir=std::env::temp_dir().join(format!("ac2-arwic-{}-{}",std::process::id(),crate::recovered_quests::now()));
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
            s.compact_journal().unwrap();
        }
        {
            let mut s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1].quests.arwic.phase,4);
            assert!(s.characters[&1].quests.arwic.learned);
            s.arwic_transaction("local",1,Action::Talk,104).unwrap();
            let length=s.file.metadata().unwrap().len();
            s.arwic_transaction("local",1,Action::Talk,105).unwrap();
            assert_eq!(s.file.metadata().unwrap().len(),length);s.compact_journal().unwrap();
        }
        {
            let s=Store::open(&dir).unwrap();let c=&s.characters[&1];
            assert_eq!(c.progression.xp,150);assert_eq!(c.quests.arwic.completed,104);
            assert!(c.quests.arwic.learned);
            for definition in [GLYPH,GUIDE] {assert_eq!(c.inventory.items().filter(|i|i.definition==definition).count(),1);}
            assert!(!c.inventory.items().any(|i|i.definition==SWORD));
        }
        std::fs::remove_file(dir.join("characters.journal")).unwrap();std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn avatar_lifetimes_survive_restart_and_reject_invalid_journal_transitions() {
        let dir = std::env::temp_dir().join(format!("ac2-instances-{}", std::process::id()));
        {
            let mut store = Store::open(&dir).unwrap();
            store.create("local", "Hero").unwrap();
            assert!(store.begin_instance("other", 1).is_err());
            assert_eq!(store.begin_instance("local", 1).unwrap(), 1);
            store.delete("local", 1).unwrap();
            assert!(store.begin_instance("local", 1).is_err());
            store.restore("local", 1).unwrap();
        }
        {
            let mut store = Store::open(&dir).unwrap();
            assert_eq!(store.begin_instance("local", 1).unwrap(), 2);
            let length = store.file.metadata().unwrap().len();
            store.characters.get_mut(&1).unwrap().instance = u16::MAX;
            assert!(store.begin_instance("local", 1).is_err());
            assert_eq!(store.file.metadata().unwrap().len(), length);
        }
        let prefix = record("v1\t1\tlocal\tHero");
        for suffix in ["e1\t1\t0", "e1\t1\t2", "e1\t2\t1", "e1\t1\t65536"] {
            let mut bytes = prefix.clone();
            bytes.extend(record(suffix));
            assert!(decode_journal(&bytes).is_err());
        }
        let mut bytes = prefix;
        bytes.extend(record("e1\t1\t1"));
        let valid = bytes.clone();
        bytes.extend(record("e1\t1\t1"));
        assert!(decode_journal(&bytes).is_err());
        bytes = valid;
        bytes.extend(record("d1\t1"));
        bytes.extend(record("e1\t1\t2"));
        assert!(decode_journal(&bytes).is_err());
        fs::remove_file(dir.join("characters.journal")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn mixed_versions_retain_exact_native_appearance_after_restart() {
        let dir = std::env::temp_dir().join(
            format!(
                "ac2-native-journal-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            )
            .replace(['(', ')'], ""),
        );
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
        fs::remove_file(dir.join("characters.journal")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn native_records_reject_torn_corrupt_and_semantically_malformed_data() {
        let req = request("Hero");
        let encoded = encode_creation(&req).unwrap();
        let good = record(&format!("v2\t1\tlocal\tHero\t{encoded}"));
        assert_eq!(
            decode_journal(&good).unwrap()[&1].creation.as_ref(),
            Some(&req)
        );
        for n in 1..good.len() {
            assert!(
                decode_journal(&good[..n]).is_err(),
                "accepted torn prefix {n}"
            );
        }
        for n in 0..encoded.len() {
            assert!(
                decode_journal(&record(&format!("v2\t1\tlocal\tHero\t{}", &encoded[..n]))).is_err()
            );
        }
        let mut malformed = encoded.clone();
        malformed.replace_range(48..56, "ffffffff");
        assert!(decode_journal(&record(&format!("v2\t1\tlocal\tHero\t{malformed}"))).is_err());
        let mut invalid = req.clone();
        invalid.physical_values.push(invalid.physical_values[0]);
        assert!(encode_creation(&invalid).is_err());
        invalid.physical_values = vec![(1, f32::NAN.to_bits())];
        assert!(encode_creation(&invalid).is_err());
        invalid.physical_values = (0..257).map(|i| (i, 0)).collect();
        assert!(encode_creation(&invalid).is_err());
        let mut mixed = record("v1\t1\tlocal\tHero");
        mixed.extend(record("v1\t2\tother\thero"));
        assert!(decode_journal(&mixed).is_err());
        let mut too_many = Vec::new();
        for i in 1..=6 {
            too_many.extend(record(&format!("v1\t{i}\tlocal\tHero {i}")));
        }
        assert!(decode_journal(&too_many).is_err());
    }
    #[test]
    fn persistence_exclusive_writer_and_corruption() {
        let dir = std::env::temp_dir().join(
            format!(
                "ac2-journal-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            )
            .replace(['(', ')'], ""),
        );
        fs::create_dir_all(&dir).unwrap();
        {
            let mut s = Store::open(&dir).unwrap();
            assert!(Store::open(&dir).is_err());
            assert_eq!(s.create("local", "Explorer").unwrap().id, 1);
            assert!(s.create("local", "explorer").is_err());
        }
        {
            let s = Store::open(&dir).unwrap();
            assert_eq!(s.characters[&1].name, "Explorer");
        }
        let path = dir.join("characters.journal");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        assert!(decode_journal(&bytes).is_err());
        assert!(decode_journal(&bytes[..bytes.len() - 1]).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
