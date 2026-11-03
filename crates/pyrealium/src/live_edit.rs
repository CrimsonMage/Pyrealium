//! Live world editing: the world builder's overlay placements (layer 2) and its
//! rules (layer 3) applied to a running server without a restart.
//!
//! The admin thread (`admin.rs`) does every slow step: parsing, DAT checks on its
//! own handles, building create messages and diffing against what the game runs.
//! The game thread receives a finished `Change` and only applies it: map inserts
//! and removals, creature re-rules and the messages for players who can see it.
use crate::{
    content::{Content, Object},
    world_rules::{Rules, Target},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
};

/// What one sync changes. Replaced placements appear in both lists.
pub struct Change {
    pub motions: BTreeMap<u64, crate::creature_motion::Profile>,
    pub removed: Vec<u64>,
    pub added: Vec<Object>,
    pub replaced: usize,
    pub rules: Option<RulesChange>,
}

pub struct RulesChange {
    pub rules: Arc<Rules>,
    /// Types and placements whose stats, behaviour, patrol or group changed.
    pub affected: BTreeSet<Target>,
    pub loot: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub added: usize,
    pub removed: usize,
    pub replaced: usize,
    pub rules: usize,
    pub messages: usize,
    pub apply_us: u64,
}

pub struct Job {
    pub change: Change,
    pub reply: mpsc::SyncSender<Result<Summary, String>>,
}

/// The admin thread's copy of what the game runs, to diff a new sync against.
/// `spawns` is the overlay layer; `base` is the prepared world under it, so an
/// overlay object with a base identity replaces that object and removing the
/// override puts the base object back.
pub struct Planner {
    game_data: PathBuf,
    base: Arc<BTreeMap<u64, Object>>,
    spawns: BTreeMap<u64, (Arc<[u8]>, Option<crate::services::Service>)>,
    rules: Arc<Rules>,
    revision: String,
}

pub fn revision(world: &str, rules: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(world.as_bytes());
    hash.update([0]);
    hash.update(rules.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_world(text: &str, game_data: &Path) -> io::Result<BTreeMap<u64, Object>> {
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    Ok(Content::parse_text(text, game_data)?.into_objects())
}

fn parse_rules(text: &str) -> io::Result<Rules> {
    if text.trim().is_empty() { Ok(Rules::default()) } else { Rules::parse(text) }
}

/// Every target whose rules differ, including members of changed groups.
fn affected(old: &Rules, new: &Rules) -> BTreeSet<Target> {
    let mut result = BTreeSet::new();
    for key in old.stats.keys().chain(new.stats.keys()) {
        if old.stats.get(key) != new.stats.get(key) { result.insert(*key); }
    }
    for key in old.behaviour.keys().chain(new.behaviour.keys()) {
        if old.behaviour.get(key) != new.behaviour.get(key) { result.insert(*key); }
    }
    for id in old.patrols.keys().chain(new.patrols.keys()) {
        if old.patrols.get(id) != new.patrols.get(id) { result.insert(Target::Actor(*id)); }
    }
    for group in old.groups.iter().chain(&new.groups) {
        if !old.groups.contains(group) || !new.groups.contains(group) {
            result.extend(group.members.iter().map(|id| Target::Actor(*id)));
        }
    }
    result
}

impl Planner {
    /// The state the server started with.
    pub fn new(game_data: PathBuf, world: &str, rules: &str, base: BTreeMap<u64, Object>) -> io::Result<Self> {
        let spawns = parse_world(world, &game_data)?.into_iter().map(|(id, o)| (id, (o.spawn, o.service))).collect();
        Ok(Self { game_data, base: Arc::new(base), spawns, rules: Arc::new(parse_rules(rules)?), revision: revision(world, rules) })
    }
    pub fn revision(&self) -> &str { &self.revision }
    pub fn objects(&self) -> usize { self.spawns.len() }

    /// Parse, check and diff. The game is untouched until the plan is applied.
    pub fn plan(&self, world: &str, rules: &str) -> io::Result<Plan> {
        let objects = parse_world(world, &self.game_data)?;
        let new_rules = parse_rules(rules)?;
        let mut removed = Vec::new();
        let mut added = Vec::new();
        let mut replaced = 0;
        // What runs for an identity: its overlay object, else the base object under it.
        let was = |id: &u64| self.spawns.get(id).cloned().or_else(|| self.base.get(id).map(|o| (o.spawn.clone(), o.service)));
        for id in self.spawns.keys() {
            if objects.contains_key(id) { continue; }
            match self.base.get(id) {
                // A dropped override: the base object comes back.
                Some(base) => { removed.push(*id); replaced += 1; added.push(base.clone()); }
                None => removed.push(*id),
            }
        }
        let mut spawns = BTreeMap::new();
        for (id, object) in objects {
            match was(&id) {
                Some(old) if old.0 == object.spawn && old.1 == object.service => {}
                Some(_) => { removed.push(id); replaced += 1; added.push(object.clone()); }
                None => added.push(object.clone()),
            }
            spawns.insert(id, (object.spawn, object.service));
        }
        let mut motions=BTreeMap::new();
        if !added.is_empty() {
            let mut dat=crate::dat::Dat::open(self.game_data.join("portal.dat"))?;
            let mut catalog=crate::creature_motion::Catalog::default();
            for object in &added {
                if let Some(profile)=catalog.resolve(&mut dat,object.visual,object.scale)? {motions.insert(object.id,profile);}
            }
        }
        let rules_change = (*self.rules != new_rules).then(|| RulesChange {
            affected: affected(&self.rules, &new_rules),
            loot: self.rules.loot != new_rules.loot,
            rules: Arc::new(new_rules),
        });
        let next = rules_change.as_ref().map_or_else(|| self.rules.clone(), |r| r.rules.clone());
        Ok(Plan {
            change: Change { motions, removed, added, replaced, rules: rules_change },
            committed: Committed { spawns, rules: next, revision: revision(world, rules) },
        })
    }

    /// The game applied the change: it is now what runs.
    pub fn commit(&mut self, committed: Committed) {
        self.spawns = committed.spawns;
        self.rules = committed.rules;
        self.revision = committed.revision;
    }
}

/// A checked diff: the change for the game thread and the state it leads to.
pub struct Plan {
    pub change: Change,
    pub committed: Committed,
}

/// What the planner records once the game has applied a change.
pub struct Committed {
    spawns: BTreeMap<u64, (Arc<[u8]>, Option<crate::services::Service>)>,
    rules: Arc<Rules>,
    revision: String,
}
impl Committed {
    pub fn revision(&self) -> &str { &self.revision }
}

/// Game thread: apply a change. All checks run first, so a refused change leaves
/// the world exactly as it was.
pub fn apply(
    change: &Change,
    scene: &mut crate::scene::Scene,
    encounters: Option<&mut crate::encounters::Encounters>,
    loot: &mut crate::loot::Loot,
    now: u64,
) -> io::Result<(Vec<crate::scene::Delivery>, Summary)> {
    scene.check_content_live(&change.removed, &change.added)?;
    if change.replaced>change.added.len().min(change.removed.len()) {
        return Err(io::Error::other("invalid replacement count"));
    }
    if let Some(rules)=&change.rules {
        if encounters.is_none() && !rules.affected.is_empty() {
            return Err(io::Error::other("creature rule edits require enabled encounters"));
        }
        if rules.loot && !loot.catalog.loaded(){return Err(io::Error::other("loot rule edits require an initialized loot catalog"));}
    }
    // Stage every fallible population change before touching the live world or
    // loot. This also preserves RNG, timers and attacks when validation fails.
    let mut candidate=encounters.as_deref().cloned();
    if let Some(runtime)=candidate.as_mut() {
        if let Some(rules)=&change.rules {runtime.live_set_rules(rules.rules.clone());}
        for object in scene.content_objects().filter(|o|!change.removed.contains(&o.id)) {
            if runtime.live_is_creature(object)!=runtime.is_creature(object.id) {
                return Err(io::Error::other("changing an existing object's creature classification requires replacing that placement or restarting"));
            }
        }
        for &id in &change.removed {runtime.live_remove(id);}
        for object in &change.added {
            if runtime.live_is_creature(object) {runtime.live_add(object,change.motions.get(&object.id).copied(),now)?;}
        }
        runtime.live_rules(change.rules.as_ref().map(|r| &r.affected))?;
    }
    if let Some(rules)=change.rules.as_ref().filter(|r|r.loot) {
        if loot.catalog.loaded(){loot.catalog.set_world_rules(&rules.rules)?;}
    }
    // The scene precheck guarantees these bounded inserts/removals cannot fail.
    let mut deliveries=Vec::new();
    for &id in &change.removed {
        loot.remove(id);
        deliveries.extend(scene.remove_content_live(id)?);
    }
    for object in &change.added {
        let creature=candidate.as_ref().is_some_and(|r|r.is_creature(object.id));
        deliveries.extend(scene.insert_content_live(object.clone(),!creature)?);
        if let Some(runtime)=candidate.as_ref().filter(|_|creature) {
            for (stat,value) in runtime.live_stats(object.id){scene.content_stat(object.id,stat,value)?;}
        }
    }
    if let (Some(runtime),Some(next))=(encounters,candidate){*runtime=next;}
    let summary = Summary {
        added: change.added.len() - change.replaced,
        removed: change.removed.len() - change.replaced,
        replaced: change.replaced,
        rules: change.rules.as_ref().map_or(0, |r| r.affected.len() + usize::from(r.loot)),
        messages: deliveries.len(),
        apply_us: 0,
    };
    Ok((deliveries, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{content::NAMESPACE, movement::Placement, scene::{Body, Scene}, services::Service, world};
    use std::path::Path;

    fn player(id: u64, placement: Placement) -> Body {
        let bytes = world::create_stationary_object(&world::StationaryObject {
            character_id: id, parent_visual_did: 1, entity_did: 2, cell_id: placement.cell,
            position: placement.coordinates, orientation: placement.orientation(), instance_stamp: 1,
            visual_ordering_stamp: 0, timestamps: [0; 4] }).unwrap();
        Body::new(id, placement, bytes).unwrap()
    }
    fn object(id: u64, placement: Placement, service: Option<Service>) -> Object {
        Content::fixture(id, placement, service).into_objects().remove(&id).unwrap()
    }
    fn change(removed: Vec<u64>, added: Vec<Object>, rules: Option<Rules>) -> Change {
        let replaced = added.iter().filter(|o| removed.contains(&o.id)).count();
        Change { motions:BTreeMap::new(), removed, added, replaced, rules: rules.map(|rules| RulesChange {
            affected: affected(&Rules::default(), &rules), loot: false, rules: Arc::new(rules) }) }
    }
    fn world() -> (Scene, crate::encounters::Encounters, crate::loot::Loot) {
        let home = Placement::HUMAN_START;
        let content = Content::fixture(NAMESPACE + 7, home, None);
        let mut runtime = crate::encounters::Encounters::load(Path::new("training"), &content).unwrap();
        let mut scene = Scene::with_content(content);
        runtime.initialize_spawners(&mut scene, 0).unwrap();
        scene.enter(player(1, home)).unwrap();
        (scene, runtime, crate::loot::Loot::default())
    }

    #[test]
    fn refused_runtime_change_does_not_change_rules_spawners_or_placements() {
        let (mut scene, mut runtime, mut loot)=world();
        let id=NAMESPACE+0x20001;
        let original=runtime.initial_health().collect::<Vec<_>>();
        let mut rules=Rules::parse(&format!("{}\nstats\t470007d1\t999\t-\t-\t-\t-\t-\t-\t-\t-\t-\n",crate::world_rules::HEADER)).unwrap();
        // An invalid roster can also arrive through an in-process operator API.
        rules.groups.push(crate::world_rules::Group{id:1,capacity:1,regeneration_ms:None,members:vec![id,id]});
        assert!(apply(&change(vec![],vec![object(id,Placement::HUMAN_START,None)],Some(rules)),&mut scene,Some(&mut runtime),&mut loot,10).is_err());
        assert_eq!(runtime.initial_health().collect::<Vec<_>>(),original);
        assert!(!runtime.is_creature(id));
        assert!(!scene.content_objects().any(|o|o.id==id));
        let fresh=object(id,Placement::HUMAN_START,Some(Service::Vendor));
        assert!(apply(&change(vec![],vec![fresh.clone(),fresh],None),&mut scene,Some(&mut runtime),&mut loot,10).is_err());
        assert!(!scene.content_objects().any(|o|o.id==id));
    }

    #[test]
    fn live_native_creatures_require_resolved_movement_before_mutation() {
        let Ok(directory)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let (mut scene,mut runtime,mut loot)=world();
        runtime.load_motion(&Content::fixture(NAMESPACE+7,Placement::HUMAN_START,None),Path::new(&directory)).unwrap();
        let id=NAMESPACE+0x20001;
        let native=runtime.movement_profile(NAMESPACE+7).unwrap();
        let mut plan=change(vec![],vec![object(id,Placement::HUMAN_START,None)],None);
        assert!(apply(&plan,&mut scene,Some(&mut runtime),&mut loot,10).is_err());
        assert!(!scene.content_objects().any(|o|o.id==id));
        plan.motions.insert(id,native);
        apply(&plan,&mut scene,Some(&mut runtime),&mut loot,10).unwrap();
        assert_eq!(runtime.movement_profile(id),Some(native));
        assert!((native.combat.speed-3.9).abs()<0.0001);
    }
    #[test]
    fn objects_appear_and_vanish_for_players_who_see_them() {
        let (mut scene, mut runtime, mut loot) = world();
        let vendor = object(NAMESPACE + 0x20000, Placement::HUMAN_START, Some(Service::Vendor));
        let (shown, summary) = apply(&change(vec![], vec![vendor.clone()], None), &mut scene, Some(&mut runtime), &mut loot, 10).unwrap();
        assert_eq!((shown.len(), shown[0].recipient, summary.added), (1, 1, 1));
        assert_eq!(&shown[0].data[..], &vendor.spawn[..]);
        assert!(scene.content_placement(vendor.id).is_some());
        let (gone, summary) = apply(&change(vec![vendor.id], vec![], None), &mut scene, Some(&mut runtime), &mut loot, 20).unwrap();
        assert_eq!((gone.len(), summary.removed), (1, 1));
        assert!(scene.content_placement(vendor.id).is_none());
        // Re-adding is a newer lifetime for clients that saw the old one.
        let (again, _) = apply(&change(vec![], vec![vendor.clone()], None), &mut scene, Some(&mut runtime), &mut loot, 30).unwrap();
        assert_eq!(&again[0].data[24..26], &2u16.to_le_bytes());
    }

    #[test]
    fn creatures_join_spawners_take_new_rules_and_leave_cleanly() {
        let (mut scene, mut runtime, mut loot) = world();
        let id = NAMESPACE + 0x20001;
        let (shown, _) = apply(&change(vec![], vec![object(id, Placement::HUMAN_START, None)], None), &mut scene, Some(&mut runtime), &mut loot, 100).unwrap();
        assert!(shown.is_empty(), "a creature appears through its spawner");
        assert!(runtime.is_creature(id));
        let actions = runtime.spawners_mut_for_test().poll(100);
        assert!(actions.contains(&crate::spawners::Action::Spawn(id)));
        let rules = Rules::parse(&format!("{}\nstats\t{id:016x}\t999\t-\t-\t40\t-\t-\t-\t-\t-\t5000\n", crate::world_rules::HEADER)).unwrap();
        apply(&change(vec![], vec![], Some(rules)), &mut scene, Some(&mut runtime), &mut loot, 200).unwrap();
        assert!(runtime.initial_health().any(|row| row == (id, 999)));
        assert_eq!(runtime.spawners().policy(id).unwrap().regeneration_ms, 5000);
        apply(&change(vec![id], vec![], None), &mut scene, Some(&mut runtime), &mut loot, 300).unwrap();
        assert!(!runtime.is_creature(id));
        assert!(runtime.spawners_mut_for_test().poll(100_000).iter().all(|a| *a != crate::spawners::Action::Spawn(id)));
    }

    #[test]
    fn a_refused_change_leaves_the_world_untouched() {
        let (mut scene, mut runtime, mut loot) = world();
        let fresh = object(NAMESPACE + 0x20002, Placement::HUMAN_START, Some(Service::Vendor));
        let clash = object(NAMESPACE + 7, Placement::HUMAN_START, Some(Service::Vendor));
        assert!(apply(&change(vec![], vec![fresh.clone(), clash], None), &mut scene, Some(&mut runtime), &mut loot, 10).is_err());
        assert!(scene.content_placement(fresh.id).is_none());
        assert!(apply(&change(vec![NAMESPACE + 0x29999], vec![], None), &mut scene, Some(&mut runtime), &mut loot, 10).is_err());
    }

    #[test]
    fn dropping_an_override_brings_the_base_object_back() {
        // Re-adding the base object resolves its movement in the DATs.
        let Ok(directory) = std::env::var("AC2_TEST_GAME_DATA") else { return };
        // The world builder saved a moved copy of base object 7; the next sync drops it.
        let base = object(NAMESPACE + 7, Placement::HUMAN_START, Some(Service::Vendor));
        let moved = object(NAMESPACE + 7, Placement { coordinates: [3., 4., 0.], ..Placement::HUMAN_START }, Some(Service::Vendor));
        let planner = Planner {
            game_data: PathBuf::from(directory),
            base: Arc::new(BTreeMap::from([(base.id, base.clone())])),
            spawns: BTreeMap::from([(moved.id, (moved.spawn.clone(), moved.service))]),
            rules: Arc::new(Rules::default()),
            revision: String::new(),
        };
        let plan = planner.plan("", "").unwrap();
        assert_eq!(plan.change.removed, vec![base.id]);
        assert_eq!(plan.change.replaced, 1);
        assert_eq!(&plan.change.added[0].spawn[..], &base.spawn[..]);
        assert!(plan.committed.spawns.is_empty());
    }

    #[test]
    fn an_override_moves_a_base_object_in_the_running_world() {
        let (mut scene, mut runtime, mut loot) = world();
        let creature = object(NAMESPACE + 7, Placement { coordinates: [3., 4., 0.], ..Placement::HUMAN_START }, None);
        let (_, summary) = apply(&change(vec![creature.id], vec![creature.clone()], None), &mut scene, Some(&mut runtime), &mut loot, 10).unwrap();
        assert_eq!((summary.replaced, summary.added, summary.removed), (1, 0, 0));
        // A creature comes back through its spawner, at the new place.
        let placed = scene.content_objects().find(|o| o.id == creature.id).unwrap();
        assert_eq!(placed.placement.coordinates, [3., 4., 0.]);
        assert!(runtime.is_creature(creature.id));
    }

    #[test]
    fn rules_diff_names_changed_targets_and_group_members() {
        let old = Rules::parse(&format!("{}\nstats\t470007d1\t5\t-\t-\t-\t-\t-\t-\t-\t-\t-\ngroup\t1\t1\t-\t8140000000020001\n", crate::world_rules::HEADER)).unwrap();
        let new = Rules::parse(&format!("{}\nstats\t470007d1\t6\t-\t-\t-\t-\t-\t-\t-\t-\t-\ngroup\t1\t1\t9000\t8140000000020001\n", crate::world_rules::HEADER)).unwrap();
        let set = affected(&old, &new);
        assert!(set.contains(&Target::Entity(0x470007d1)) && set.contains(&Target::Actor(0x8140000000020001)));
        assert!(affected(&new, &new).is_empty());
    }

    #[test]
    fn a_thousand_object_sync_applies_within_the_frame_budget() {
        let (mut scene, mut runtime, mut loot) = world();
        let objects = (0..1000u64).map(|i| {
            let block = (i / 64) as u32;
            let cell = ((0x80 + block) << 24) | (0x80 << 16) | (1 + (i % 64) as u32);
            object(NAMESPACE + 0x20000 + i, Placement { cell, coordinates: [10., 10., 0.], heading: 0 }, Some(Service::Vendor))
        }).collect::<Vec<_>>();
        let started = std::time::Instant::now();
        apply(&change(vec![], objects, None), &mut scene, Some(&mut runtime), &mut loot, 10).unwrap();
        let elapsed = started.elapsed();
        eprintln!("1000-object live apply: {elapsed:?}");
        assert!(elapsed < std::time::Duration::from_millis(if cfg!(debug_assertions) { 100 } else { 10 }));
    }
}
