//! The durable steps of play: progression, skills, quests, rewards, items, trades and the UI settings.
use super::*;

impl Store {
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
    /// Quest records of a set that changes quests and talks only (a death, a logout, a run's
    /// expiry, or a delayed set's quest part): durable whatever the character is doing.
    pub fn quest_records(&mut self,id:u64,records:&[crate::quests::Command])->io::Result<()> {
        if records.is_empty() {return Ok(());}
        let c=self.characters.get(&id).filter(|c|!c.deleted).ok_or_else(||invalid("Quest record owner unavailable"))?;
        let next=records.iter().try_fold(c.quests.clone(),|b,r|b.apply(r)).map_err(io::Error::other)?;
        self.touch(id);
        self.characters.get_mut(&id).unwrap().quests=next;
        Ok(())
    }
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
        actor: u64,
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
        let (kill_quests,kill_xp)=kill_emotes(character,entity,actor,time);
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
        actor: u64,
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
        let (kill_quests,kill_xp)=kill_emotes(character,entity,actor,time);
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
    /// A trade's swap (`trade`): both characters' item commands checked first, then both
    /// applied, so neither changes alone; the next save writes both in one transaction.
    /// `first` is the first new item id, used by `a`'s grants and then `b`'s.
    pub fn swap_items(
        &mut self,
        a: u64,
        a_items: &[crate::inventory::Command],
        b: u64,
        b_items: &[crate::inventory::Command],
        first: u64,
    ) -> io::Result<()> {
        if a == b {
            return Err(invalid("a swap needs two characters"));
        }
        let granted = a_items.iter().filter(|c| c.allocated_id().is_some()).count() as u64;
        let mut next = Vec::new();
        for (id, items, start) in [(a, a_items, first), (b, b_items, first + granted)] {
            let character = self.characters.get(&id).filter(|c| !c.deleted).ok_or_else(|| invalid("swap character missing"))?;
            if items.is_empty() {
                continue;
            }
            let (inventory, _) = crate::player_transaction::candidate(&character.inventory, &character.progression, items, &[], Some(start))?;
            next.push((id, inventory));
        }
        for (id, inventory) in next {
            self.touch(id);
            self.characters.get_mut(&id).unwrap().inventory = inventory;
        }
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
    pub fn update_chat_options(
        &mut self,
        account: &str,
        id: u64,
        options: crate::preferences::ChatOptions,
    ) -> io::Result<()> {
        let character = self.characters.get(&id)
            .filter(|character| character.account == account && !character.deleted)
            .ok_or_else(|| invalid("chat settings character not owned"))?;
        if character.chat_options.as_ref() == Some(&options) { return Ok(()); }
        self.touch(id);
        self.characters.get_mut(&id).unwrap().chat_options = Some(options);
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
}
