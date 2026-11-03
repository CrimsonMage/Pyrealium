//! Ground items share the character journal: removal/publication and pickup/grant
//! are single durable transactions, including across a server restart.
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
        self.append_record(&format!("wd1\t{character}\t{item}\t{}",encode_placement(placement)))?;
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
        self.append_record(&format!("wp1\t{character}\t{id}\t{}",item.id))?;
        self.characters.get_mut(&character).unwrap().inventory=next;
        self.drops.remove(&id);
        Ok(item)
    }
}
pub(super) fn replay(fields: &[&str], characters: &mut BTreeMap<u64,Character>, drops: &mut BTreeMap<u64,DroppedItem>) -> io::Result<()> {
    let character=fields[1].parse::<u64>().map_err(|_|invalid("Invalid ground-item owner"))?;
    let id=fields[2].parse::<u64>().map_err(|_|invalid("Invalid ground-item identity"))?;
    let next_id=characters.values().map(|c|c.inventory.high_water()).max().unwrap_or(crate::inventory::ITEM_NAMESPACE).checked_add(1);
    let saved=characters.get_mut(&character).filter(|c|!c.deleted && c.progression.alive()).ok_or_else(||invalid("Invalid ground-item character"))?;
    if fields[0]=="wd1" {
        let placement=decode_placement(fields[3])?;
        let (next,item)=remove(&saved.inventory,id)?;
        let dropped=DroppedItem {item,placement};
        if drops.len()>=MAX_DROPS || drops.contains_key(&dropped.id()) {return Err(invalid("Invalid ground-item publication"));}
        saved.inventory=next;drops.insert(dropped.id(),dropped);
    } else {
        let allocated=fields[3].parse::<u64>().map_err(|_|invalid("Invalid pickup allocation"))?;
        if Some(allocated)!=next_id {return Err(invalid("Invalid pickup identity progression"));}
        let dropped=drops.get(&id).ok_or_else(||invalid("Ground item already taken"))?;
        saved.inventory=grant(&saved.inventory,dropped,allocated)?.0;
        drops.remove(&id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drop_pickup_restart_and_compaction_are_atomic() {
        let dir=std::env::temp_dir().join(format!("ac2-drop-{}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a,b,ground);
        {
            let mut s=Store::open(&dir).unwrap();
            a=s.create("alice","Alice").unwrap().id;b=s.create("bob","Bob").unwrap().id;
            let item=crate::item_services::potion(s.next_item_id().unwrap(),0);
            s.inventory_command("alice",a,Command::Grant(item.clone())).unwrap();
            assert!(s.drop_item("bob",a,item.id,Placement::HUMAN_START).is_err());
            ground=s.drop_item("alice",a,item.id,Placement::HUMAN_START).unwrap().id();
            assert_eq!(s.characters[&a].inventory.items().count(),0);
            assert!(s.drop_item("alice",a,item.id,Placement::HUMAN_START).is_err());
            s.compact_journal().unwrap();
        }
        {
            let mut s=Store::open(&dir).unwrap();assert!(s.drops.contains_key(&ground));
            let item=s.pickup_item("bob",b,ground).unwrap();
            assert_eq!(item.definition,crate::item_services::POTION_DEFINITION);
            assert!(s.pickup_item("alice",a,ground).is_err());
            s.compact_journal().unwrap();
        }
        {let s=Store::open(&dir).unwrap();assert!(s.drops.is_empty());assert_eq!(s.characters[&b].inventory.items().count(),1);}
        std::fs::remove_dir_all(dir).unwrap();
    }
}
