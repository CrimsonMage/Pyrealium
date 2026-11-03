//! Experimental empty-player HUD profile. Protocol layouts, not captured player state.
//! See docs/world-evidence.md for capture and matching-client evidence boundaries.
use crate::{script, world::WorldError};
use std::collections::BTreeMap;

pub const LOAD: u32 = 0x82120027;
/// SelectionInfo::StreamPack 006097c0 and retained ac2_npc_target.elog.
/// The native target panel is separate from the object's public qualities.
pub fn selection(target: u64, vitals: Option<[u32; 4]>) -> Result<Vec<u8>, WorldError> {
    let values = if let Some([health, maximum, vigor, max_vigor]) = vitals {
        if maximum == 0 || health > maximum || vigor > max_vigor {
            return Err(WorldError::InvalidValue);
        }
        vec![0x40000001, health, 0, maximum, vigor, 0, max_vigor]
    } else {
        vec![0]
    };
    let mut args = Vec::new();
    graph(&mut args, &[Object::native(0xab, &values)]);
    args.extend(script::identity(target));
    script::private_call(0x8212002c, &args)
}
/// A timed effect as the client's `EffectRecord` (package 0x14E, 26 slots) holds it: the time it
/// was cast as a double in slots 2-3, the duration as a float in slot 6, the spellcraft the
/// examination shows as a float in slot 8 (`EffectRecord` 0x11, read by the examination script and
/// shown through a float-to-int conversion), a reference to the `Effect` state's singleton in
/// slot 14 and the flags in slot 20 (0x2 timed, 0x400 pulsed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectView {
    pub eid: u32,
    /// The `Effect` state (DID).
    pub did: u32,
    pub time_cast: f64,
    pub duration: f32,
    pub power: f32,
    pub pulsed: bool,
}

/// The `EffectRecord` object for `e`, its `Effect` singleton being graph object `effect_ref`.
fn effect_record(e: &EffectView, effect_ref: u32) -> Result<Object, WorldError> {
    if !e.time_cast.is_finite() || !e.duration.is_finite() || e.duration <= 0.0 || e.did == 0 {
        return Err(WorldError::InvalidValue);
    }
    let mut fields = [(0u32, INT); 26];
    let bits = e.time_cast.to_bits();
    fields[2] = (bits as u32, 0xc3);
    fields[3] = ((bits >> 32) as u32, 0xcd);
    fields[6] = (e.duration.to_bits(), 3);
    fields[8] = (e.power.to_bits(), 3);
    fields[14] = (effect_ref, REF);
    fields[20] = (if e.pulsed { 0x402 } else { 0x2 }, INT);
    Ok(Object::interpreted(0x14e, &fields))
}

/// `WM_Effect` 0x17 (`CPlayer` 0x6B -> `CEffectSystem` 0x6 -> `EffectRegistry` 0x3A): a timed
/// effect joins the player's list. The values are the `EffectRecord`, then the effect id.
pub fn effect_added(e: &EffectView) -> Result<Vec<u8>, WorldError> {
    let mut args = Vec::new();
    graph(&mut args, &[effect_record(e, 2)?, Object::singleton(e.did)]);
    args.extend(script::integer(e.eid));
    script::private_call(0x82080017, &args)
}
/// The login's `EffectRegistry` (package 0x14F, 14 slots), empty: slot 4 the (AR) hash of effect
/// id to record and slot 11 the (AA) hash of order to effect id, which the registry's own add
/// (`WM_Effect` 0x17) fills, together with the indexes in slots 0 and 12 (the count per stat of
/// the `Effect` state's slot 30 list and a multi hash by its slot 27) that its remove and its
/// `UpdateEffect` notice go by. A login that listed effects itself left those two out, and an
/// effect it listed then left the strip without a notice: the server tells the client of the
/// effects still on the player with `WM_Effect` 0x17 after the login.
fn effect_registry() -> Vec<Object> {
    const NULL_REF: (u32, u8) = (NULL, REF);
    vec![
        Object::interpreted(
            0x14f,
            &[
                NULL_REF,
                NULL_REF,
                NULL_REF,
                (0, INT),
                (2, REF),
                (0, 0xc3),
                (0xbff00000, 0xcd), // Native -1.0 double sentinel.
                NULL_REF,
                NULL_REF,
                (0, INT),
                NULL_REF,
                (3, REF),
                NULL_REF,
                NULL_REF,
            ],
        ),
        Object::native(0x11, &[0]),
        Object::native(1, &[0]),
    ]
}
/// `WM_Effect` 0x16 (`CPlayer` 0x133 -> `CEffectSystem` 0x3 -> `EffectRegistry` 0x39): effect
/// `eid` leaves the player's list.
pub fn effect_removed(eid: u32) -> Result<Vec<u8>, WorldError> {
    script::private_call(0x82080016, &script::integer(eid))
}
pub fn death_notice(reason: &str) -> Result<Vec<u8>, WorldError> {
    script::private_call(0x8207002b, &script::literal(reason)?)
}
pub fn lock_player(locked: bool) -> Result<Vec<u8>, WorldError> {
    script::private_call(0x82070033, &script::integer(u32::from(locked)))
}
pub fn lifestone_bound(
    owner: u64,
    target: u64,
    point: crate::movement::Placement,
) -> Result<Vec<u8>, WorldError> {
    use_success(owner, target, point, None)
}
pub fn portal_used(
    owner: u64,
    target: u64,
    point: crate::movement::Placement,
    distance: f32,
) -> Result<Vec<u8>, WorldError> {
    if !distance.is_finite() || !(0.0..=4.0).contains(&distance) {
        return Err(WorldError::InvalidValue);
    }
    use_success(owner, target, point, Some(distance))
}
fn use_success(
    owner: u64,
    target: u64,
    point: crate::movement::Placement,
    portal_distance: Option<f32>,
) -> Result<Vec<u8>, WorldError> {
    if !point.valid() {
        return Err(WorldError::InvalidValue);
    }
    let mut fields = [0u32; 20];
    fields[0] = 2;
    for index in [1, 15] {
        fields[index] = u32::MAX;
    }
    fields[2] = target as u32;
    fields[3] = (target >> 32) as u32;
    fields[4] = 3;
    fields[5] = 0x1f;
    fields[6] = owner as u32;
    fields[7] = (owner >> 32) as u32;
    fields[8] = (-1f32).to_bits();
    fields[11] = 0x405;
    fields[17] = 0x13c;
    let mut tags = [
        1, 1, 0x84, 0xcd, 1, 4, 0x84, 0xcd, 3, 0x82, 0xcd, 2, 0x82, 0xcd, 2, 1, 2, 2, 2, 2,
    ];
    if let Some(distance) = portal_distance {
        fields[8] = distance.to_bits();
        fields[11] = 0;
        fields[12] = owner as u32;
        fields[13] = (owner >> 32) as u32;
        fields[17] = 0x3b4;
        fields[18] = 1;
        tags = [
            1, 1, 0x84, 0xcd, 1, 4, 0x84, 0xcd, 4, 0x82, 0xcd, 2, 0x84, 0xcd, 4, 1, 4, 2, 4, 4,
        ];
    }
    let root: Vec<_> = fields.into_iter().zip(tags).collect();
    let mut position = vec![point.cell];
    position.extend(
        point
            .coordinates
            .into_iter()
            .chain(point.orientation())
            .map(f32::to_bits),
    );
    let mut args = Vec::new();
    graph(
        &mut args,
        &[
            Object::interpreted(0x245, &root),
            Object::native(0x6a, &[0x4e1aec40, 0x25000014, 0]),
            Object::native(0x5a, &position),
        ],
    );
    script::private_call(0x82160021, &args)
}
const REF: u8 = 1;
const INT: u8 = 2;
const NULL: u32 = u32::MAX;

/// Check serialized interpreted-object sizes before enabling this fixed profile.
/// VTableSection::StreamPack 006e89d0 stores two nested word arrays, followed by
/// (size, checksum) package-info pairs. This checks layout, not script semantics.
pub fn validate_wlib(bytes: &[u8]) -> Result<(), WorldError> {
    validate_layouts(bytes, &[(0x1a4, 2), (0x233, 8), (0x14f, 14), (0x24a7, 2)])
}
pub fn validate_inventory_wlib(bytes: &[u8]) -> Result<(), WorldError> {
    validate_layouts(
        bytes,
        &[
            (0x1a2, 3),
            (0x1f9, 6),
            (0x1f8, 51),
            (0x245, 20),
            (0x1f6, 19),
            (0x1f5, 7),
        ],
    )
}
pub fn validate_skills_wlib(bytes: &[u8]) -> Result<(), WorldError> {
    validate_layouts(bytes, &[(0x231, 15)])
}
pub(crate) fn validate_layouts(bytes: &[u8], layouts: &[(usize, u32)]) -> Result<(), WorldError> {
    struct Reader<'a> {
        bytes: &'a [u8],
        pos: usize,
    }
    impl Reader<'_> {
        fn take(&mut self, size: usize) -> Result<&[u8], WorldError> {
            let end = self
                .pos
                .checked_add(size)
                .ok_or(WorldError::LimitExceeded)?;
            let value = self.bytes.get(self.pos..end).ok_or(WorldError::Truncated)?;
            self.pos = end;
            Ok(value)
        }
        fn word(&mut self) -> Result<u32, WorldError> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }
        fn nested(&mut self) -> Result<(), WorldError> {
            let count = self.word()?;
            if count > 32768 {
                return Err(WorldError::LimitExceeded);
            }
            for _ in 0..count {
                let count = self.word()? as usize;
                self.take(count.checked_mul(4).ok_or(WorldError::LimitExceeded)?)?;
            }
            Ok(())
        }
    }
    let mut reader = Reader { bytes, pos: 0 };
    if reader.word()? != 0x56000005 || reader.word()? != 0x1a015342 || reader.word()? != 15 {
        return Err(WorldError::InvalidValue);
    }
    let mut vtable = None;
    let mut sections = 0;
    while reader.pos < bytes.len() {
        sections += 1;
        if sections > 128 {
            return Err(WorldError::LimitExceeded);
        }
        let kind = reader.word()?;
        let size = reader.word()? as usize;
        let start = reader.pos;
        reader.take(size)?;
        if kind == 6 {
            if vtable.is_some() {
                return Err(WorldError::InvalidValue);
            }
            vtable = Some(&bytes[start..start + size]);
        }
    }
    let mut reader = Reader {
        bytes: vtable.ok_or(WorldError::InvalidValue)?,
        pos: 0,
    };
    reader.nested()?;
    reader.nested()?;
    let count = reader.word()? as usize;
    if count > 32768 {
        return Err(WorldError::LimitExceeded);
    }
    let info = reader.take(count.checked_mul(8).ok_or(WorldError::LimitExceeded)?)?;
    for &(package, size) in layouts {
        let entry = info
            .get(package * 8..package * 8 + 4)
            .ok_or(WorldError::InvalidValue)?;
        if entry != size.to_le_bytes() {
            return Err(WorldError::InvalidValue);
        }
    }
    Ok(())
}

struct Object {
    kind: u32,
    native: Vec<u32>,
    fields: Vec<(u32, u8)>,
    /// A package singleton by `WState` DID, packed as a reference entry of its own.
    singleton: Option<u32>,
}
impl Object {
    fn native(kind: u16, payload: &[u32]) -> Self {
        Self {
            kind: 0xffff0000 | u32::from(kind),
            native: payload.to_vec(),
            fields: Vec::new(),
            singleton: None,
        }
    }
    fn interpreted(package: u16, fields: &[(u32, u8)]) -> Self {
        Self {
            kind: u32::from(package) << 16,
            native: Vec::new(),
            fields: fields.to_vec(),
            singleton: None,
        }
    }
    fn singleton(did: u32) -> Self {
        Self {
            kind: 0,
            native: Vec::new(),
            fields: Vec::new(),
            singleton: Some(did),
        }
    }
}
fn word(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend(value.to_le_bytes());
}
fn graph(bytes: &mut Vec<u8>, objects: &[Object]) {
    word(bytes, 5); // PackBuffer package/reference graph.
    word(bytes, objects.len() as u32);
    for id in 1..=objects.len() {
        word(bytes, id as u32);
    }
    for object in objects {
        if let Some(did) = object.singleton {
            word(bytes, 0x20000100);
            word(bytes, did);
            continue;
        }
        word(bytes, 0x44000100); // Serialized reference flags.
        word(bytes, 0); // No external package-state dependency.
        word(bytes, object.kind);
        for value in &object.native {
            word(bytes, *value);
        }
        word(bytes, object.fields.len() as u32);
        for (value, _) in &object.fields {
            word(bytes, *value);
        }
        for (_, tag) in &object.fields {
            bytes.push(*tag);
        }
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }
}

pub fn hand_equipment(
    owner: u64,
    item: u64,
    slot: u16,
    other_hands: u16,
    metadata: crate::inventory_request::Equipment,
    equipped: bool,
) -> Result<Vec<u8>, WorldError> {
    hand_equipment_at(owner, item, slot, other_hands, metadata, equipped, 0)
}

pub fn hand_equipment_at(
    owner: u64,
    item: u64,
    slot: u16,
    other_hands: u16,
    metadata: crate::inventory_request::Equipment,
    equipped: bool,
    destination: u16,
) -> Result<Vec<u8>, WorldError> {
    // Body apparel uses the same native descriptor as held items. Recovered
    // headwear (0x01) and gloves (0x20) must not fail after profile validation.
    if !matches!(slot, 1 | 2 | 8 | 0x10 | 0x20 | 0x40 | 0x80) || other_hands & !0xc0 != 0 || other_hands & slot != 0 {
        return Err(WorldError::InvalidValue);
    }
    let mut fields = [0; 19];
    fields[1] = 2;
    fields[2] = 1;
    fields[3] = item as u32;
    fields[4] = (item >> 32) as u32;
    fields[5] = u32::from(other_hands);
    fields[7] = u32::from(slot);
    for index in [8, 10] {
        fields[index] = owner as u32;
        fields[index + 1] = (owner >> 32) as u32;
    }
    fields[12] = if equipped {
        if slot == 0x40 { 0xce } else if slot == 0x80 { 0x8e } else { 0xe }
    } else {
        0x9e
    };
    fields[14] = u32::from(slot | other_hands);
    fields[15] = u32::from(slot);
    let mut tags = [
        2, 1, 2, 0x84, 0xcd, 4, 2, 4, 0x84, 0xcd, 0x84, 0xcd, 2, 2, 2, 4, 0x82, 0xcd, 2,
    ];
    if !equipped {
        if destination >= crate::inventory::BAG_SLOTS {
            return Err(WorldError::InvalidValue);
        }
        fields[6] = u32::from(destination);
        fields[18] = u32::from(destination);
        if destination != 0 {
            tags[6] = 4;
            tags[18] = 4;
        }
        // Retained 820e0056 replies keep both equipment-slot fields tagged 4;
        // they are not constructor-zero fields even when removing equipment.
    }
    let root: Vec<_> = fields.into_iter().zip(tags).collect();
    let data = if equipped {
        vec![
            (0, 4),
            (metadata.primary, 4),
            (metadata.secondary, 4),
            (metadata.valid, 4),
            (0, 2),
            (metadata.preferred, 4),
            (0, 4),
        ]
    } else {
        vec![(0, 2); 7]
    };
    let mut arguments = Vec::new();
    graph(
        &mut arguments,
        &[
            Object::interpreted(0x1f6, &root),
            Object::interpreted(0x1f5, &data),
        ],
    );
    script::private_call(if equipped { 0x820e0051 } else { 0x820e0056 }, &arguments)
}

/// Deliberately empty development state: no grants, active effects, XP or gold.
/// This initializes presentation only. It is not an inventory or progression store.
pub fn empty_player() -> Result<Vec<u8>, WorldError> {
    player(None, None)
}
/// Retail 2017 login SkillData: state flags 1 known / 2 bought with credits /
/// 0x20 last-use stamp set, rank at field 10 and credits spent at field 12.
/// `children` fills SkillData field 10, which the client reads in one place: the skill is not
/// untrainable while it is nonzero (`SkillSystem` CanSkillBeUntrained -> Skill_ChildrenTrained,
/// 30033), so it counts the trained skills that need this one (`Book::trained_children`).
/// `used` is the world-clock second the skill was last used (the clock the time sync carries,
/// which the client's `Timer::cur_time` follows). The client's `SkillSystem` InqSkillTimeLeft
/// (wlib 7230, word 286891) reads it only under flag 0x20 (SkillInfo 0x18, word 281713) from
/// fields 0-1 (SkillInfo 0x1E, word 281692), and counts the skill's reset time (ActiveSkill
/// 0x16, WState field 34) down from it: the shortcut slot's recharge meter.
fn skill_object(id: u32, entry: &crate::skills::Trained, children: u32, used: Option<f64>) -> Object {
    let known = if entry.locked && entry.credits == 0 { 1 } else { 3 };
    let flags = if used.is_some() { known | 0x20 } else { known };
    let stamp = used.unwrap_or(-1.0).to_bits();
    Object::interpreted(
        0x231,
        &[
            (stamp as u32, 0xc3),
            ((stamp >> 32) as u32, 0xcd),
            (0, INT),
            (0, 0x83),
            (0, 0xcd),
            (entry.xp as u32, 0x82),
            ((entry.xp >> 32) as u32, 0xcd),
            (flags, INT),
            (0, 0xc3),
            (0xbff00000, 0xcd),
            (children, INT),
            (0, INT),
            (u32::from(entry.credits), INT),
            (0, INT),
            (id, INT),
        ],
    )
}
/// Retail skill hashes are sent at bucket index 2 (47 buckets) in bucket order.
const SKILL_BUCKETS: u32 = 47;
/// The skill's SkillData (`WM_Skill` 0xD); `used` as for [`skill_object`].
pub fn skill_update(id: u32, skills: &crate::skills::Book, used: Option<f64>) -> Result<Vec<u8>, WorldError> {
    let entry = skills.get(id).ok_or(WorldError::InvalidValue)?;
    if used.is_some_and(|t| !t.is_finite()) {
        return Err(WorldError::InvalidValue);
    }
    let mut args = Vec::new();
    graph(&mut args, &[skill_object(id, entry, skills.trained_children(id), used)]);
    script::private_call(0x8214000d, &args)
}
pub fn skill_remove(id: u32) -> Result<Vec<u8>, WorldError> {
    script::private_call(0x82140014, &script::integer(id))
}
pub fn player(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
) -> Result<Vec<u8>, WorldError> {
    player_inventory(settings, hotbar, None)
}
pub fn player_inventory(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
    inventory: Option<&crate::inventory::Inventory>,
) -> Result<Vec<u8>, WorldError> {
    player_state(settings, hotbar, inventory, None)
}
pub fn player_state(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
    inventory: Option<&crate::inventory::Inventory>,
    progression: Option<&crate::progression::Progression>,
) -> Result<Vec<u8>, WorldError> {
    player_all(settings, hotbar, inventory, progression, None)
}
pub fn player_all(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
    inventory: Option<&crate::inventory::Inventory>,
    progression: Option<&crate::progression::Progression>,
    skills: Option<&crate::skills::Book>,
) -> Result<Vec<u8>, WorldError> {
    player_all_quests(settings, hotbar, inventory, progression, skills, None)
}
pub fn player_all_quests(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
    inventory: Option<&crate::inventory::Inventory>,
    progression: Option<&crate::progression::Progression>,
    skills: Option<&crate::skills::Book>,
    quests: Option<&crate::quests::Book>,
) -> Result<Vec<u8>, WorldError> {
    player_all_quests_options(settings, hotbar, inventory, progression, skills, quests, None)
}
pub fn player_all_quests_options(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
    inventory: Option<&crate::inventory::Inventory>,
    progression: Option<&crate::progression::Progression>,
    skills: Option<&crate::skills::Book>,
    quests: Option<&crate::quests::Book>,
    gameplay: Option<&crate::preferences::GameplayOptions>,
) -> Result<Vec<u8>, WorldError> {
    player_login(settings, hotbar, inventory, progression, skills, quests, gameplay)
}
/// The player's login profile.
pub fn player_login(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
    inventory: Option<&crate::inventory::Inventory>,
    progression: Option<&crate::progression::Progression>,
    skills: Option<&crate::skills::Book>,
    quests: Option<&crate::quests::Book>,
    gameplay: Option<&crate::preferences::GameplayOptions>,
) -> Result<Vec<u8>, WorldError> {
    player_login_with_chat(settings, hotbar, inventory, progression, skills, quests, gameplay, None)
}
/// [`player_login`] with the chat tables the player uploaded (`preferences::ChatOptions`).
#[allow(clippy::too_many_arguments)]
pub fn player_login_with_chat(
    settings: Option<&crate::preferences::UiSettings>,
    hotbar: Option<&crate::preferences::Hotbar>,
    inventory: Option<&crate::inventory::Inventory>,
    progression: Option<&crate::progression::Progression>,
    skills: Option<&crate::skills::Book>,
    quests: Option<&crate::quests::Book>,
    gameplay: Option<&crate::preferences::GameplayOptions>,
    chat: Option<&crate::preferences::ChatOptions>,
) -> Result<Vec<u8>, WorldError> {
    let mut args = script::integer(progression.map_or(0, |state| state.gold));
    graph(
        &mut args,
        &[
            Object::interpreted(0x1a4, &[(0, INT), (2, REF)]),
            Object::native(0x11, &[0]), // Story chapter hash.
        ],
    );
    if let Some(book) = quests {
        args.extend(book.list_graph()?);
    } else {
        graph(&mut args, &[Object::native(0xa1, &[0])]);
    }
    // GameplayOptionsProfile::StreamPack 006000b0 starts with a u64 mask.
    // A zero mask retains native Clear/default values; no captured settings.
    // A character that never uploaded options gets the page's defaults (a zero radar mask hides
    // every blip from the radar and the selection keys).
    let new_character = crate::preferences::GameplayOptions::new_character();
    let options = crate::preferences::options_words_with_chat(
        settings,
        hotbar,
        gameplay.or(Some(&new_character)),
        chat,
    )?;
    graph(&mut args, &[Object::native(0x9b, &options)]);
    skill_graph(&mut args, skills, progression, &BTreeMap::new());
    graph(&mut args, &effect_registry());
    args.extend(script::integer(0)); // Empty effect/inventory sequence.
    inventory_map(&mut args, inventory, true);
    inventory_map(&mut args, inventory, false);
    // Four empty development bags. Capacity is policy, not a recovered grant.
    graph(
        &mut args,
        &[
            Object::native(0x65, &[4, 2, 3, 4, 5]),
            Object::interpreted(0x24a7, &[(42, INT), (bag_count(inventory, 0, 42), INT)]),
            Object::interpreted(0x24a7, &[(12, INT), (bag_count(inventory, 42, 54), INT)]),
            Object::interpreted(0x24a7, &[(12, INT), (bag_count(inventory, 54, 66), INT)]),
            Object::interpreted(0x24a7, &[(12, INT), (bag_count(inventory, 66, 78), INT)]),
        ],
    );
    graph(&mut args, &[Object::native(0x3b, &[0])]);
    let mut bag_items = Vec::new();
    if let Some(inventory) = inventory {
        for item in inventory.items() {
            if let crate::inventory::Location::Bag(slot) = item.location {
                bag_items.resize(bag_items.len().max(slot as usize + 1), 0u64);
                bag_items[slot as usize] = item.id;
            }
        }
    }
    let mut list = vec![bag_items.len() as u32];
    for id in bag_items {
        list.extend([id as u32, (id >> 32) as u32]);
    }
    graph(&mut args, &[Object::native(0x3b, &list)]);
    args.extend(script::integer(1));
    args.extend(script::integer(0));
    script::private_call(LOAD, &args)
}

fn bag_count(inventory: Option<&crate::inventory::Inventory>, start: u16, end: u16) -> u32 {
    inventory.map(|inventory|inventory.items().filter(|item|matches!(item.location,crate::inventory::Location::Bag(slot) if slot>=start && slot<end)).count() as u32).unwrap_or(0)
}
fn inventory_map(
    out: &mut Vec<u8>,
    inventory: Option<&crate::inventory::Inventory>,
    equipment: bool,
) {
    use crate::inventory::Location;
    let mut items: Vec<_> = inventory
        .into_iter()
        .flat_map(|inventory| inventory.items())
        // Both hashes hold the worn items only: `InventorySystem::AddItemToInventory` (the player's
        // script, called by `EquipItem_Done`, `UnEquipItem_Done` and `Client_UnApplyToInventory`)
        // adds to them when an item is worn and `RemoveItemFromInventory` takes it out, and the
        // script's `ReorganizeContents_Done` / `MoveItem_Done` re-add an item they removed from a
        // container only when `Inventory::f6` (this LRHash) does not know it
        // (`InventorySystem::f29`), so a hash listing the pack's items makes the script drop an
        // item from its list when it is reorganized.
        .filter(|item| matches!(item.location, Location::Equipped(_)))
        .collect();
    // LRHash uses the full u64 remainder; the equipment ARHash uses u32 slots.
    items.sort_unstable_by_key(|item| {
        let key = if equipment {
            match item.location {
                Location::Equipped(slot) => u64::from(slot),
                _ => 0,
            }
        } else {
            item.id
        };
        (key % 11, key)
    });
    let mut payload = vec![items.len() as u32];
    for (index, item) in items.iter().enumerate() {
        if equipment {
            let Location::Equipped(slot) = item.location else {
                unreachable!()
            };
            payload.push(u32::from(slot));
        } else {
            payload.extend([item.id as u32, (item.id >> 32) as u32]);
        }
        payload.push(2 + index as u32 * 5);
    }
    let mut objects = vec![Object::native(
        if equipment { 0x11 } else { 0x40 },
        &payload,
    )];
    for item in items {
        let root = objects.len() as u32 + 1;
        let slot = match item.location {
            Location::Equipped(slot) => u32::from(slot),
            _ => 0,
        };
        objects.push(Object::interpreted(
            0x1f9,
            &[
                (root + 1, REF),
                (u32::from(item.equipment_mask), INT),
                (slot, INT),
                // The item's implement: `EquipItem_Done` records it from the item's weenie
                // (`GetInternalWeenieObject`), and the skills that want a weapon read it from
                // this record (`IsSkillUsableBasedOnInventory`); a login record without it left
                // every such skill unusable until the weapon was worn again.
                (crate::inventory_request::implement_of(item.definition), INT),
                (item.id as u32, 0x82),
                ((item.id >> 32) as u32, 0xcd),
            ],
        ));
        objects.push(Object::interpreted(
            0x1a2,
            &[(root + 2, REF), (root + 3, REF), (root + 4, REF)],
        ));
        objects.push(Object::native(0x70, &[1f32.to_bits(); 3]));
        objects.push(Object::native(0xf, &[0])); // No unverified appearance overrides.
        objects.push(Object::native(0x71, &[2, item.visual]));
    }
    graph(out, &objects);
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The SkillData fields (values, then tags) of a one-object `WM_Skill` 0xD update.
    fn skill_fields(wire: &[u8]) -> (Vec<u32>, Vec<u8>) {
        let words: Vec<u32> = wire
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let at = words.iter().position(|&w| w == 0x0231_0000).unwrap();
        assert_eq!(words[at + 1], 15);
        let start = (at + 2) * 4;
        let values = words[at + 2..at + 17].to_vec();
        (values, wire[start + 60..start + 75].to_vec())
    }
    #[test]
    fn skill_update_stamps_the_last_use_for_the_client_reset_meter() {
        use crate::progression::{Command, Progression};
        let state = Progression::default().apply(&Command::AwardXp(1000)).unwrap();
        let book = crate::skills::Book::default()
            .with_starter_skills(&state, 1, 0x1000)
            .unwrap()
            .apply(crate::skills::Command::Train(0x59), &state, 1)
            .unwrap()
            .apply(crate::skills::Command::Train(0x55), &state, 1)
            .unwrap();
        // Never used: no 0x20 flag and the -1.0 stamp SkillInfo's constructor writes.
        let (values, tags) = skill_fields(&skill_update(0x55, &book, None).unwrap());
        assert_eq!((values[0], values[1]), (0, 0xbff0_0000));
        assert_eq!((tags[0], tags[1]), (0xc3, 0xcd));
        assert_eq!(values[7] & 0x20, 0);
        // Used at world second 1234.5: fields 0-1 hold the time double (low word first) and
        // flag 0x20 says it is set; the rest of the record is unchanged.
        let used = 1234.5f64.to_bits();
        let (stamped, stamped_tags) = skill_fields(&skill_update(0x55, &book, Some(1234.5)).unwrap());
        assert_eq!((stamped[0], stamped[1]), (used as u32, (used >> 32) as u32));
        assert_eq!(stamped_tags, tags);
        assert_eq!(stamped[7], values[7] | 0x20);
        assert_eq!(stamped[2..7], values[2..7]);
        assert_eq!(stamped[8..], values[8..]);
        assert!(skill_update(0x55, &book, Some(f64::NAN)).is_err());
        // The repository resend keeps each skill's stamp.
        let progression = state;
        let used_map = BTreeMap::from([(0x55, 1234.5)]);
        let all = skills_state(&book, &progression, &used_map).unwrap();
        let unstamped = skills_state(&book, &progression, &BTreeMap::new()).unwrap();
        assert_eq!(all.len(), unstamped.len());
        let words = |b: &[u8]| -> Vec<u32> {
            b.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect()
        };
        assert!(words(&all).windows(2).any(|w| w == [used as u32, (used >> 32) as u32]));
        assert!(!words(&unstamped).windows(2).any(|w| w == [used as u32, (used >> 32) as u32]));
    }
    #[test]
    fn layout_gate_rejects_changed_sizes_truncation_and_unbounded_counts() {
        let mut section = Vec::new();
        for value in [1, 0, 1, 0, 0x24a8] {
            word(&mut section, value);
        }
        section.resize(section.len() + 0x24a8 * 8, 0);
        for (package, size) in [(0x1a4, 2u32), (0x233, 8), (0x14f, 14), (0x24a7, 2)] {
            section[20 + package * 8..24 + package * 8].copy_from_slice(&size.to_le_bytes());
        }
        let mut wlib = Vec::new();
        for value in [0x56000005, 0x1a015342, 15, 6, section.len() as u32] {
            word(&mut wlib, value);
        }
        wlib.extend(&section);
        validate_wlib(&wlib).unwrap();
        for length in [0, 1, 4, 8, 11, 12, 16, 19, 20, 24, wlib.len() - 1] {
            assert!(validate_wlib(&wlib[..length]).is_err());
        }
        for (offset, value) in [
            (8, 16),
            (16, u32::MAX),
            (20, 32769),
            (28, 32769),
            (36, u32::MAX),
            (40 + 0x14f * 8, 13),
        ] {
            let mut invalid = wlib.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(validate_wlib(&invalid).is_err());
        }
        let mut duplicate = wlib.clone();
        duplicate.extend(&wlib[12..]);
        assert!(validate_wlib(&duplicate).is_err());
    }
    #[test]
    fn profile_has_fourteen_arguments_and_closed_reference_graphs() {
        let bytes = empty_player().unwrap();
        let words: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(&words[..2], &[0x10066, LOAD]);
        assert_eq!(words[2] as usize, bytes.len() - 12);
        let mut p = 3;
        let mut arguments = 0;
        let mut packages = Vec::new();
        while p < words.len() {
            arguments += 1;
            let tag = words[p];
            p += 1;
            if tag == 1 {
                p += 1;
                continue;
            }
            assert_eq!(tag, 5);
            let count = words[p];
            p += 1;
            assert!((1..=5).contains(&count));
            assert_eq!(
                &words[p..p + count as usize],
                &(1..=count).collect::<Vec<_>>()
            );
            p += count as usize;
            for _ in 0..count {
                assert_eq!(words[p], 0x44000100);
                assert_eq!(words[p + 1], 0);
                let kind = words[p + 2];
                p += 3;
                packages.push(kind);
                if kind & 0xffff != 0 {
                    if kind & 0xffff == 0x9b {
                        // A character that never uploaded options gets the page's defaults:
                        // the mask, then the shortcut height, damage range, radar mask and
                        // bit field.
                        assert_eq!(&words[p..p + 2], &[0x10a8, 0]);
                        assert_eq!(words[p + 2], 1, "shortcut height");
                        assert_eq!(words[p + 3], (-1.0_f32).to_bits(), "damage range");
                        assert_eq!(words[p + 4], 0x1F, "radar mask");
                        assert_eq!(words[p + 5], 0x4 | 0x20 | 0x1000, "bit field");
                        p += 6;
                    } else {
                        let n = words[p] & 0x00ff_ffff;
                        p += 1;
                        let stride = if kind & 0xffff == 0x65 {
                            1
                        } else {
                            assert_eq!(n, 0);
                            0
                        };
                        for reference in &words[p..p + n as usize * stride] {
                            assert!((1..=count).contains(reference));
                        }
                        p += n as usize * stride;
                    }
                }
                let n = words[p] as usize;
                p += 1;
                let values = &words[p..p + n];
                p += n;
                let tags = &bytes[p * 4..p * 4 + n];
                for (&value, &tag) in values.iter().zip(tags) {
                    if tag == REF {
                        assert!(value == NULL || (1..=count).contains(&value));
                    }
                }
                p += n.div_ceil(4);
            }
        }
        assert_eq!(arguments, 14);
        assert_eq!(p, words.len());
        assert!(packages.contains(&0x02330000));
        assert!(bytes.len() < 2048);
    }
}

fn skill_graph(
    args: &mut Vec<u8>,
    skills: Option<&crate::skills::Book>,
    progression: Option<&crate::progression::Progression>,
    used: &BTreeMap<u32, f64>,
) {
    let mut trained = skills
        .into_iter()
        .flat_map(|book| book.entries())
        .collect::<Vec<_>>();
    trained.sort_unstable_by_key(|(id, _)| (id % SKILL_BUCKETS, *id));
    let mut skill_map = vec![0x02000000 | trained.len() as u32];
    for (index, (id, _)) in trained.iter().enumerate() {
        skill_map.extend([*id, 5 + index as u32]);
    }
    let mut skill_objects =
        vec![
            Object::interpreted(
                0x233,
                &[
                    (
                        progression.map_or(0, |state| {
                            u32::from(skills.map_or(state.level().credits, |book| {
                                book.available_credits(state)
                            }))
                        }),
                        INT,
                    ),
                    (0, 0x82),
                    (0, 0xcd),
                    (0, INT),
                    (2, REF),
                    (skills.map_or(0, |book| book.untraining_id()), INT),
                    (3, REF),
                    (4, REF),
                ],
            ),
            Object::native(1, &[0]),
            Object::native(1, &[0]),
            Object::native(0x11, &skill_map),
        ];
    for (id, entry) in trained {
        let children = skills.map_or(0, |book| book.trained_children(id));
        skill_objects.push(skill_object(id, entry, children, used.get(&id).copied().filter(|t| t.is_finite())));
    }
    graph(args, &skill_objects);
}
/// The whole skill repository (`WM_Skill` 0xE); `used` holds each skill's last-use stamp.
pub fn skills_state(
    skills: &crate::skills::Book,
    progression: &crate::progression::Progression,
    used: &BTreeMap<u32, f64>,
) -> Result<Vec<u8>, WorldError> {
    let mut args=Vec::new();
    skill_graph(&mut args, Some(skills), Some(progression), used);
    script::private_call(0x8214000e, &args)
}
