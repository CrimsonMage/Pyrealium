//! Every action an `emote_action` row may name, with its parameters: one list that the loader
//! checks rows against, `pyrealium-content actions` prints for the editor, and
//! `docs/quest-actions.md` is written from (a test keeps that page in step).
//!
//! A row's p1..p5 are its parameters in order; an optional one may be left empty, and every
//! column after the last parameter must be empty.
use std::collections::BTreeMap;

use super::{Action, Stat};
use crate::movement::Placement;

/// What a parameter holds, and the check its text gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A quest's name (`quest.name`).
    Quest,
    /// A quest stage, 0 to 255.
    Stage,
    /// One of a quest's counters, 0 to 255.
    Counter,
    /// A whole number in lo..=hi.
    Range(u64, u64),
    /// An item or creature: an EntityDesc DID (`0x47...`).
    Entity,
    /// An `Effect` state: a WState DID (`0x6f...`).
    Effect,
    /// A set's name (`emote_set.name`).
    Set,
    /// Text (not empty).
    Text,
    /// A hex number (`0x...`): a string table, string key or cell.
    Hex,
    /// A percent in (0, 100], up to two decimals.
    Percent,
    /// A number (metres).
    Number,
    /// `0x47...:weight,...`: items with whole-number weights.
    Weights,
    /// A player stat (`Stat::NAMES`).
    Stat,
    /// An instance group's name (`instance_group.name`).
    Group,
    /// A spawn slot's name (`spawn_slot.name`).
    Slot,
}

impl Kind {
    /// What the editor shows for a parameter of this kind.
    pub fn describe(self) -> String {
        match self {
            Kind::Quest => "quest name".into(),
            Kind::Stage => "stage (0-255)".into(),
            Kind::Counter => "counter (0-255, or its quest_counter name)".into(),
            Kind::Range(lo, hi) if hi == u64::from(u32::MAX) || hi == u64::MAX => format!("whole number, at least {lo}"),
            Kind::Range(lo, hi) => format!("whole number {lo}-{hi}"),
            Kind::Entity => "entity DID (0x47...) or entity alias".into(),
            Kind::Effect => "Effect state DID (0x6f...) or effect alias".into(),
            Kind::Set => "set name".into(),
            Kind::Text => "text".into(),
            Kind::Hex => "hex (0x...) or alias".into(),
            Kind::Percent => "percent (up to 2 decimals)".into(),
            Kind::Number => "number".into(),
            Kind::Weights => "0x47...:weight,...".into(),
            Kind::Stat => format!("stat ({})", Stat::NAMES.join(", ")),
            Kind::Group => "instance group name".into(),
            Kind::Slot => "spawn slot name".into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Param { pub name: &'static str, pub kind: Kind, pub optional: bool }
const fn req(name: &'static str, kind: Kind) -> Param { Param { name, kind, optional: false } }
const fn opt(name: &'static str, kind: Kind) -> Param { Param { name, kind, optional: true } }

#[derive(Clone, Copy, Debug)]
pub struct Spec { pub name: &'static str, pub params: &'static [Param], pub doc: &'static str }

const U16: Kind = Kind::Range(0, 65_535);
const U32: Kind = Kind::Range(0, u32::MAX as u64);
const OK: Param = opt("success set", Kind::Set);
const FAIL: Param = opt("failure set", Kind::Set);
const QUEST: Param = req("quest", Kind::Quest);
const COUNTER: Param = req("counter", Kind::Counter);
const ITEM: Param = req("item", Kind::Entity);

/// Every action, in the order the reference lists them.
pub const ACTIONS: &[Spec] = &[
    // Talking
    Spec { name: "Tell", params: &[req("table", Kind::Hex), req("key", Kind::Hex)],
        doc: "Says a DAT string (StringInfo table and key) to the player as the NPC's chat line." },
    Spec { name: "Message", params: &[req("text", Kind::Text)],
        doc: "Says Pyrealium text to the player, where retail has no DAT line." },
    Spec { name: "Say", params: &[req("text", Kind::Text), opt("speaker", Kind::Text)],
        doc: "The NPC says the text: a speech bubble over it and the dialogue window, titled with the speaker (else the NPC's name)." },
    // Rewards and costs
    Spec { name: "AwardXP", params: &[req("xp", Kind::Range(1, u64::MAX))], doc: "Gives experience." },
    Spec { name: "AwardLevelProportionalXP", params: &[req("percent", Kind::Percent), opt("min", Kind::Range(0, u64::MAX)), opt("max", Kind::Range(0, u64::MAX))],
        doc: "Gives a percent of the experience the player's current level spans, at least min and at most max (empty: no limit)." },
    Spec { name: "AwardGold", params: &[req("gold", Kind::Range(1, u32::MAX as u64))], doc: "Gives coins." },
    Spec { name: "AwardXPShare", params: &[req("percent", Kind::Percent), QUEST, req("part", Kind::Counter), req("whole", Kind::Counter)],
        doc: "Gives the percent of the level's experience span times counter part over counter whole of the quest (a share of a reward: fights won of fights)." },
    Spec { name: "AwardGoldShare", params: &[req("gold", Kind::Range(1, u32::MAX as u64)), QUEST, req("part", Kind::Counter), req("whole", Kind::Counter)],
        doc: "Gives the coins times counter part over counter whole of the quest." },
    Spec { name: "GiveItem", params: &[ITEM, req("count", Kind::Range(1, 16))],
        doc: "Gives items (one bag slot each). The whole set fails when the bag is full." },
    Spec { name: "TakeItem", params: &[ITEM, req("count", Kind::Range(1, 16))],
        doc: "Takes items from the bag. The whole set fails when the player has fewer." },
    Spec { name: "RandomGive", params: &[req("items", Kind::Weights)], doc: "Gives one item drawn by weight." },
    Spec { name: "GiveStack", params: &[ITEM, req("quantity", Kind::Range(1, 65_535))],
        doc: "Gives the quantity of a stackable item as one stack (merged into one the bag holds when it fits)." },
    Spec { name: "TakeQuantity", params: &[ITEM, req("quantity", Kind::Range(1, 65_535))],
        doc: "Takes the quantity of a stackable item from the bag's stacks. The whole set fails when the player has less." },
    Spec { name: "GrantScene", params: &[req("scene", Kind::Hex)], doc: "Grants a story scene (the Quest panel's Story page)." },
    Spec { name: "LearnRecipe", params: &[req("recipe", Kind::Hex)],
        doc: "Teaches a recipe: a recipe any quest teaches can be crafted only once learned." },
    Spec { name: "AwardKingdomPoints", params: &[req("points", Kind::Range(1, u32::MAX as u64)), opt("max", U32)],
        doc: "Adds Kingdom Points, up to max (empty or 0: no cap)." },
    Spec { name: "SetKingdom", params: &[req("kingdom", Kind::Range(0, 3))], doc: "Sets the player's kingdom: 1 Order, 2 Dominion, 3 Shadow, 0 none." },
    Spec { name: "ApplyEffect", params: &[req("effect", Kind::Effect), req("seconds", Kind::Range(1, 31_536_000))],
        doc: "Puts an Effect state on the player for a time (a quest's curse or blessing); it shows on the effect strip." },
    Spec { name: "RemoveEffect", params: &[req("effect", Kind::Effect)], doc: "Takes an Effect state off the player." },
    // Quest state
    Spec { name: "SetStage", params: &[QUEST, req("stage", Kind::Stage)],
        doc: "Moves the quest to a stage and prints its journal line. Leaving stage 0 (or a completed stage) starts a run: refused below the quest's min_level, before its repeat delay, or past its solve limit. Reaching a completed stage solves it." },
    Spec { name: "IncrementStage", params: &[QUEST],
        doc: "SetStage to the quest's current stage plus one (with SetStage's start and solve rules and journal line)." },
    Spec { name: "EraseQuest", params: &[QUEST],
        doc: "Resets the quest to stage 0: its counters and run start go; its solves stay (they count for repeats)." },
    Spec { name: "IncrementQuest", params: &[QUEST], doc: "Counts one more on the quest's kill counter, up to its goal." },
    Spec { name: "IncrementCounter", params: &[QUEST, COUNTER, req("cap", Kind::Range(1, 65_535))], doc: "Counts one more on a counter, up to cap." },
    Spec { name: "DecrementCounter", params: &[QUEST, COUNTER], doc: "Counts one less on a counter (not below 0)." },
    Spec { name: "SetCounter", params: &[QUEST, COUNTER, req("value", U16)], doc: "Sets a counter." },
    // Checks: each runs its success set or its failure set (either may be empty)
    Spec { name: "InqStage", params: &[QUEST, req("min", Kind::Stage), req("max", Kind::Stage), OK, FAIL], doc: "Whether the quest's stage is in min..max." },
    Spec { name: "InqQuestCount", params: &[QUEST, req("count", U16), OK, FAIL], doc: "Whether the quest's kill counter reached count." },
    Spec { name: "InqCounter", params: &[QUEST, COUNTER, req("count", U16), OK, FAIL], doc: "Whether a counter reached count." },
    Spec { name: "InqItem", params: &[ITEM, OK, FAIL], doc: "Whether the player has the item in the bag." },
    Spec { name: "InqOwnsItems", params: &[ITEM, req("count", Kind::Range(1, 1000)), OK, FAIL], doc: "Whether the player has at least count of the item in the bag." },
    Spec { name: "InqLevel", params: &[QUEST, OK, FAIL], doc: "Whether the player's level reaches the quest's min_level." },
    Spec { name: "InqQuestReady", params: &[QUEST, OK, FAIL], doc: "Whether a new run of the quest may start now (its repeat delay and solve limit)." },
    Spec { name: "InqSolved", params: &[QUEST, OK, FAIL], doc: "Whether the quest was ever solved (a prerequisite)." },
    Spec { name: "InqQuestSolves", params: &[QUEST, req("min", U16), opt("max", U16), OK, FAIL], doc: "Whether the quest's solve count is in min..max (empty max: no upper bound)." },
    Spec { name: "InqStat", params: &[req("stat", Kind::Stat), req("min", U32), opt("max", U32), OK, FAIL], doc: "Whether a player stat is in min..max (empty max: no upper bound)." },
    Spec { name: "InqKingdom", params: &[req("kingdom", Kind::Range(0, 3)), OK, FAIL], doc: "Whether the player is in the kingdom (0: in any)." },
    Spec { name: "InqKingdomPoints", params: &[req("points", U32), OK, FAIL], doc: "Whether the player has at least points Kingdom Points." },
    // Flow
    Spec { name: "Goto", params: &[req("set", Kind::Set)], doc: "Runs another set's actions, then carries on with this one's." },
    // Instances
    Spec { name: "ClaimInstance", params: &[req("group", Kind::Group), OK, FAIL],
        doc: "Takes a free lane of the instance group for the player (success set), or finds none free (failure set). A player already holding a lane of it succeeds." },
    Spec { name: "ReleaseInstance", params: &[req("group", Kind::Group)],
        doc: "Frees the player's lane of the group and sends its slots' creatures away." },
    Spec { name: "Generate", params: &[req("slot", Kind::Slot)],
        doc: "Summons the slot's creature in the player's lane of its group (an earlier one's corpse goes first); nothing without a lane." },
    Spec { name: "DismissSlots", params: &[req("group", Kind::Group)],
        doc: "Sends away the creatures, alive or dead, of the group's slots in the player's lane." },
    Spec { name: "MoveToLane", params: &[req("group", Kind::Group)],
        doc: "Moves the player to the entry of its lane of the group." },
    Spec { name: "GenerateStep", params: &[req("group", Kind::Group), QUEST],
        doc: "Generate for the group's slot whose step (spawn_slot.step) is the quest's current stage: a chain of rooms or waves by stage." },
    Spec { name: "MoveToStep", params: &[req("group", Kind::Group), QUEST],
        doc: "MoveToSlot for the group's slot whose step is the quest's current stage." },
    Spec { name: "MoveToSlot", params: &[req("slot", Kind::Slot)],
        doc: "Moves the player to the slot's arrival point in its lane (spawn_slot.arrival: a room of its own per slot)." },
    // World
    Spec { name: "MoveTo", params: &[req("cell", Kind::Hex), req("x", Kind::Number), req("y", Kind::Number), req("z", Kind::Number), req("heading", Kind::Stage)],
        doc: "Moves the player to a place (heading 0-255, clockwise from north)." },
    Spec { name: "StartEscort", params: &[req("creature", Kind::Entity)], doc: "The nearest free creature of the entity starts following the player." },
    Spec { name: "EndEscort", params: &[], doc: "The player's escorts stop following." },
];

pub fn spec(name: &str) -> Option<&'static Spec> { ACTIONS.iter().find(|s| s.name == name) }

/// A checked parameter.
#[derive(Clone, Debug)]
enum V { None, Quest(u32), U(u64), Hex(u32), Set(String), Text(String), F(f32), Basis(u64), Weights(Vec<(u32, u32)>), Stat(Stat) }
impl V {
    fn u(&self) -> u64 { if let V::U(n) | V::Basis(n) = self { *n } else { 0 } }
    fn u8(&self) -> u8 { self.u() as u8 }
    fn u16(&self) -> u16 { self.u() as u16 }
    fn u32(&self) -> u32 { match self { V::Hex(n) | V::Quest(n) => *n, other => other.u() as u32 } }
    fn f(&self) -> f32 { if let V::F(x) = self { *x } else { 0.0 } }
    fn set(&self) -> Option<String> { if let V::Set(s) = self { Some(s.clone()) } else { None } }
    fn opt_u(&self) -> Option<u64> { if let V::None = self { None } else { Some(self.u()) } }
}

fn hex(v: &str) -> Option<u32> { u32::from_str_radix(v.strip_prefix("0x").or_else(|| v.strip_prefix("0X"))?, 16).ok() }

/// Names a parse needs to resolve: the registry's quests, every set, the aliases and the
/// quests' named counters.
pub struct Names<'a> {
    pub quests: &'a BTreeMap<String, u32>, pub sets: &'a BTreeMap<String, String>,
    pub aliases: &'a BTreeMap<String, (AliasKind, String)>, pub counters: &'a BTreeMap<(u32, String), u8>,
    pub groups: &'a BTreeMap<String, super::instances::Group>, pub slots: &'a BTreeMap<String, super::instances::Slot>,
}

/// What an alias (`alias.kind`) stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasKind { Entity, Effect, Hex, Placement, Entities, Objects }

/// The `alias` rows (name, kind, value, source), each value checked for its kind.
pub fn aliases(rows: &[super::Row]) -> std::io::Result<BTreeMap<String, (AliasKind, String)>> {
    let mut out = BTreeMap::new();
    for (at, f) in rows {
        let bad = |why: &str| std::io::Error::other(format!("{at}: {why}"));
        let kind = match f[1].as_str() { "entity" => AliasKind::Entity, "effect" => AliasKind::Effect, "hex" => AliasKind::Hex,
            "placement" => AliasKind::Placement, "entities" => AliasKind::Entities, "objects" => AliasKind::Objects,
            _ => return Err(bad("kind must be entity, effect, hex, placement, entities or objects")) };
        let list = |each: &dyn Fn(&str) -> bool| !f[2].trim().is_empty() && f[2].split(',').all(|v| each(v.trim()));
        let ok = match kind {
            AliasKind::Entity => hex(&f[2]).is_some_and(|e| e >> 24 == 0x47),
            AliasKind::Effect => hex(&f[2]).is_some_and(|e| e >> 24 == 0x6f),
            AliasKind::Hex => hex(&f[2]).is_some(),
            AliasKind::Placement => placement(&f[2]).is_some(),
            AliasKind::Entities => list(&|v| hex(v).is_some_and(|e| e >> 24 == 0x47)),
            AliasKind::Objects => list(&|v| v.strip_prefix("0x").is_some_and(|h| u64::from_str_radix(h, 16).is_ok())),
        };
        if !ok { return Err(bad("value does not fit its kind (entity 0x47..., effect 0x6f..., hex 0x..., placement `cell x y z heading`, entities `0x47...,0x47...`, objects `0x...,0x...`)")); }
        out.insert(f[0].clone(), (kind, f[2].clone()));
    }
    Ok(out)
}
pub(super) fn placement(text: &str) -> Option<Placement> {
    let t: Vec<&str> = text.split_whitespace().collect();
    if t.len() != 5 { return None; }
    let p = Placement { cell: hex(t[0])?, coordinates: [t[1].parse().ok()?, t[2].parse().ok()?, t[3].parse().ok()?], heading: t[4].parse().ok()? };
    p.valid().then_some(p)
}

/// The `quest_counter` rows (quest, name, idx, source): each quest's counter names.
pub fn counters(rows: &[super::Row], quests: &BTreeMap<String, u32>) -> std::io::Result<BTreeMap<(u32, String), u8>> {
    let mut out = BTreeMap::new();
    let mut used = std::collections::BTreeSet::new();
    for (at, f) in rows {
        let bad = |why: &str| std::io::Error::other(format!("{at}: {why}"));
        let quest = *quests.get(&f[0]).ok_or_else(|| bad("quest is not in the registry"))?;
        let idx: u8 = f[2].parse().map_err(|_| bad("idx must be 0 to 255"))?;
        if !used.insert((quest, idx)) { return Err(bad("two counters of the quest share an idx")); }
        out.insert((quest, f[1].clone()), idx);
    }
    Ok(out)
}

/// A hex value, or an alias of one of `kinds` standing for one.
fn hex_or_alias(text: &str, names: &Names, kinds: &[AliasKind]) -> Option<u32> {
    hex(text).or_else(|| names.aliases.get(text).filter(|(k, _)| kinds.contains(k)).and_then(|(_, v)| hex(v)))
}

fn value(kind: Kind, text: &str, names: &Names, quest: Option<u32>) -> Result<V, String> {
    let whole = |lo: u64, hi: u64| text.parse::<u64>().ok().filter(|n| (lo..=hi).contains(n)).map(V::U);
    Ok(match kind {
        Kind::Quest => V::Quest(*names.quests.get(text).ok_or("quest is not in the registry")?),
        Kind::Stage => whole(0, 255).ok_or("must be 0 to 255")?,
        Kind::Counter => match whole(0, 255) {
            Some(v) => v,
            None => V::U(u64::from(*quest.and_then(|q| names.counters.get(&(q, text.to_owned())))
                .ok_or("must be 0 to 255 or one of the quest's counter names (quest_counter)")?)),
        },
        Kind::Range(lo, hi) => whole(lo, hi).ok_or_else(|| if hi == u64::MAX { format!("must be a whole number, at least {lo}") } else { format!("must be a whole number {lo} to {hi}") })?,
        Kind::Entity => V::Hex(hex_or_alias(text, names, &[AliasKind::Entity]).filter(|e| e >> 24 == 0x47).ok_or("must be an EntityDesc DID (0x47...) or an entity alias")?),
        Kind::Effect => V::Hex(hex_or_alias(text, names, &[AliasKind::Effect]).filter(|e| e >> 24 == 0x6f).ok_or("must be an Effect state (WState 0x6f) DID or an effect alias")?),
        Kind::Set => if names.sets.contains_key(text) { V::Set(text.to_owned()) } else { return Err("set is not in emote-sets".into()) },
        Kind::Text => V::Text(text.to_owned()),
        Kind::Hex => V::Hex(hex_or_alias(text, names, &[AliasKind::Hex, AliasKind::Entity, AliasKind::Effect]).ok_or("must be hex (0x...) or an alias")?),
        Kind::Percent => {
            // The Vault's '2.5% XP': up to two decimals.
            let p = text.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v <= 100.0 && (v * 100.0).fract().abs() < 1e-9)
                .ok_or("must be in (0, 100] with at most two decimals")?;
            V::Basis((p * 100.0).round() as u64)
        }
        Kind::Number => V::F(text.parse::<f32>().ok().filter(|x| x.is_finite()).ok_or("must be a number")?),
        Kind::Weights => {
            let mut pairs = Vec::new();
            for part in text.split(',') {
                let (e, w) = part.split_once(':').ok_or("takes `0x47...:weight,...`")?;
                let w = w.trim().parse::<u32>().ok().filter(|w| *w > 0).ok_or("weights are positive whole numbers")?;
                pairs.push((hex_or_alias(e.trim(), names, &[AliasKind::Entity]).filter(|e| e >> 24 == 0x47).ok_or("items are EntityDesc DIDs (0x47...) or entity aliases")?, w));
            }
            V::Weights(pairs)
        }
        Kind::Group => if names.groups.contains_key(text) { V::Text(text.to_owned()) } else { return Err("is not an instance_group".into()) },
        Kind::Slot => if names.slots.contains_key(text) { V::Text(text.to_owned()) } else { return Err("is not a spawn_slot".into()) },
        Kind::Stat => V::Stat(Stat::parse(text).ok_or_else(|| format!("must be one of {}", Stat::NAMES.join(", ")))?),
    })
}

/// A row's action from its name and p1..p5, or why not (naming the parameter).
pub fn parse(name: &str, p: &[&str], names: &Names) -> Result<Action, String> {
    let spec = spec(name).ok_or_else(|| format!("unknown action `{name}` (`pyrealium-content actions` lists them)"))?;
    // MoveTo's place may be one placement alias in p1.
    let spread: Vec<String>;
    let p: Vec<&str> = match names.aliases.get(p[0]) {
        Some((AliasKind::Placement, place)) if name == "MoveTo" && p[1..].iter().all(|v| v.is_empty()) => {
            spread = place.split_whitespace().map(str::to_owned).collect();
            spread.iter().map(String::as_str).collect()
        }
        _ => p.to_vec(),
    };
    let n = spec.params.len();
    if p[n..].iter().any(|v| !v.is_empty()) {
        return Err(format!("{name} takes {n} parameter{}", if n == 1 { "" } else { "s" }));
    }
    let mut v: Vec<V> = Vec::with_capacity(n);
    for (i, param) in spec.params.iter().enumerate() {
        let quest = v.iter().find_map(|x| if let V::Quest(q) = x { Some(*q) } else { None });
        v.push(if p[i].is_empty() {
            if !param.optional { return Err(format!("{name} needs its {} (p{})", param.name, i + 1)); }
            V::None
        } else {
            value(param.kind, p[i], names, quest).map_err(|why| format!("p{} ({}) {why}", i + 1, param.name))?
        });
    }
    let quest = |i: usize| v[i].u32();
    let text = |x: &V| if let V::Text(t) = x { t.clone() } else { String::new() };
    Ok(match name {
        "Tell" => Action::Tell { table: v[0].u32(), key: v[1].u32() },
        "Message" => Action::Message(match &v[0] { V::Text(t) => t.clone(), _ => String::new() }),
        "Say" => Action::Say { text: text(&v[0]), speaker: match &v[1] { V::Text(t) => Some(t.clone()), _ => None } },
        "GiveStack" => Action::GiveStack { entity: v[0].u32(), quantity: v[1].u32() },
        "TakeQuantity" => Action::TakeQuantity { entity: v[0].u32(), quantity: v[1].u32() },
        "GrantScene" => Action::GrantScene(v[0].u32()),
        "LearnRecipe" => Action::LearnRecipe(v[0].u32()),
        "AwardXP" => Action::AwardXp(v[0].u()),
        "AwardLevelProportionalXP" => {
            let (min, max) = (v[1].opt_u().unwrap_or(0), v[2].opt_u().unwrap_or(0));
            if max != 0 && max < min { return Err("max must not be below min".into()); }
            Action::AwardLevelProportionalXp { basis_points: v[0].u(), min, max }
        }
        "AwardGold" => Action::AwardGold(v[0].u32()),
        "GiveItem" => Action::GiveItem { entity: v[0].u32(), count: v[1].u32() },
        "TakeItem" => Action::TakeItem { entity: v[0].u32(), count: v[1].u16() },
        "RandomGive" => Action::RandomGive(match &v[0] { V::Weights(w) => w.clone(), _ => Vec::new() }),
        "AwardKingdomPoints" => Action::AwardKingdomPoints { points: v[0].u32(), max: v[1].opt_u().unwrap_or(0) as u32 },
        "SetKingdom" => Action::SetKingdom(v[0].u8()),
        "ApplyEffect" => Action::ApplyEffect { effect: v[0].u32(), seconds: v[1].u() },
        "RemoveEffect" => Action::RemoveEffect { effect: v[0].u32() },
        "SetStage" => Action::SetStage { quest: quest(0), stage: v[1].u8() },
        "EraseQuest" => Action::EraseQuest { quest: quest(0) },
        "IncrementQuest" => Action::IncrementQuest { quest: quest(0) },
        "IncrementCounter" => Action::IncrementCounter { quest: quest(0), counter: v[1].u8(), cap: v[2].u16() },
        "DecrementCounter" => Action::DecrementCounter { quest: quest(0), counter: v[1].u8() },
        "SetCounter" => Action::SetCounter { quest: quest(0), counter: v[1].u8(), value: v[2].u16() },
        "InqStage" => Action::InqStage { quest: quest(0), min: v[1].u8(), max: v[2].u8(), success: v[3].set(), failure: v[4].set() },
        "InqQuestCount" => Action::InqQuestCount { quest: quest(0), count: v[1].u16(), success: v[2].set(), failure: v[3].set() },
        "InqCounter" => Action::InqCounter { quest: quest(0), counter: v[1].u8(), count: v[2].u16(), success: v[3].set(), failure: v[4].set() },
        "InqItem" => Action::InqOwnsItems { entity: v[0].u32(), count: 1, success: v[1].set(), failure: v[2].set() },
        "InqOwnsItems" => Action::InqOwnsItems { entity: v[0].u32(), count: v[1].u16(), success: v[2].set(), failure: v[3].set() },
        "InqLevel" => Action::InqLevel { quest: quest(0), success: v[1].set(), failure: v[2].set() },
        "InqQuestReady" => Action::InqQuestReady { quest: quest(0), success: v[1].set(), failure: v[2].set() },
        "InqSolved" => Action::InqSolved { quest: quest(0), success: v[1].set(), failure: v[2].set() },
        "InqQuestSolves" => {
            let (min, max) = (v[1].u16(), v[2].opt_u().map(|m| m as u16));
            if max.is_some_and(|m| m < min) { return Err("max must not be below min".into()); }
            Action::InqQuestSolves { quest: quest(0), min, max, success: v[3].set(), failure: v[4].set() }
        }
        "InqStat" => {
            let stat = match &v[0] { V::Stat(s) => *s, _ => Stat::Level };
            let (min, max) = (v[1].u32(), v[2].opt_u().map(|m| m as u32));
            if max.is_some_and(|m| m < min) { return Err("max must not be below min".into()); }
            Action::InqStat { stat, min, max, success: v[3].set(), failure: v[4].set() }
        }
        "InqKingdom" => Action::InqKingdom { kingdom: v[0].u8(), success: v[1].set(), failure: v[2].set() },
        "InqKingdomPoints" => Action::InqKingdomPoints { points: v[0].u32(), success: v[1].set(), failure: v[2].set() },
        "Goto" => Action::Goto(v[0].set().unwrap_or_default()),
        "MoveTo" => {
            let placement = Placement { cell: v[0].u32(), coordinates: [v[1].f(), v[2].f(), v[3].f()], heading: v[4].u8() };
            if !placement.valid() { return Err("not a valid placement".into()); }
            Action::MoveTo(placement)
        }
        "StartEscort" => Action::StartEscort(v[0].u32()),
        "ClaimInstance" => Action::ClaimInstance { group: text(&v[0]), success: v[1].set(), failure: v[2].set() },
        "ReleaseInstance" => Action::ReleaseInstance(text(&v[0])),
        "DismissSlots" => Action::DismissSlots(text(&v[0])),
        "Generate" => Action::Generate(text(&v[0])),
        "MoveToLane" => Action::MoveToLane(text(&v[0])),
        "MoveToSlot" => Action::MoveToSlot(text(&v[0])),
        "GenerateStep" => Action::GenerateStep { group: text(&v[0]), quest: quest(1) },
        "MoveToStep" => Action::MoveToStep { group: text(&v[0]), quest: quest(1) },
        "IncrementStage" => Action::IncrementStage { quest: quest(0) },
        "AwardXPShare" => Action::AwardXpShare { basis_points: v[0].u(), quest: quest(1), part: v[2].u8(), whole: v[3].u8() },
        "AwardGoldShare" => Action::AwardGoldShare { gold: v[0].u32(), quest: quest(1), part: v[2].u8(), whole: v[3].u8() },
        "EndEscort" => Action::EndEscort,
        other => return Err(format!("action `{other}` has no builder")),
    })
}

/// The action reference (`docs/quest-actions.md`), written from `ACTIONS`.
pub fn reference() -> String {
    let mut out = String::from("# Quest actions\n\n\
        Generated from `crates/pyrealium/src/quest_emotes/spec.rs` (`pyrealium-content actions --markdown`); \
        a test keeps it in step. Each `emote_action` row names one action in `action` and its parameters in \
        p1..p5, in this order; an optional parameter (in brackets) may be left empty, and the columns after the \
        last parameter stay empty. `delay_ms` waits that long before the action runs (the rest of the set waits \
        with it). Where a parameter takes a DID, an `alias` row's name may stand for it; MoveTo takes a \
        placement alias in p1 alone; a counter may be named by its `quest_counter` row. A `template_use` \
        fills a template's `$parameters` (`content/README.md`).\n\n| action | parameters | what it does |\n|---|---|---|\n");
    for spec in ACTIONS {
        let params: Vec<String> = spec.params.iter().enumerate().map(|(i, p)| {
            let text = format!("p{} {}: {}", i + 1, p.name, p.kind.describe());
            if p.optional { format!("[{text}]") } else { text }
        }).collect();
        out.push_str(&format!("| `{}` | {} | {} |\n", spec.name, if params.is_empty() { "none".into() } else { params.join("<br>") }, spec.doc));
    }
    out
}

/// The action list as tab-separated rows (name, parameter list, doc), for the editor.
pub fn table() -> String {
    let mut out = String::from("action\tparameters\tdoc\n");
    for spec in ACTIONS {
        let params: Vec<String> = spec.params.iter().map(|p| format!("{}{}:{}", if p.optional { "?" } else { "" }, p.name, p.kind.describe())).collect();
        out.push_str(&format!("{}\t{}\t{}\n", spec.name, params.join("|"), spec.doc));
    }
    out
}
