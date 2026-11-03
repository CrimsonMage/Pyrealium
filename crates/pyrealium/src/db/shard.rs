//! The `shard` schema: saved characters, their items, ground items and world-wide values.
//! `Store` owns the in-memory state and builds a `Snapshot` of what changed; this module only
//! moves rows.
use super::{error, from_sql, invalid, to_sql};
use crate::{
    inventory::{Item, Location},
    movement::Placement,
    store::DroppedItem,
};
use postgres::{Client, GenericClient};
use serde_json::Value;
use std::io;

/// One saved character, as stored.
#[derive(Clone, Debug)]
pub struct CharacterRow {
    pub id: u64,
    pub account: String,
    pub name: String,
    pub deleted: bool,
    pub instance: u16,
    pub xp: u64,
    pub available_xp: u64,
    pub gold: u32,
    pub health: u32,
    pub vigor: u32,
    pub placement: Option<Placement>,
    pub item_high_water: u64,
    pub training_kit: bool,
    pub training_supplies: bool,
    pub starter_gear: bool,
    pub creation: Option<Value>,
    pub binding: Option<Value>,
    pub ui: Option<Vec<u8>>,
    pub gameplay_options: Option<Vec<u8>>,
    pub hotbar: Value,
    pub skills: Value,
    pub professions: Value,
    pub quests: Value,
    pub tradeskills: Option<String>,
    pub items: Vec<Item>,
}

/// Everything one save writes, in one transaction: the changed characters (with all their
/// items) and, when any changed, the full set of ground items.
#[derive(Default)]
pub struct Snapshot {
    pub characters: Vec<CharacterRow>,
    pub ground: Option<Vec<DroppedItem>>,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.characters.is_empty() && self.ground.is_none()
    }
}

fn int<T: TryFrom<i64>>(v: i64, what: &str) -> io::Result<T> {
    T::try_from(v).map_err(|_| invalid(format!("saved {what} out of range: {v}")))
}

pub fn save(client: &mut Client, snapshot: &Snapshot) -> io::Result<()> {
    if snapshot.is_empty() {
        return Ok(());
    }
    let mut tx = client.transaction().map_err(error)?;
    let upsert = tx
        .prepare(
            "INSERT INTO shard.characters (id, account, name, name_key, deleted, instance, xp,
                available_xp, gold, health, vigor, cell, x, y, z, heading, item_high_water,
                training_kit, training_supplies, starter_gear, creation, binding, ui,
                gameplay_options, hotbar, skills, professions, quests, tradeskills, saved_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,
                $22,$23,$24,$25,$26,$27,$28,$29, now())
             ON CONFLICT (id) DO UPDATE SET account=excluded.account, name=excluded.name,
                name_key=excluded.name_key, deleted=excluded.deleted, instance=excluded.instance,
                xp=excluded.xp, available_xp=excluded.available_xp, gold=excluded.gold,
                health=excluded.health, vigor=excluded.vigor, cell=excluded.cell, x=excluded.x,
                y=excluded.y, z=excluded.z, heading=excluded.heading,
                item_high_water=excluded.item_high_water, training_kit=excluded.training_kit,
                training_supplies=excluded.training_supplies, starter_gear=excluded.starter_gear,
                creation=excluded.creation, binding=excluded.binding, ui=excluded.ui,
                gameplay_options=excluded.gameplay_options, hotbar=excluded.hotbar,
                skills=excluded.skills, professions=excluded.professions, quests=excluded.quests,
                tradeskills=excluded.tradeskills, saved_at=now()",
        )
        .map_err(error)?;
    for c in &snapshot.characters {
        let p = c.placement;
        tx.execute(
            &upsert,
            &[
                &to_sql(c.id),
                &c.account,
                &c.name,
                &c.name.to_ascii_lowercase(),
                &c.deleted,
                &i32::from(c.instance),
                &to_sql(c.xp),
                &to_sql(c.available_xp),
                &i64::from(c.gold),
                &i64::from(c.health),
                &i64::from(c.vigor),
                &p.map(|p| i64::from(p.cell)),
                &p.map(|p| p.coordinates[0]),
                &p.map(|p| p.coordinates[1]),
                &p.map(|p| p.coordinates[2]),
                &p.map(|p| i16::from(p.heading)),
                &to_sql(c.item_high_water),
                &c.training_kit,
                &c.training_supplies,
                &c.starter_gear,
                &c.creation,
                &c.binding,
                &c.ui,
                &c.gameplay_options,
                &c.hotbar,
                &c.skills,
                &c.professions,
                &c.quests,
                &c.tradeskills,
            ],
        )
        .map_err(error)?;
    }
    // Delete every saved character's items before inserting any, so an item that moved between
    // two characters in this snapshot never collides with its old row.
    let ids: Vec<i64> = snapshot.characters.iter().map(|c| to_sql(c.id)).collect();
    tx.execute("DELETE FROM shard.items WHERE character_id = ANY($1)", &[&ids]).map_err(error)?;
    let insert = tx
        .prepare(
            "INSERT INTO shard.items (id, character_id, definition, visual, quantity, stack_limit,
                equipment_mask, equipped, slot, roll) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .map_err(error)?;
    for c in &snapshot.characters {
        for item in &c.items {
            insert_item(&mut tx, &insert, c.id, item)?;
        }
    }
    if let Some(ground) = &snapshot.ground {
        tx.execute("DELETE FROM shard.ground_items", &[]).map_err(error)?;
        for d in ground {
            let item = serde_json::to_value(&d.item).map_err(io::Error::other)?;
            let [x, y, z] = d.placement.coordinates;
            tx.execute(
                "INSERT INTO shard.ground_items (id, item, cell, x, y, z, heading)
                 VALUES ($1,$2,$3,$4,$5,$6,$7)",
                &[&to_sql(d.id()), &item, &i64::from(d.placement.cell), &x, &y, &z, &i16::from(d.placement.heading)],
            )
            .map_err(error)?;
        }
    }
    tx.commit().map_err(error)
}

fn insert_item(tx: &mut impl GenericClient, insert: &postgres::Statement, owner: u64, item: &Item) -> io::Result<()> {
    let (equipped, slot) = match item.location {
        Location::Bag(slot) => (false, slot),
        Location::Equipped(slot) => (true, slot),
    };
    let roll = item.roll.map(serde_json::to_value).transpose().map_err(io::Error::other)?;
    tx.execute(
        insert,
        &[
            &to_sql(item.id),
            &to_sql(owner),
            &i64::from(item.definition),
            &i64::from(item.visual),
            &i64::from(item.quantity),
            &i64::from(item.stack_limit),
            &i32::from(item.equipment_mask),
            &equipped,
            &i32::from(slot),
            &roll,
        ],
    )
    .map_err(error)?;
    Ok(())
}

pub fn load(client: &mut Client) -> io::Result<(Vec<CharacterRow>, Vec<DroppedItem>)> {
    let mut items: std::collections::BTreeMap<u64, Vec<Item>> = Default::default();
    for r in client
        .query(
            "SELECT id, character_id, definition, visual, quantity, stack_limit, equipment_mask,
                equipped, slot, roll FROM shard.items ORDER BY id",
            &[],
        )
        .map_err(error)?
    {
        let slot: u16 = int(i64::from(r.get::<_, i32>(8)), "item slot")?;
        let roll: Option<Value> = r.get(9);
        items.entry(from_sql(r.get(1))).or_default().push(Item {
            id: from_sql(r.get(0)),
            definition: int(r.get(2), "item definition")?,
            visual: int(r.get(3), "item visual")?,
            quantity: int(r.get(4), "item quantity")?,
            stack_limit: int(r.get(5), "item stack limit")?,
            equipment_mask: int(i64::from(r.get::<_, i32>(6)), "item equipment mask")?,
            location: if r.get(7) { Location::Equipped(slot) } else { Location::Bag(slot) },
            roll: roll.map(serde_json::from_value).transpose().map_err(|e| invalid(format!("saved item roll: {e}")))?,
        });
    }
    let mut characters = Vec::new();
    for r in client
        .query(
            "SELECT id, account, name, deleted, instance, xp, available_xp, gold, health, vigor,
                cell, x, y, z, heading, item_high_water, training_kit, training_supplies,
                starter_gear, creation, binding, ui, gameplay_options, hotbar, skills,
                professions, quests, tradeskills FROM shard.characters ORDER BY id",
            &[],
        )
        .map_err(error)?
    {
        let id = from_sql(r.get(0));
        let cell: Option<i64> = r.get(10);
        let placement = match (cell, r.get::<_, Option<f32>>(11), r.get::<_, Option<f32>>(12), r.get::<_, Option<f32>>(13), r.get::<_, Option<i16>>(14)) {
            (None, ..) => None,
            (Some(cell), Some(x), Some(y), Some(z), Some(heading)) => Some(Placement {
                cell: int(cell, "cell")?,
                coordinates: [x, y, z],
                heading: int(i64::from(heading), "heading")?,
            }),
            _ => return Err(invalid(format!("character {id} has a partial saved placement"))),
        };
        characters.push(CharacterRow {
            id,
            account: r.get(1),
            name: r.get(2),
            deleted: r.get(3),
            instance: int(i64::from(r.get::<_, i32>(4)), "instance")?,
            xp: from_sql(r.get(5)),
            available_xp: from_sql(r.get(6)),
            gold: int(r.get(7), "gold")?,
            health: int(r.get(8), "health")?,
            vigor: int(r.get(9), "vigor")?,
            placement,
            item_high_water: from_sql(r.get(15)),
            training_kit: r.get(16),
            training_supplies: r.get(17),
            starter_gear: r.get(18),
            creation: r.get(19),
            binding: r.get(20),
            ui: r.get(21),
            gameplay_options: r.get(22),
            hotbar: r.get(23),
            skills: r.get(24),
            professions: r.get(25),
            quests: r.get(26),
            tradeskills: r.get(27),
            items: items.remove(&id).unwrap_or_default(),
        });
    }
    if let Some((owner, _)) = items.into_iter().next() {
        return Err(invalid(format!("saved items belong to unknown character {owner}")));
    }
    let mut ground = Vec::new();
    for r in client
        .query("SELECT id, item, cell, x, y, z, heading FROM shard.ground_items ORDER BY id", &[])
        .map_err(error)?
    {
        let d = DroppedItem {
            item: serde_json::from_value(r.get(1)).map_err(|e| invalid(format!("saved ground item: {e}")))?,
            placement: Placement {
                cell: int(r.get(2), "ground cell")?,
                coordinates: [r.get(3), r.get(4), r.get(5)],
                heading: int(i64::from(r.get::<_, i16>(6)), "ground heading")?,
            },
        };
        if to_sql(d.id()) != r.get::<_, i64>(0) {
            return Err(invalid("ground item id does not match its item"));
        }
        ground.push(d);
    }
    Ok((characters, ground))
}

/// A world-wide value (`shard.world_state`), if set.
pub fn world_value(client: &mut impl GenericClient, key: &str) -> io::Result<Option<Value>> {
    Ok(client
        .query_opt("SELECT value FROM shard.world_state WHERE key = $1", &[&key])
        .map_err(error)?
        .map(|r| r.get(0)))
}

/// Sets `key` only if it is unset; returns the value now stored.
pub fn world_value_init(client: &mut impl GenericClient, key: &str, value: &Value) -> io::Result<Value> {
    client
        .execute("INSERT INTO shard.world_state (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING", &[&key, value])
        .map_err(error)?;
    world_value(client, key)?.ok_or_else(|| invalid(format!("world value {key} vanished")))
}

pub fn world_value_set(client: &mut impl GenericClient, key: &str, value: &Value) -> io::Result<()> {
    client
        .execute(
            "INSERT INTO shard.world_state (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            &[&key, value],
        )
        .map_err(error)?;
    Ok(())
}

/// Whether characters are saved under `account` (compared ignoring case): such a name cannot be
/// registered by someone else, e.g. a development-login account.
pub fn account_owns_characters(client: &mut impl GenericClient, account: &str) -> io::Result<bool> {
    Ok(client
        .query_opt(
            "SELECT 1 FROM shard.characters WHERE lower(account) = $1 LIMIT 1",
            &[&account.to_ascii_lowercase()],
        )
        .map_err(error)?
        .is_some())
}
