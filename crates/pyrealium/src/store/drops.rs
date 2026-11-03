//! Ground items: removal/publication and pickup/grant change the character and the ground in
//! one step, saved together in the same snapshot.
use super::*;
use crate::inventory::{Command, Inventory, Item, Location};
pub const MAX_DROPS: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub struct DroppedItem {
    pub item: Item,
    pub placement: Placement,
}
impl DroppedItem {
    pub fn id(&self) -> u64 { 0x8180000000000000 | (self.item.id & 0x0000ffffffffffff) }
}
fn remove(inventory: &Inventory, item: u64) -> io::Result<(Inventory, Item)> {
    let owned = inventory.items().find(|v| v.id == item).cloned()
        .ok_or_else(|| invalid("Item is not in your backpack."))?;
    if !matches!(owned.location, Location::Bag(_)) {return Err(invalid("Unequip the item before dropping it."));}
    let next = inventory.apply(&Command::ConsumeStackDense { item, quantity: owned.quantity }).map_err(io::Error::other)?;
    Ok((next, owned))
}
fn grant(inventory: &Inventory, drop: &DroppedItem, new_id: u64) -> io::Result<(Inventory, Item)> {
    let mut item = drop.item.clone();
    item.id = new_id;
    item.location = Location::Bag(inventory.free_slot().ok_or_else(|| invalid("Your backpack is full."))?);
    let next = inventory.apply(&Command::Grant(item.clone())).map_err(io::Error::other)?;
    Ok((next, item))
}
impl Store {
    pub fn drop_candidate(&self, account: &str, character: u64, item: u64, placement: Placement) -> io::Result<DroppedItem> {
        let saved=self.characters.get(&character).filter(|c|!c.deleted && c.account==account && c.progression.alive())
            .ok_or_else(|| invalid("You must be alive to drop items."))?;
        if !placement.valid() || self.drops.len()>=MAX_DROPS {return Err(invalid("No room for more ground items."));}
        let (_,item)=remove(&saved.inventory,item)?;
        let dropped=DroppedItem {item,placement};
        if self.drops.contains_key(&dropped.id()) {return Err(invalid("Item is already on the ground."));}
        Ok(dropped)
    }
    pub fn drop_item(&mut self, account: &str, character: u64, item: u64, placement: Placement) -> io::Result<DroppedItem> {
        let dropped=self.drop_candidate(account,character,item,placement)?;
        let (next,_)=remove(&self.characters[&character].inventory,item)?;
        self.touch(character);self.ground_dirty=true;
        self.characters.get_mut(&character).unwrap().inventory=next;
        self.drops.insert(dropped.id(),dropped.clone());
        Ok(dropped)
    }
    pub fn pickup_candidate(&self, account: &str, character: u64, id: u64) -> io::Result<Item> {
        let saved=self.characters.get(&character).filter(|c|!c.deleted && c.account==account && c.progression.alive())
            .ok_or_else(||invalid("You must be alive to pick up items."))?;
        let dropped=self.drops.get(&id).ok_or_else(||invalid("Item is no longer on the ground."))?;
        grant(&saved.inventory,dropped,self.next_item_id()?).map(|(_,item)|item)
    }
    pub fn pickup_item(&mut self, account: &str, character: u64, id: u64) -> io::Result<Item> {
        let item=self.pickup_candidate(account,character,id)?;
        let next=self.characters[&character].inventory.apply(&Command::Grant(item.clone())).map_err(io::Error::other)?;
        self.touch(character);self.ground_dirty=true;
        self.characters.get_mut(&character).unwrap().inventory=next;
        self.drops.remove(&id);
        Ok(item)
    }
}
