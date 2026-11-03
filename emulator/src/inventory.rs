//! Bounded single-owner inventory transitions. No packet fields grant item authority.
use std::collections::BTreeMap;

pub const ITEM_NAMESPACE: u64 = 0x8130000000000000;
pub const BAG_SLOTS: u16 = 78; // Local policy: 42 + 12 + 12 + 12.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    Bag(u16),
    Equipped(u16),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub roll: Option<Roll>,
    pub id: u64,
    pub definition: u32,
    pub visual: u32,
    pub quantity: u32,
    pub stack_limit: u32,
    pub equipment_mask: u16,
    pub location: Location,
}
/// Persisted equipment roll. Zero/None retains historical item semantics.
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub struct Roll {pub armor:u16,pub tint:u16,pub level:u16,pub damage:u16,pub crafted:bool,pub wear:u16}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    items: BTreeMap<u64, Item>,
    high_water: u64,
    // Derived indexes are rebuilt on candidates before Store can publish them.
    // They never enter the journal or provide a second source of ownership.
    bags: [Option<u64>; BAG_SLOTS as usize],
    equipment: [Option<u64>; 11],
}
impl Default for Inventory {
    fn default() -> Self {
        Self {
            items: BTreeMap::new(),
            high_water: 0,
            bags: [None; BAG_SLOTS as usize],
            equipment: [None; 11],
        }
    }
}
#[derive(Clone, Debug)]
pub enum Command {
    Grant(Item),
    /// Server-only, monotonic per-instance durability; never accepted from a packet.
    ToolWear { item:u64, previous:u16, used:u16 },
    /// Restore missing native equipment metadata on a historical loot grant.
    /// This is a journaled server migration, never a client-selected mask.
    RepairEquipment { item:u64, definition:u32, mask:u16 },
    Move {
        item: u64,
        source: u16,
        destination: u16,
    },
    /// Dense native bag-list insertion. Destination is the pre-removal boundary.
    Reorder {
        item: u64,
        source: u16,
        destination: u16,
    },
    /// Grant an unrolled item as the first entry of the dense bag list (the others move up one).
    /// The client's player script lists a newly acquired item first (the retained pickup captures
    /// all name slot 0, and it ignores a later slot in the acquisition message), so a request
    /// naming a slot of a later pickup only agrees with the server's list when the item is
    /// first. The item's own location is ignored.
    GrantFront(Item),
    Equip {
        item: u64,
        slot: u16,
    },
    /// Native dense-list removal into an empty equipment slot. Kept distinct
    /// from historical `Equip` journal semantics, which can leave bag holes.
    Wear {
        item: u64,
        slot: u16,
    },
    Unequip {
        item: u64,
    },
    /// Native equipment-window removal into the dense bag list.
    UnequipAt {
        item: u64,
        slot: u16,
        destination: u16,
    },
    Split {
        item: u64,
        new_id: u64,
        quantity: u32,
        destination: u16,
    },
    /// Move `quantity` of a bag stack onto another stack of the same kind in the bag (a drop on
    /// an item, `ContainerDisplay::HandleListBoxDrop` -> the merge `MoveItem`). A whole stack
    /// leaves the dense bag list and closes its gap.
    Merge {
        item: u64,
        into: u64,
        quantity: u32,
    },
    Consume {
        item: u64,
        quantity: u32,
    },
    /// Remove one unstacked bag item and close its native list gap. Historical
    /// Consume remains unchanged for old journals and operator slot semantics.
    ConsumeDense {
        item: u64,
    },
    /// Take `quantity` units out of a bag stack and keep the rest where it is (a craft that uses
    /// part of a stack). The stack must hold more than `quantity`.
    ConsumePart {
        item: u64,
        quantity: u32,
    },
    /// Remove a complete, explicitly expected stack and close its bag gap.
    /// A distinct journal command preserves historical single-item semantics.
    ConsumeStackDense {
        item: u64,
        quantity: u32,
    },
}
#[derive(Debug)]
pub enum InventoryError {
    InvalidItem,
    NotOwned,
    StaleLocation,
    NoSpace,
    IncompatibleEquipment,
    InvalidStack,
}
impl std::fmt::Display for InventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InventoryError {}
impl Inventory {
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.values()
    }
    pub fn high_water(&self) -> u64 {
        self.high_water.max(ITEM_NAMESPACE)
    }
    pub fn free_slot(&self) -> Option<u16> {
        self.bags
            .iter()
            .position(Option::is_none)
            .map(|slot| slot as u16)
    }
    pub fn dense_bags(&self) -> bool {
        let count = self.bags.iter().flatten().count();
        self.bags[..count].iter().all(Option::is_some)
    }
    fn at(&self, location: Location) -> Option<u64> {
        match location {
            Location::Bag(slot) => self.bags.get(usize::from(slot)).copied().flatten(),
            Location::Equipped(slot) if slot.is_power_of_two() && slot <= 0x400 => {
                self.equipment[slot.trailing_zeros() as usize]
            }
            Location::Equipped(_) => None,
        }
    }
    fn reindex(&mut self) -> Result<(), InventoryError> {
        self.bags.fill(None);
        self.equipment.fill(None);
        for item in self.items.values() {
            let occupied = match item.location {
                Location::Bag(slot) => self.bags.get_mut(usize::from(slot)),
                Location::Equipped(slot) if slot.is_power_of_two() && slot <= 0x400 => {
                    self.equipment.get_mut(slot.trailing_zeros() as usize)
                }
                Location::Equipped(_) => None,
            }
            .ok_or(InventoryError::StaleLocation)?;
            if occupied.replace(item.id).is_some() {
                return Err(InventoryError::StaleLocation);
            }
        }
        Ok(())
    }
    fn owned(&self, id: u64) -> Result<&Item, InventoryError> {
        self.items.get(&id).ok_or(InventoryError::NotOwned)
    }
    /// Candidate state is published by Store only after its event has been flushed.
    pub fn apply(&self, command: &Command) -> Result<Self, InventoryError> {
        let mut next = self.clone();
        match command {
            Command::ToolWear {item,previous,used} => {
                let old=self.owned(*item)?;
                let durability=crate::professions::tool_durability(old.definition).ok_or(InventoryError::InvalidItem)?;
                let creation=crate::professions::creation_tool(old.definition).is_some();
                if !(matches!(old.location,Location::Bag(_))||creation&&old.location==Location::Equipped(0x40)) || old.quantity!=1 || old.stack_limit!=1
                    || old.roll.map_or(0,|r|r.wear)!=*previous || used<=previous || u32::from(*used)>durability || !creation&&u32::from(*used)==durability {
                    return Err(InventoryError::InvalidItem);
                }
                let next=next.items.get_mut(item).unwrap();
                let r=next.roll.get_or_insert(Roll{level:1,crafted:true,..Default::default()});r.wear=*used;
            }
            Command::RepairEquipment {item,definition,mask} => {
                let old=self.owned(*item)?;
                if old.definition!=*definition || old.equipment_mask!=0
                    || old.quantity!=1 || old.stack_limit!=1
                    || !matches!(old.location,Location::Bag(_))
                    || !mask.is_power_of_two() || *mask>32 {
                    return Err(InventoryError::IncompatibleEquipment);
                }
                next.items.get_mut(item).unwrap().equipment_mask=*mask;
            }
            Command::Grant(item) => {
                if item.id <= self.high_water()
                    || item.id >> 48 != ITEM_NAMESPACE >> 48
                    || item.definition >> 24 != 0x47
                    || item.visual == 0
                    || item.quantity == 0
                    || item.stack_limit == 0
                    || item.quantity > item.stack_limit
                    || item.equipment_mask & !0x7ff != 0
                    || item.roll.is_some_and(|r|r.armor>1000||r.damage>1000||r.tint>1000||!(1..=150).contains(&r.level)
                        ||item.quantity!=1||item.stack_limit!=1||if r.crafted { !crate::professions::valid_finish(item) }else if r.damage>0{!crate::treasure_weapons::valid_finish(item)}else{r.wear!=0||r.armor==0||crate::apparel::recovered_slot(item.definition,item.visual)!=Some(item.equipment_mask)})
                    || !matches!(item.location,Location::Bag(slot) if slot<BAG_SLOTS)
                {
                    return Err(InventoryError::InvalidItem);
                }
                if self.at(item.location).is_some() {
                    return Err(InventoryError::NoSpace);
                }
                next.high_water = item.id;
                next.items.insert(item.id, item.clone());
            }
            Command::GrantFront(item) => {
                let slot = self.free_slot().ok_or(InventoryError::NoSpace)?;
                if !self.dense_bags() || item.roll.is_some() {
                    return Err(InventoryError::StaleLocation);
                }
                let mut at_end = item.clone();
                at_end.location = Location::Bag(slot);
                next = self.apply(&Command::Grant(at_end))?.apply(&Command::Reorder {
                    item: item.id,
                    source: slot,
                    destination: 0,
                })?;
            }
            Command::Move {
                item,
                source,
                destination,
            } => {
                if *source >= BAG_SLOTS || *destination >= BAG_SLOTS {
                    return Err(InventoryError::StaleLocation);
                }
                let moved = self.owned(*item)?;
                if moved.location != Location::Bag(*source) {
                    return Err(InventoryError::StaleLocation);
                }
                if source == destination {
                    return Ok(next);
                }
                if let Some(other_id) = self.at(Location::Bag(*destination)) {
                    let other = &self.items[&other_id];
                    if (
                        moved.definition,
                        moved.visual,
                        moved.stack_limit,
                        moved.equipment_mask,
                    ) == (
                        other.definition,
                        other.visual,
                        other.stack_limit,
                        other.equipment_mask,
                    ) && moved.stack_limit > 1
                    {
                        let total = u32::from(moved.quantity) + u32::from(other.quantity);
                        if total > u32::from(other.stack_limit) {
                            return Err(InventoryError::InvalidStack);
                        }
                        next.items.get_mut(&other_id).unwrap().quantity = total;
                        next.items.remove(item);
                    } else {
                        next.items.get_mut(item).unwrap().location = Location::Bag(*destination);
                        next.items.get_mut(&other_id).unwrap().location = Location::Bag(*source);
                    }
                } else {
                    next.items.get_mut(item).unwrap().location = Location::Bag(*destination);
                }
            }
            Command::Reorder {
                item,
                source,
                destination,
            } => {
                let moved = self.owned(*item)?;
                if moved.location != Location::Bag(*source) {
                    return Err(InventoryError::StaleLocation);
                }
                let mut order: Vec<u64> = self.bags.iter().flatten().copied().collect();
                // Captures establish insertion in a dense list, not how holes
                // should compact. Refuse sparse bags rather than moving them.
                if self.bags[..order.len()].iter().any(Option::is_none)
                    || usize::from(*source) >= order.len()
                    || usize::from(*destination) > order.len()
                {
                    return Err(InventoryError::StaleLocation);
                }
                let destination = usize::from(*destination) - usize::from(source < destination);
                order.remove(usize::from(*source));
                order.insert(destination, *item);
                for (slot, id) in order.into_iter().enumerate() {
                    next.items.get_mut(&id).unwrap().location = Location::Bag(slot as u16);
                }
            }
            Command::Wear { item, slot } => {
                let order: Vec<u64> = self.bags.iter().flatten().copied().collect();
                if self.bags[..order.len()].iter().any(Option::is_none)
                    || self.at(Location::Equipped(*slot)).is_some()
                    || !matches!(self.owned(*item)?.location, Location::Bag(_))
                {
                    return Err(InventoryError::StaleLocation);
                }
                next = self.apply(&Command::Equip {
                    item: *item,
                    slot: *slot,
                })?;
                for (slot, id) in order.into_iter().filter(|id| id != item).enumerate() {
                    next.items.get_mut(&id).unwrap().location = Location::Bag(slot as u16);
                }
            }
            Command::Equip { item, slot } => {
                let equipped = self.owned(*item)?;
                if !slot.is_power_of_two()
                    || *slot > 0x400
                    || equipped.equipment_mask & slot == 0
                    || equipped.quantity != 1
                {
                    return Err(InventoryError::IncompatibleEquipment);
                }
                if equipped.location == Location::Equipped(*slot) {
                    return Ok(next);
                }
                let Location::Bag(source) = equipped.location else {
                    return Err(InventoryError::StaleLocation);
                };
                if let Some(previous) = self.at(Location::Equipped(*slot)) {
                    next.items.get_mut(&previous).unwrap().location = Location::Bag(source);
                }
                next.items.get_mut(item).unwrap().location = Location::Equipped(*slot);
            }
            Command::Unequip { item } => {
                if !matches!(self.owned(*item)?.location, Location::Equipped(_)) {
                    return Err(InventoryError::StaleLocation);
                }
                let destination = self.free_slot().ok_or(InventoryError::NoSpace)?;
                next.items.get_mut(item).unwrap().location = Location::Bag(destination);
            }
            Command::UnequipAt {
                item,
                slot,
                destination,
            } => {
                let original = self.owned(*item)?;
                let count = self
                    .items
                    .values()
                    .filter(|item| matches!(item.location, Location::Bag(_)))
                    .count();
                if !self.dense_bags()
                    || original.location != Location::Equipped(*slot)
                    || usize::from(*destination) > count
                    || *destination >= BAG_SLOTS
                {
                    return Err(InventoryError::StaleLocation);
                }
                if self.free_slot().is_none() {
                    return Err(InventoryError::NoSpace);
                }
                for entry in next.items.values_mut() {
                    if let Location::Bag(index) = &mut entry.location {
                        if *index >= *destination {
                            *index += 1;
                        }
                    }
                }
                next.items.get_mut(item).unwrap().location = Location::Bag(*destination);
            }
            Command::Split {
                item,
                new_id,
                quantity,
                destination,
            } => {
                let original = self.owned(*item)?;
                if !matches!(original.location, Location::Bag(_))
                    || *quantity == 0
                    || *quantity >= original.quantity
                    || *new_id <= self.high_water()
                    || new_id >> 48 != ITEM_NAMESPACE >> 48
                    || *destination >= BAG_SLOTS
                {
                    return Err(InventoryError::InvalidStack);
                }
                // A new stack at an occupied slot of the dense list goes in front of what is
                // there (the later items move up by one).
                if self.at(Location::Bag(*destination)).is_some() {
                    let count = self.bags.iter().flatten().count();
                    if !self.dense_bags() || count >= usize::from(BAG_SLOTS) {
                        return Err(InventoryError::NoSpace);
                    }
                    for entry in next.items.values_mut() {
                        if let Location::Bag(slot) = &mut entry.location {
                            if *slot >= *destination {
                                *slot += 1;
                            }
                        }
                    }
                }
                let mut split = original.clone();
                split.id = *new_id;
                split.quantity = *quantity;
                split.location = Location::Bag(*destination);
                next.items.get_mut(item).unwrap().quantity -= quantity;
                next.items.insert(*new_id, split);
                next.high_water = *new_id;
            }
            Command::Merge {
                item,
                into,
                quantity,
            } => {
                let moved = self.owned(*item)?;
                let target = self.owned(*into)?;
                let (Location::Bag(source), Location::Bag(_)) = (moved.location, target.location)
                else {
                    return Err(InventoryError::StaleLocation);
                };
                if item == into
                    || !self.dense_bags()
                    || moved.stack_limit <= 1
                    || (moved.definition, moved.visual, moved.stack_limit, moved.equipment_mask)
                        != (target.definition, target.visual, target.stack_limit, target.equipment_mask)
                {
                    return Err(InventoryError::InvalidStack);
                }
                if *quantity == 0
                    || *quantity > moved.quantity
                    || u32::from(target.quantity) + u32::from(*quantity) > u32::from(target.stack_limit)
                {
                    return Err(InventoryError::InvalidStack);
                }
                next.items.get_mut(into).unwrap().quantity += quantity;
                if *quantity == moved.quantity {
                    next.items.remove(item);
                    for entry in next.items.values_mut() {
                        if let Location::Bag(slot) = &mut entry.location {
                            if *slot > source {
                                *slot -= 1;
                            }
                        }
                    }
                } else {
                    next.items.get_mut(item).unwrap().quantity -= quantity;
                }
            }
            Command::ConsumeDense { item } | Command::ConsumeStackDense { item, .. } => {
                let original = self.owned(*item)?;
                let Location::Bag(source) = original.location else {
                    return Err(InventoryError::InvalidItem);
                };
                let expected = match command {
                    Command::ConsumeStackDense { quantity, .. } => *quantity,
                    _ => 1,
                };
                if !self.dense_bags() || expected == 0 || original.quantity != expected {
                    return Err(InventoryError::InvalidStack);
                }
                next.items.remove(item);
                for entry in next.items.values_mut() {
                    if let Location::Bag(slot) = &mut entry.location {
                        if *slot > source {
                            *slot -= 1;
                        }
                    }
                }
            }
            Command::ConsumePart { item, quantity } => {
                let original = self.owned(*item)?;
                if *quantity == 0
                    || *quantity >= original.quantity
                    || !matches!(original.location, Location::Bag(_))
                {
                    return Err(InventoryError::InvalidStack);
                }
                next.items.get_mut(item).unwrap().quantity -= quantity;
            }
            Command::Consume { item, quantity } => {
                let original = self.owned(*item)?;
                if *quantity == 0
                    || *quantity > original.quantity
                    || !matches!(original.location, Location::Bag(_))
                {
                    return Err(InventoryError::InvalidStack);
                }
                if *quantity == original.quantity {
                    next.items.remove(item);
                } else {
                    next.items.get_mut(item).unwrap().quantity -= quantity;
                }
            }
        }
        next.reindex()?;
        Ok(next)
    }
}
impl Command {
    /// Local journal format; independent of the native serialized request graphs.
    pub fn encode(&self) -> String {
        match self {
            Self::ToolWear {item,previous,used} => format!("j,{item},{previous},{used}"),
            Self::Grant(item) if item.roll.is_some_and(|r|r.crafted&&r.wear>0) => {
                let r=item.roll.unwrap();format!("H,{},{},{},{},{},{},{},{},{},{},{},{}",item.id,item.definition,item.visual,item.quantity,item.stack_limit,item.equipment_mask,
                    match item.location{Location::Bag(s)|Location::Equipped(s)=>s},r.armor,r.tint,r.level,r.damage,r.wear)
            },
            Self::RepairEquipment {item,definition,mask} => format!("q,{item},{definition},{mask}"),
            Self::Grant(item) if item.roll.is_some_and(|r|r.crafted) => {
                let r=item.roll.unwrap();format!("F,{},{},{},{},{},{},{},{},{},{},{}",item.id,item.definition,item.visual,item.quantity,item.stack_limit,item.equipment_mask,
                    match item.location{Location::Bag(s)|Location::Equipped(s)=>s},r.armor,r.tint,r.level,r.damage)
            },
            Self::Grant(item) if item.roll.is_some_and(|r| r.damage > 0) => {
                let r=item.roll.unwrap();format!("W,{},{},{},{},{},{},{},{},{},{},{}",item.id,item.definition,item.visual,item.quantity,item.stack_limit,item.equipment_mask,
                    match item.location{Location::Bag(s)|Location::Equipped(s)=>s},r.armor,r.tint,r.level,r.damage)
            },
            Self::Grant(item) if item.roll.is_some() => {
                let r=item.roll.unwrap();format!("G,{},{},{},{},{},{},{},{},{},{}",item.id,item.definition,item.visual,item.quantity,item.stack_limit,item.equipment_mask,
                    match item.location{Location::Bag(s)|Location::Equipped(s)=>s},r.armor,r.tint,r.level)
            },
            Self::Grant(item) => format!(
                "g,{},{},{},{},{},{},{}",
                item.id,
                item.definition,
                item.visual,
                item.quantity,
                item.stack_limit,
                item.equipment_mask,
                match item.location {
                    Location::Bag(slot) => slot,
                    Location::Equipped(_) => u16::MAX,
                }
            ),
            Self::GrantFront(item) => format!(
                "f,{},{},{},{},{},{}",
                item.id, item.definition, item.visual, item.quantity, item.stack_limit, item.equipment_mask
            ),
            Self::Move {
                item,
                source,
                destination,
            } => format!("m,{item},{source},{destination}"),
            Self::Reorder {
                item,
                source,
                destination,
            } => {
                format!("r,{item},{source},{destination}")
            }
            Self::Equip { item, slot } => format!("e,{item},{slot}"),
            Self::Wear { item, slot } => format!("w,{item},{slot}"),
            Self::Unequip { item } => format!("u,{item}"),
            Self::UnequipAt {
                item,
                slot,
                destination,
            } => format!("n,{item},{slot},{destination}"),
            Self::Split {
                item,
                new_id,
                quantity,
                destination,
            } => format!("s,{item},{new_id},{quantity},{destination}"),
            Self::Merge { item, into, quantity } => format!("M,{item},{into},{quantity}"),
            Self::ConsumePart { item, quantity } => format!("p,{item},{quantity}"),
            Self::Consume { item, quantity } => format!("c,{item},{quantity}"),
            Self::ConsumeDense { item } => format!("d,{item}"),
            Self::ConsumeStackDense { item, quantity } => format!("b,{item},{quantity}"),
        }
    }
    pub fn decode(text: &str) -> Result<Self, InventoryError> {
        if text.len() > 256 {
            return Err(InventoryError::InvalidItem);
        }
        let mut fields = text.split(',');
        let kind = fields.next().ok_or(InventoryError::InvalidItem)?;
        let values: Vec<u64> = fields
            .map(|v| v.parse().map_err(|_| InventoryError::InvalidItem))
            .collect::<Result<_, _>>()?;
        let small = |v: u64| u16::try_from(v).map_err(|_| InventoryError::InvalidItem);
        let word = |v: u64| u32::try_from(v).map_err(|_| InventoryError::InvalidItem);
        Ok(match (kind, values.as_slice()) {
            ("j",[item,previous,used])=>Self::ToolWear{item:*item,previous:small(*previous)?,used:small(*used)?},
            ("H",[id,did,visual,quantity,limit,mask,slot,armor,tint,level,damage,wear])=>Self::Grant(Item{
                id:*id,definition:word(*did)?,visual:word(*visual)?,quantity:word(*quantity)?,stack_limit:word(*limit)?,equipment_mask:small(*mask)?,location:Location::Bag(small(*slot)?),
                roll:Some(Roll{armor:small(*armor)?,tint:small(*tint)?,level:small(*level)?,damage:small(*damage)?,wear:small(*wear)?,crafted:true})}),
            ("F",[id,did,visual,quantity,limit,mask,slot,armor,tint,level,damage])=>Self::Grant(Item{
                id:*id,definition:word(*did)?,visual:word(*visual)?,quantity:word(*quantity)?,stack_limit:word(*limit)?,equipment_mask:small(*mask)?,location:Location::Bag(small(*slot)?),
                roll:Some(Roll{armor:small(*armor)?,tint:small(*tint)?,level:small(*level)?,damage:small(*damage)?,crafted:true,..Default::default()})}),
            ("W",[id,did,visual,quantity,limit,mask,slot,armor,tint,level,damage])=>Self::Grant(Item{
                id:*id,definition:word(*did)?,visual:word(*visual)?,quantity:word(*quantity)?,stack_limit:word(*limit)?,equipment_mask:small(*mask)?,location:Location::Bag(small(*slot)?),
                roll:Some(Roll{armor:small(*armor)?,tint:small(*tint)?,level:small(*level)?,damage:small(*damage)?,..Default::default()})}),
            ("G",[id,did,visual,quantity,limit,mask,slot,armor,tint,level])=>Self::Grant(Item{
                id:*id,definition:word(*did)?,visual:word(*visual)?,quantity:word(*quantity)?,stack_limit:word(*limit)?,equipment_mask:small(*mask)?,location:Location::Bag(small(*slot)?),
                roll:Some(Roll{armor:small(*armor)?,tint:small(*tint)?,level:small(*level)?,..Default::default()})}),
            ("q",[item,definition,mask]) => Self::RepairEquipment {item:*item,definition:word(*definition)?,mask:small(*mask)?},
            ("g", [id, did, visual, quantity, limit, mask, slot]) => Self::Grant(Item {roll: None,
                id: *id,
                definition: word(*did)?,
                visual: word(*visual)?,
                quantity: word(*quantity)?,
                stack_limit: word(*limit)?,
                equipment_mask: small(*mask)?,
                location: Location::Bag(small(*slot)?),
            }),
            ("f", [id, did, visual, quantity, limit, mask]) => Self::GrantFront(Item {
                roll: None,
                id: *id,
                definition: word(*did)?,
                visual: word(*visual)?,
                quantity: word(*quantity)?,
                stack_limit: word(*limit)?,
                equipment_mask: small(*mask)?,
                location: Location::Bag(0),
            }),
            ("m", [item, source, destination]) => Self::Move {
                item: *item,
                source: small(*source)?,
                destination: small(*destination)?,
            },
            ("r", [item, source, destination]) => Self::Reorder {
                item: *item,
                source: small(*source)?,
                destination: small(*destination)?,
            },
            ("e", [item, slot]) => Self::Equip {
                item: *item,
                slot: small(*slot)?,
            },
            ("w", [item, slot]) => Self::Wear {
                item: *item,
                slot: small(*slot)?,
            },
            ("u", [item]) => Self::Unequip { item: *item },
            ("n", [item, slot, destination]) => Self::UnequipAt {
                item: *item,
                slot: small(*slot)?,
                destination: small(*destination)?,
            },
            ("s", [item, new_id, quantity, destination]) => Self::Split {
                item: *item,
                new_id: *new_id,
                quantity: word(*quantity)?,
                destination: small(*destination)?,
            },
            ("M", [item, into, quantity]) => Self::Merge {
                item: *item,
                into: *into,
                quantity: word(*quantity)?,
            },
            ("p", [item, quantity]) => Self::ConsumePart {
                item: *item,
                quantity: word(*quantity)?,
            },
            ("c", [item, quantity]) => Self::Consume {
                item: *item,
                quantity: word(*quantity)?,
            },
            ("d", [item]) => Self::ConsumeDense { item: *item },
            ("b", [item, quantity]) => Self::ConsumeStackDense {
                item: *item,
                quantity: word(*quantity)?,
            },
            _ => return Err(InventoryError::InvalidItem),
        })
    }
    pub fn allocated_id(&self) -> Option<u64> {
        match self {
            Self::Grant(item) | Self::GrantFront(item) => Some(item.id),
            Self::Split { new_id, .. } => Some(*new_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod stack_tests {
    use super::*;

    fn stack(offset: u64, quantity: u32) -> Item {
        Item {
            roll: None,
            id: ITEM_NAMESPACE + offset,
            definition: 0x4700_10f3,
            visual: 0x1f00_0234,
            quantity,
            stack_limit: 100,
            equipment_mask: 0,
            location: Location::Bag(0),
        }
    }
    fn slot(bag: &Inventory, offset: u64) -> Location {
        bag.items().find(|i| i.id == ITEM_NAMESPACE + offset).unwrap().location
    }

    #[test]
    fn a_front_grant_lists_first_and_a_part_consumed_stack_keeps_its_place() {
        let first = stack(1, 10);
        let bag = Inventory::default().apply(&Command::Grant(first)).unwrap();
        let bag = bag.apply(&Command::GrantFront(stack(2, 10))).unwrap();
        assert_eq!(slot(&bag, 2), Location::Bag(0));
        assert_eq!(slot(&bag, 1), Location::Bag(1));
        let part = Command::ConsumePart { item: ITEM_NAMESPACE + 1, quantity: 4 };
        let bag = bag.apply(&part).unwrap();
        assert_eq!(slot(&bag, 1), Location::Bag(1));
        assert_eq!(bag.items().find(|i| i.id == ITEM_NAMESPACE + 1).unwrap().quantity, 6);
        // The whole stack, or nothing, or more than it holds, is not a part.
        for quantity in [0, 6, 7] {
            assert!(bag.apply(&Command::ConsumePart { item: ITEM_NAMESPACE + 1, quantity }).is_err());
        }
        // The journal round trip.
        for command in [Command::GrantFront(stack(3, 5)), part] {
            assert_eq!(Command::decode(&command.encode()).unwrap().encode(), command.encode());
        }
    }
}
