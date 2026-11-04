//! A character's setup in the world: starter gear and kit, skills, the lifestone binding and relocations.
use super::*;

impl Store {
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
        let quests = character.quests.clone();
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
}
