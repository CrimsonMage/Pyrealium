//! The game tables: saved characters, their items, ground items and world-wide values.
//! `Store` owns the in-memory state and builds a `Snapshot` of what changed; this module only
//! moves rows.
use super::{Connection, error, from_sql, invalid, json_text, json_value, to_sql};
use crate::{
    inventory::{Item, Location},
    movement::Placement,
    store::DroppedItem,
};
use rusqlite::{OptionalExtension, Row, params};
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
    pub chat_options: Option<Vec<u8>>,
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
fn opt_json(v: &Option<Value>) -> io::Result<Option<String>> {
    v.as_ref().map(json_text).transpose()
}
fn get<T: rusqlite::types::FromSql>(r: &Row, i: usize) -> io::Result<T> {
    r.get(i).map_err(error)
}
fn get_json(r: &Row, i: usize, what: &str) -> io::Result<Value> {
    json_value(&get::<String>(r, i)?, what)
}
fn get_opt_json(r: &Row, i: usize, what: &str) -> io::Result<Option<Value>> {
    get::<Option<String>>(r, i)?.map(|t| json_value(&t, what)).transpose()
}

pub fn save(connection: &mut Connection, snapshot: &Snapshot) -> io::Result<()> {
    if snapshot.is_empty() {
        return Ok(());
    }
    let tx = connection.transaction().map_err(error)?;
    {
        let mut upsert = tx
            .prepare(
                "INSERT INTO characters (id, account, name, name_key, deleted, instance, xp,
                    available_xp, gold, health, vigor, cell, x, y, z, heading, item_high_water,
                    training_kit, training_supplies, starter_gear, creation, binding, ui,
                    gameplay_options, hotbar, skills, professions, quests, tradeskills, chat_options, saved_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,
                    ?22,?23,?24,?25,?26,?27,?28,?29,?30, unixepoch())
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
                    tradeskills=excluded.tradeskills, chat_options=excluded.chat_options,
                    saved_at=unixepoch()",
            )
            .map_err(error)?;
        for c in &snapshot.characters {
            let p = c.placement;
            upsert
                .execute(params![
                    to_sql(c.id),
                    c.account,
                    c.name,
                    c.name.to_ascii_lowercase(),
                    c.deleted,
                    i64::from(c.instance),
                    to_sql(c.xp),
                    to_sql(c.available_xp),
                    i64::from(c.gold),
                    i64::from(c.health),
                    i64::from(c.vigor),
                    p.map(|p| i64::from(p.cell)),
                    p.map(|p| f64::from(p.coordinates[0])),
                    p.map(|p| f64::from(p.coordinates[1])),
                    p.map(|p| f64::from(p.coordinates[2])),
                    p.map(|p| i64::from(p.heading)),
                    to_sql(c.item_high_water),
                    c.training_kit,
                    c.training_supplies,
                    c.starter_gear,
                    opt_json(&c.creation)?,
                    opt_json(&c.binding)?,
                    c.ui,
                    c.gameplay_options,
                    json_text(&c.hotbar)?,
                    json_text(&c.skills)?,
                    json_text(&c.professions)?,
                    json_text(&c.quests)?,
                    c.tradeskills,
                    c.chat_options,
                ])
                .map_err(error)?;
        }
        // Delete every saved character's items before inserting any, so an item that moved
        // between two characters in this snapshot never collides with its old row.
        let mut delete = tx.prepare("DELETE FROM items WHERE character_id = ?1").map_err(error)?;
        for c in &snapshot.characters {
            delete.execute([to_sql(c.id)]).map_err(error)?;
        }
        let mut insert = tx
            .prepare(
                "INSERT INTO items (id, character_id, definition, visual, quantity, stack_limit,
                    equipment_mask, equipped, slot, roll) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )
            .map_err(error)?;
        for c in &snapshot.characters {
            for item in &c.items {
                insert_item(&mut insert, c.id, item)?;
            }
        }
        if let Some(ground) = &snapshot.ground {
            tx.execute("DELETE FROM ground_items", []).map_err(error)?;
            let mut insert = tx
                .prepare(
                    "INSERT INTO ground_items (id, item, cell, x, y, z, heading)
                     VALUES (?1,?2,?3,?4,?5,?6,?7)",
                )
                .map_err(error)?;
            for d in ground {
                let item = serde_json::to_string(&d.item).map_err(io::Error::other)?;
                let [x, y, z] = d.placement.coordinates.map(f64::from);
                insert
                    .execute(params![
                        to_sql(d.id()),
                        item,
                        i64::from(d.placement.cell),
                        x,
                        y,
                        z,
                        i64::from(d.placement.heading)
                    ])
                    .map_err(error)?;
            }
        }
    }
    tx.commit().map_err(error)
}

fn insert_item(insert: &mut rusqlite::Statement, owner: u64, item: &Item) -> io::Result<()> {
    let (equipped, slot) = match item.location {
        Location::Bag(slot) => (false, slot),
        Location::Equipped(slot) => (true, slot),
    };
    let roll = item.roll.map(|r| serde_json::to_string(&r)).transpose().map_err(io::Error::other)?;
    insert
        .execute(params![
            to_sql(item.id),
            to_sql(owner),
            i64::from(item.definition),
            i64::from(item.visual),
            i64::from(item.quantity),
            i64::from(item.stack_limit),
            i64::from(item.equipment_mask),
            equipped,
            i64::from(slot),
            roll,
        ])
        .map_err(error)?;
    Ok(())
}

/// Runs `query` and converts each row with `f`.
fn rows<T>(connection: &Connection, query: &str, mut f: impl FnMut(&Row) -> io::Result<T>) -> io::Result<Vec<T>> {
    let mut statement = connection.prepare(query).map_err(error)?;
    let mut rows = statement.query([]).map_err(error)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(error)? {
        out.push(f(r)?);
    }
    Ok(out)
}

pub fn load(connection: &mut Connection) -> io::Result<(Vec<CharacterRow>, Vec<DroppedItem>)> {
    let mut items: std::collections::BTreeMap<u64, Vec<Item>> = Default::default();
    for (owner, item) in rows(
        connection,
        "SELECT id, character_id, definition, visual, quantity, stack_limit, equipment_mask,
            equipped, slot, roll FROM items ORDER BY id",
        |r| {
            let slot: u16 = int(get(r, 8)?, "item slot")?;
            let roll: Option<String> = get(r, 9)?;
            Ok((
                from_sql(get(r, 1)?),
                Item {
                    id: from_sql(get(r, 0)?),
                    definition: int(get(r, 2)?, "item definition")?,
                    visual: int(get(r, 3)?, "item visual")?,
                    quantity: int(get(r, 4)?, "item quantity")?,
                    stack_limit: int(get(r, 5)?, "item stack limit")?,
                    equipment_mask: int(get(r, 6)?, "item equipment mask")?,
                    location: if get(r, 7)? { Location::Equipped(slot) } else { Location::Bag(slot) },
                    roll: roll
                        .map(|t| serde_json::from_str(&t))
                        .transpose()
                        .map_err(|e| invalid(format!("saved item roll: {e}")))?,
                },
            ))
        },
    )? {
        items.entry(owner).or_default().push(item);
    }
    let mut characters = rows(
        connection,
        "SELECT id, account, name, deleted, instance, xp, available_xp, gold, health, vigor,
            cell, x, y, z, heading, item_high_water, training_kit, training_supplies,
            starter_gear, creation, binding, ui, gameplay_options, hotbar, skills,
            professions, quests, tradeskills, chat_options FROM characters ORDER BY id",
        |r| {
            let id = from_sql(get(r, 0)?);
            let cell: Option<i64> = get(r, 10)?;
            let coordinate = |i| get::<Option<f64>>(r, i).map(|v| v.map(|v| v as f32));
            let placement = match (cell, coordinate(11)?, coordinate(12)?, coordinate(13)?, get::<Option<i64>>(r, 14)?) {
                (None, ..) => None,
                (Some(cell), Some(x), Some(y), Some(z), Some(heading)) => Some(Placement {
                    cell: int(cell, "cell")?,
                    coordinates: [x, y, z],
                    heading: int(heading, "heading")?,
                }),
                _ => return Err(invalid(format!("character {id} has a partial saved placement"))),
            };
            Ok(CharacterRow {
                id,
                account: get(r, 1)?,
                name: get(r, 2)?,
                deleted: get(r, 3)?,
                instance: int(get(r, 4)?, "instance")?,
                xp: from_sql(get(r, 5)?),
                available_xp: from_sql(get(r, 6)?),
                gold: int(get(r, 7)?, "gold")?,
                health: int(get(r, 8)?, "health")?,
                vigor: int(get(r, 9)?, "vigor")?,
                placement,
                item_high_water: from_sql(get(r, 15)?),
                training_kit: get(r, 16)?,
                training_supplies: get(r, 17)?,
                starter_gear: get(r, 18)?,
                creation: get_opt_json(r, 19, "creation")?,
                binding: get_opt_json(r, 20, "binding")?,
                ui: get(r, 21)?,
                gameplay_options: get(r, 22)?,
                hotbar: get_json(r, 23, "hotbar")?,
                skills: get_json(r, 24, "skills")?,
                professions: get_json(r, 25, "professions")?,
                quests: get_json(r, 26, "quests")?,
                tradeskills: get(r, 27)?,
                chat_options: get(r, 28)?,
                items: Vec::new(),
            })
        },
    )?;
    for c in &mut characters {
        c.items = items.remove(&c.id).unwrap_or_default();
    }
    if let Some((owner, _)) = items.into_iter().next() {
        return Err(invalid(format!("saved items belong to unknown character {owner}")));
    }
    let ground = rows(connection, "SELECT id, item, cell, x, y, z, heading FROM ground_items ORDER BY id", |r| {
        let d = DroppedItem {
            item: serde_json::from_str(&get::<String>(r, 1)?).map_err(|e| invalid(format!("saved ground item: {e}")))?,
            placement: Placement {
                cell: int(get(r, 2)?, "ground cell")?,
                coordinates: [get::<f64>(r, 3)? as f32, get::<f64>(r, 4)? as f32, get::<f64>(r, 5)? as f32],
                heading: int(get(r, 6)?, "ground heading")?,
            },
        };
        if to_sql(d.id()) != get::<i64>(r, 0)? {
            return Err(invalid("ground item id does not match its item"));
        }
        Ok(d)
    })?;
    Ok((characters, ground))
}

/// A world-wide value (`world_state`), if set.
pub fn world_value(connection: &Connection, key: &str) -> io::Result<Option<Value>> {
    connection
        .query_row("SELECT value FROM world_state WHERE key = ?1", [key], |r| r.get::<_, String>(0))
        .optional()
        .map_err(error)?
        .map(|t| json_value(&t, key))
        .transpose()
}

/// Sets `key` only if it is unset; returns the value now stored.
pub fn world_value_init(connection: &Connection, key: &str, value: &Value) -> io::Result<Value> {
    connection
        .execute(
            "INSERT INTO world_state (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING",
            params![key, json_text(value)?],
        )
        .map_err(error)?;
    world_value(connection, key)?.ok_or_else(|| invalid(format!("world value {key} vanished")))
}

pub fn world_value_set(connection: &Connection, key: &str, value: &Value) -> io::Result<()> {
    connection
        .execute(
            "INSERT INTO world_state (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, json_text(value)?],
        )
        .map_err(error)?;
    Ok(())
}

/// Whether characters are saved under `account` (compared ignoring case): such a name cannot be
/// registered by someone else, e.g. a development-login account.
pub fn account_owns_characters(connection: &Connection, account: &str) -> io::Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM characters WHERE lower(account) = ?1 LIMIT 1",
            [account.to_ascii_lowercase()],
            |_| Ok(()),
        )
        .optional()
        .map_err(error)?
        .is_some())
}
