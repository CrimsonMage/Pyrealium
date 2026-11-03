//! Bounded inventory sorting request profile from retained packet captures.
//! Parses data only; does not instantiate interpreted objects or execute scripts.
use crate::{
    inventory::{BAG_SLOTS, Command},
    world::WorldError,
};

pub const MOVE: u32 = 0x820e0049;
pub const DROP: u32 = 0x820e0036;
pub const MESSAGE_BYTES: usize = 296;
const TAGS: [u8; 51] = [
    2, 2, 0x84, 0xcd, 2, 2, 4, 2, 0x84, 0xcd, 2, 4, 2, 2, 2, 2, 0x84, 0xcd, 0x82, 0xcd, 2, 2, 4, 2,
    4, 2, 4, 2, 0x84, 0xcd, 1, 2, 0x84, 0xcd, 2, 2, 0x84, 0xcd, 4, 4, 2, 2, 2, 2, 0x84, 0xcd, 2, 2,
    2, 2, 4,
];


/// Whether two runs of interpreter slot tags agree up to the integer kind: the retail
/// interpreter tags a script integer 2 (0x82 for a 64-bit one) or, when it came from a native
/// argument or a ulong field, 4 (0x84). The kind is not part of the value and the packing
/// client's wlib decides it, so a request is judged by what each slot holds (a reference, an
/// integer, a float, uninitialised), not by the integer's declared kind.
pub fn tags_agree(got: &[u8], want: &[u8]) -> bool {
    let kind = |t: u8| match t {
        4 => 2,
        0x84 => 0x82,
        t => t,
    };
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| kind(*g) == kind(*w))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropRequest {pub moved: MoveRequest, pub landblock: u32}
impl DropRequest {
    pub fn arguments(bytes: &[u8]) -> Result<Self, WorldError> {
        if bytes.len()!=MESSAGE_BYTES-12 {return Err(WorldError::InvalidValue);}
        let word=|i:usize|u32::from_le_bytes(bytes[28+i*4..32+i*4].try_into().unwrap());
        let landblock=word(2);
        if landblock==0 || landblock & 0xffff !=0 || word(3)!=0 {return Err(WorldError::InvalidValue);}
        for i in [8,32] {if word(i)!=landblock || word(i+1)!=0 {return Err(WorldError::InvalidValue);}}
        // A drop names no slot: -1 in the packets captured from later clients, 0 in the one the 7230
        // player script builds from the UI's literal slot 0 (`gm3DItemsUI::FinishItemDrop`).
        for i in [11,24,50] {if !matches!(word(i),0|u32::MAX) {return Err(WorldError::InvalidValue);}}
        let mut normalized=bytes.to_vec();
        for i in [2,8,32] {
            normalized[28+i*4..36+i*4].copy_from_slice(&bytes[28+16*4..36+16*4]);
        }
        for i in [11,24,50] {normalized[28+i*4..32+i*4].fill(0);}
        // The move type: 0 in the captured packets, 33 (a move to the world) in the 7230 script's.
        if !matches!(word(12),0|33) {return Err(WorldError::InvalidValue);}
        normalized[28+12*4..32+12*4].fill(0);
        Ok(Self {moved:MoveRequest::arguments(&normalized)?,landblock})
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;
    #[test]
    fn drop_descriptor_checks_all_repeated_fields_and_truncations() {
        let mut fields=[0u32;51];
        for i in [0,10,13,23,25,26,35,47,49] {fields[i]=1;}
        for i in [2,8,32] {fields[i]=0x02990000;}
        for i in [16,36] {fields[i]=7;}
        for i in [28,44] {fields[i]=42;fields[i+1]=0x81300000;}
        for i in [11,24,30,50] {fields[i]=u32::MAX;}
        let mut bytes=Vec::new();
        for v in [5u32,1,1,0x44000100,0,0x01f80000,51] {bytes.extend(v.to_le_bytes());}
        for v in fields {bytes.extend(v.to_le_bytes());}bytes.extend(TAGS);bytes.push(0);
        let request=DropRequest::arguments(&bytes).unwrap();
        assert_eq!(request.landblock,0x02990000);assert_eq!(request.moved.owner,7);
        for end in 0..bytes.len() {assert!(DropRequest::arguments(&bytes[..end]).is_err());}
        for field in [2,3,8,16,24,25,32,36,39,44,50] {
            let mut bad=bytes.clone();bad[28+field*4]^=1;
            assert!(DropRequest::arguments(&bad).is_err(),"accepted inconsistent field {field}");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveRequest {
    pub owner: u64,
    pub item: u64,
    pub source: u16,
    pub destination: u16,
    /// Complete quantity requested by the native drag; never a split amount.
    pub quantity: u32,
}
impl MoveRequest {
    pub fn arguments(bytes: &[u8]) -> Result<Self, WorldError> {
        if bytes.len() != MESSAGE_BYTES - 12 {
            return Err(WorldError::InvalidValue);
        }
        let mut message = Vec::with_capacity(MESSAGE_BYTES);
        for word in [0x10053u32, MOVE, bytes.len() as u32] {
            message.extend(word.to_le_bytes());
        }
        message.extend(bytes);
        Self::decode(&message)
    }
    /// Input is one reassembled application message, without transport headers.
    pub fn decode(message: &[u8]) -> Result<Self, WorldError> {
        if message.len() != MESSAGE_BYTES {
            return Err(WorldError::InvalidValue);
        }
        let word =
            |offset: usize| u32::from_le_bytes(message[offset..offset + 4].try_into().unwrap());
        if word(0) != 0x10053
            || word(4) != MOVE
            || word(8) != 284
            || word(12) != 5
            || word(16) != 1
            || word(20) == u32::MAX
            || word(24) != 0x44000100
            || word(28) != 0
            || word(32) != 0x01f80000
            || word(36) != 51
            || message[295] != 0
        {
            return Err(WorldError::InvalidValue);
        }
        let fields: [u32; 51] = std::array::from_fn(|i| word(40 + i * 4));
        let mut expected_tags = TAGS;
        // Captured drag requests use a uint source ordinal when nonzero and
        // a uint whole-stack count instead of the single-item constant.
        if !matches!(message[245], 2 | 4) {
            return Err(WorldError::InvalidValue);
        }
        expected_tags[1] = message[245];
        if fields[25] > 1 { expected_tags[25] = 4; }
        if !tags_agree(&message[244..295], &expected_tags) {
            return Err(WorldError::InvalidValue);
        }
        let owner = u64::from(fields[2]) | (u64::from(fields[3]) << 32);
        let item = u64::from(fields[28]) | (u64::from(fields[29]) << 32);
        if [0, u64::MAX].contains(&owner)
            || [0, u64::MAX].contains(&item)
            || owner == item
            || fields[1] >= u32::from(BAG_SLOTS)
            || fields[11] > u32::from(BAG_SLOTS)
            || fields[25] == 0
            || fields[25] > u32::from(u16::MAX)
            || fields[25] != fields[26]
        {
            return Err(WorldError::InvalidValue);
        }
        // Verify repeated identities, positions, discriminants and absent optional
        // metadata. No client-provided definition/icon/appearance becomes authority.
        let mut expected = [0u32; 51];
        for index in [0, 10, 13, 23, 25, 26, 35, 47, 49] {
            expected[index] = 1;
        }
        for index in [2, 8, 16, 32, 36] {
            expected[index] = owner as u32;
            expected[index + 1] = (owner >> 32) as u32;
        }
        for index in [28, 44] {
            expected[index] = item as u32;
            expected[index + 1] = (item >> 32) as u32;
        }
        expected[1] = fields[1];
        expected[39] = fields[1];
        for index in [11, 24, 50] {
            expected[index] = fields[11];
        }
        expected[30] = u32::MAX;
        expected[25] = fields[25];
        expected[26] = fields[26];
        if fields != expected {
            return Err(WorldError::InvalidValue);
        }
        Ok(Self {
            owner,
            item,
            source: fields[1] as u16,
            destination: fields[11] as u16,
            quantity: fields[25],
        })
    }
    /// A drop on an empty slot past the last item: the retail UI sends that slot's index
    /// (`ContainerDisplay::GetNewIndex`), which lands the item at the end of the dense list.
    pub fn clamped(mut self, bag_items: usize) -> Self {
        self.destination = self.destination.min(bag_items.min(usize::from(u16::MAX)) as u16);
        self
    }
    pub fn command(&self, owner: u64) -> Result<Command, WorldError> {
        if self.owner != owner {
            return Err(WorldError::InvalidValue);
        }
        Ok(Command::Reorder {
            item: self.item,
            source: self.source,
            destination: self.destination,
        })
    }
    pub fn checked_command(
        &self,
        owner: u64,
        inventory: &crate::inventory::Inventory,
    ) -> Result<Command, WorldError> {
        if !inventory.items().any(|item| {
            item.id == self.item
                && item.quantity == self.quantity
                && item.quantity > 0
                && item.location == crate::inventory::Location::Bag(self.source)
        }) {
            return Err(WorldError::InvalidValue);
        }
        self.command(owner)
    }

    /// Same-IID whole-item move acknowledgment, including a complete stack.
    /// Optional per-item graphs and partial-stack movement remain unsupported.
    pub fn acknowledgment(
        &self,
        item: &crate::inventory::Item,
        metadata: Metadata,
    ) -> Result<Vec<u8>, WorldError> {
        self.acknowledgment_as(0x820e0055, item, metadata)
    }
    /// The same descriptor under another `WM_Inventory` function (`MoveItem_Done`, 0x5e).
    pub fn acknowledgment_as(
        &self,
        function: u32,
        item: &crate::inventory::Item,
        metadata: Metadata,
    ) -> Result<Vec<u8>, WorldError> {
        if item.id != self.item {
            return Err(WorldError::InvalidValue);
        }
        self.descriptor_as(function, item, metadata, None)
    }
    /// The answer to a request that split part of a stack off: `ReorganizeContents_Done` (0x55)
    /// for the stack the request named (`self.item`, `quantity` of it) with the new stack as the
    /// descriptor's `splitItemID` (slot 28) and `allItemUnitsTakenFlag` (slot 23) clear.
    /// `CInventorySystem::ReorganizeContents_Done` removes the item from its container only
    /// when all its units were taken, and adds `splitItemID` at `actualTargetSlot` when the
    /// client does not know it yet (`InventorySystem::f29`), which is how the client lists the
    /// new stack. `split.location` is the new stack's slot.
    pub fn split_acknowledgment(
        &self,
        split: &crate::inventory::Item,
        metadata: Metadata,
    ) -> Result<Vec<u8>, WorldError> {
        if split.id == self.item {
            return Err(WorldError::InvalidValue);
        }
        self.descriptor_as(0x820e0055, split, metadata, Some(self.item))
    }
    fn descriptor_as(
        &self,
        function: u32,
        item: &crate::inventory::Item,
        metadata: Metadata,
        split_of: Option<u64>,
    ) -> Result<Vec<u8>, WorldError> {
        if item.quantity != self.quantity || item.quantity == 0 {
            return Err(WorldError::InvalidValue);
        }
        let crate::inventory::Location::Bag(final_slot) = item.location else {
            return Err(WorldError::InvalidValue);
        };
        let mut fields = [0u32; 51];
        for index in [7, 10, 13, 23, 26, 35, 47] {
            fields[index] = 1;
        }
        for index in [2, 8, 16, 32, 36] {
            fields[index] = self.owner as u32;
            fields[index + 1] = (self.owner >> 32) as u32;
        }
        for index in [28, 44] {
            fields[index] = item.id as u32;
            fields[index + 1] = (item.id >> 32) as u32;
        }
        if let Some(original) = split_of {
            fields[44] = original as u32;
            fields[45] = (original >> 32) as u32;
            fields[23] = 0;
        }
        fields[1] = u32::from(self.source);
        fields[39] = u32::from(self.source);
        fields[11] = u32::from(self.destination);
        fields[24] = u32::from(self.destination);
        fields[50] = u32::from(final_slot);
        fields[6] = metadata.icon;
        fields[22] = item.definition;
        fields[30] = u32::MAX;
        fields[38] = metadata.physical;
        fields[26] = u32::from(item.quantity);
        if item.quantity > 1 {
            fields[48] = u32::from(item.quantity);
        }
        let mut arguments = Vec::with_capacity(MESSAGE_BYTES - 12);
        for word in [5, 1, 1, 0x44000100, 0, 0x01f80000, 51] {
            arguments.extend(u32::to_le_bytes(word));
        }
        for word in fields {
            arguments.extend(word.to_le_bytes());
        }
        let mut tags = TAGS;
        if item.quantity > 1 { tags[48] = 4; }
        tags[50] = if self.source < self.destination { 2 } else { 4 };
        arguments.extend(tags);
        arguments.push(0);
        crate::script::private_call(function, &arguments)
    }
}

/// Each item definition's implement bits (`IntStats` ImplementType 0xFA0), from the server's
/// item metadata, for the records that are built without it at hand (the login profile).
static IMPLEMENTS: std::sync::OnceLock<std::collections::BTreeMap<u32, u32>> =
    std::sync::OnceLock::new();

/// Records the implement bits of the definitions that have them; the first call wins.
pub fn register_implements(entries: impl IntoIterator<Item = (u32, u32)>) {
    let _ = IMPLEMENTS.set(entries.into_iter().collect());
}

/// The implement bits of `definition`, 0 when it has none (or none were registered).
pub fn implement_of(definition: u32) -> u32 {
    IMPLEMENTS
        .get()
        .and_then(|map| map.get(&definition).copied())
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug)]
pub struct Metadata {
    /// Native IntStats ImplementType (0xfa0); required by skill hand masks.
    pub implement: Option<u32>,
    /// Legacy field name: DID quality 103 / inventory AppearanceFile, not the
    /// UI descriptor's texture icon. Zero denotes an absent appearance profile.
    pub icon: u32,
    pub physical: u32,
    pub equipment: Option<Equipment>,
}
#[derive(Clone, Copy, Debug)]
pub struct Equipment {
    pub valid: u32,
    pub preferred: u32,
    pub primary: u32,
    pub secondary: u32,
}
impl Equipment {
    pub fn hand_slot(self, item: &crate::inventory::Item) -> Option<u16> {
        let slot = match item.location {
            crate::inventory::Location::Equipped(slot) => u32::from(slot),
            _ => self.preferred,
        };
        // Explicit initial hand-equipment policy. Body armor needs VisualDesc
        // composition. Arbitrary parenting selectors are never accepted.
        if !matches!(slot, 0x40 | 0x80)
            || self.valid & slot == 0
            || u32::from(item.equipment_mask) & slot == 0
            || !matches!(self.primary, 1..=4)
            || (self.primary >= 3 && !crate::starter_gear::is_weapon(item.definition))
            || (slot == 0x80 && self.primary != 2)
            || item.quantity != 1
        {
            return None;
        }
        Some(slot as u16)
    }
}
impl Metadata {
    /// Read only the authoritative qualities prefix needed by bag acknowledgments.
    /// gmWeenieDesc::StreamPack 006037c0, CBaseQualities::UnPack 005192f0.
    /// Localized names with substitutions and string quality tables are unsupported.
    pub fn load(portal: &mut crate::dat::Dat, definition: u32) -> std::io::Result<Self> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported item metadata layout",
            )
        };
        if definition >> 24 != 0x47 {
            return Err(invalid());
        }
        let did = 0x81000000 | (definition & 0xffffff);
        let bytes = portal.read(did)?;
        let starter = crate::starter_gear::is_apparel(definition) || crate::starter_gear::is_weapon(definition);
        if starter && !crate::starter_gear::quality_matches(definition, &bytes) {
            return Err(invalid());
        }
        if bytes.len() > 65536 {
            return Err(invalid());
        }
        let mut pos = 0usize;
        let mut word = || -> std::io::Result<u32> {
            let value = bytes.get(pos..pos + 4).ok_or_else(invalid)?;
            pos += 4;
            Ok(u32::from_le_bytes(value.try_into().unwrap()))
        };
        let mask = word()?;
        if mask & 0x21 != 0x21 || mask & !0xfff != 0 || mask & 0x80 != 0 || word()? != did {
            return Err(invalid());
        }
        let desc = word()?;
        if desc & !0x7fffffff != 0 || desc & 0x10 == 0 {
            return Err(invalid());
        }
        for bit in [1, 0x2000000, 2] {
            if desc & bit != 0 {
                word()?;
            }
        }
        for bit in [4, 8] {
            if desc & bit != 0 {
                let table = word()?;
                word()?;
                if table == 0 || word()? != 0 {
                    return Err(invalid());
                }
            }
        }
        word()?; // Description icon selector; the move reply uses DID quality 103.
        for (bit, words) in [
            (0x20, 2),
            (0x40, 2),
            (0x100, 2),
            (0x200, 2),
            (0x20000000, 2),
            (0x40000000, 2),
            (0x4000000, 2),
            (0x80, 1),
            (0x800, 1),
            (0x1000, 1),
            (0x2000, 1),
            (0x200000, 1),
            (0x400000, 1),
            (0x8000, 1),
            (0x10000, 1),
            (0x20000, 1),
            (0x40000, 1),
            (0x80000, 1),
            (0x100000, 1),
            (0x8000000, 1),
            (0x10000000, 1),
        ] {
            if desc & bit != 0 {
                for _ in 0..words {
                    word()?;
                }
            }
        }
        // Reject currently unassigned description bits rather than offset guessing.
        const DESC_BITS: u32 = 1
            | 0x2000000
            | 2
            | 4
            | 8
            | 0x10
            | 0x20
            | 0x40
            | 0x100
            | 0x200
            | 0x20000000
            | 0x40000000
            | 0x4000000
            | 0x80
            | 0x800
            | 0x1000
            | 0x2000
            | 0x200000
            | 0x400000
            | 0x8000
            | 0x10000
            | 0x20000
            | 0x40000
            | 0x80000
            | 0x100000
            | 0x8000000
            | 0x10000000;
        // Native gmWeenieDesc::StreamPack 006037c0 has no payload for 0x800000, including the
        // ordinary weapon/armor defaults recovered from corpse captures.
        let allowed = DESC_BITS | 0x800000;
        if desc & !allowed != 0 {
            return Err(invalid());
        }
        let mut integers = std::collections::BTreeMap::new();
        for (bit, width) in [(2, 2), (4, 2), (8, 2), (0x10, 3)] {
            if mask & bit != 0 {
                let header = word()?;
                let count = header & 0xffffff;
                if header >> 24 > 22 || count > 512 {
                    return Err(invalid());
                }
                let mut keys = std::collections::BTreeSet::new();
                for _ in 0..count {
                    let key = word()?;
                    let value = word()?;
                    if !keys.insert(key) {
                        return Err(invalid());
                    }
                    if bit == 2 {
                        integers.insert(key, value);
                    }
                    for _ in 2..width {
                        word()?;
                    }
                }
            }
        }
        let header = word()?;
        let count = header & 0xffffff;
        if header >> 24 > 22 || count > 512 {
            return Err(invalid());
        }
        let mut physical = None;
        let mut icon = None;
        let mut keys = std::collections::BTreeSet::new();
        for _ in 0..count {
            let key = word()?;
            let value = word()?;
            if !keys.insert(key) {
                return Err(invalid());
            }
            if key == 1 {
                physical = Some(value);
            }
            if key == 0x103 {
                icon = Some(value);
            }
        }
        let physical = physical.ok_or_else(invalid)?;
        // Materials have a texture icon in their descriptor but no inventory
        // appearance animation. Preserve that absent optional DID as zero.
        let icon = icon.unwrap_or(0);
        if (icon != 0 && icon >> 24 != 0x20) || physical >> 24 != 0x47 {
            return Err(invalid());
        }
        if icon != 0 { portal.read(icon)?; }
        portal.read(physical)?;
        if starter && !crate::starter_gear::physical_matches(definition, &portal.read(physical)?) {
            return Err(invalid());
        }
        let equipment = (|| {
            Some(Equipment {
                valid: *integers.get(&0x113)?,
                preferred: *integers.get(&0x11a)?,
                // Body armor has slots but no hand parenting qualities.
                primary: integers.get(&0x114).copied().unwrap_or(0),
                secondary: integers.get(&0x115).copied().unwrap_or(0),
            })
        })();
        Ok(Self {
            icon,
            physical,
            equipment,
            implement: integers.get(&0xfa0).copied(),
        })
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;
    /// A split's answer is `ReorganizeContents_Done` for the stack the request named, with the
    /// new stack as `splitItemID` and `allItemUnitsTakenFlag` clear
    /// (`CInventorySystem::ReorganizeContents_Done` removes the item from its container only when
    /// all its units were taken, and adds `splitItemID` at `actualTargetSlot`).
    #[test]
    fn a_split_answer_names_the_new_stack_and_keeps_the_old_one() {
        let owner = 0x8110_0000_0000_0001u64;
        let (original, new_stack) = (0x8130_0000_0000_0005u64, 0x8130_0000_0000_0020u64);
        let mut split = crate::item_services::potion(new_stack, 13);
        split.quantity = 3;
        let metadata = Metadata { implement: None, icon: 0x0600_1234, physical: 0x0100_0005, equipment: None };
        let request = MoveRequest { owner, item: original, source: 4, destination: 13, quantity: 3 };
        let message = request.split_acknowledgment(&split, metadata).unwrap();
        let word = |i: usize| u32::from_le_bytes(message[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!(word(1), 0x820e0055);
        let field = |i: usize| word(10 + i);
        let id = |i: usize| u64::from(field(i)) | u64::from(field(i + 1)) << 32;
        assert_eq!((id(44), id(28)), (original, new_stack));
        assert_eq!(field(23), 0, "not all units were taken");
        assert_eq!((field(1), field(39), field(11), field(24), field(50)), (4, 4, 13, 13, 13));
        assert_eq!((field(26), field(40)), (3, 0));
        assert_eq!(id(16), owner);
        // A whole-item answer keeps the flag set and names the item itself.
        let mut whole = split.clone();
        whole.id = original;
        let message = MoveRequest { item: original, ..request.clone() }.acknowledgment(&whole, metadata).unwrap();
        let word = |i: usize| u32::from_le_bytes(message[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((word(10 + 23), word(10 + 28), word(10 + 44)), (1, original as u32, original as u32));
        assert!(request.split_acknowledgment(&whole, metadata).is_err());
    }
}
