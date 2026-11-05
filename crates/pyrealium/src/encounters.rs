//! Server-authoritative development encounters. Balance rules remain authored.
//! This module does not claim original combat balance, collision or animation parity.
use crate::{
    content::Content,
    movement::Placement,
    progression::{Command, Progression},
    scene::Scene,
    store::Store,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, Read},
    path::Path,
};
fn invalid(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

#[cfg(test)]
mod skill_tests;

#[cfg(test)]
mod locomotion_tests {
    use super::*;
    #[test]
    fn different_speeds_preserve_collision_steps_and_attack_standoff() {
        let from=Placement::HUMAN_START;
        let goal=from.translated([10.,0.,0.]).unwrap();
        for speed in [3.9,6.5,7.8] {
            let mut route=Vec::new();
            let next=advance(from,goal,2.5,speed*0.1,&mut route,|a,b| {
                assert!(a.distance(b).unwrap()<0.201);Some(b)
            }).unwrap();
            assert!((from.distance(next).unwrap()-speed*0.1).abs()<0.0001);
            let stop=advance(from,goal,2.5,20.,&mut Vec::new(),|_,b|Some(b)).unwrap();
            assert!((stop.distance(goal).unwrap()-2.5).abs()<0.0001);
        }
        // A wall encountered partway through a fast tick cannot be skipped.
        let limited=advance(from,goal,0.,2.,&mut Vec::new(),|_,b|
            (b.coordinates[0]<=from.coordinates[0]+0.41).then_some(b)).unwrap();
        assert!((from.distance(limited).unwrap()-0.4).abs()<0.0001);
    }
    #[test]
    fn return_spends_time_budget_across_waypoints_without_cutting_corners() {
        let home=Placement::HUMAN_START;
        let mut route=Vec::new();
        let corner=advance(home,home.translated([1.,0.,0.]).unwrap(),0.,1.,&mut route,|_,b|Some(b)).unwrap();
        let end=advance(corner,corner.translated([0.,1.,0.]).unwrap(),0.,1.,&mut route,|_,b|Some(b)).unwrap();
        let next=retreat(end,1.5,&mut route,|a,b| {
            let [x,y,_]=a.offset_to(b).unwrap();
            assert!(x.abs()<0.001 || y.abs()<0.001,"Must follow recorded right-angle path");
            assert!(a.distance(b).unwrap()<0.201);Some(b)
        }).unwrap();
        let [x,y,_]=home.offset_to(next).unwrap();assert!((x-0.5).abs()<0.001 && y.abs()<0.001);
        let back=retreat(next,1.5,&mut route,|_,b|Some(b)).unwrap();
        assert!(home.distance(back).unwrap()<0.001);assert!(route.is_empty());
    }
}

#[cfg(test)]
mod spawner_tests {
    use super::*;
    #[test]
    fn arwic_room_rosters_use_native_stats_and_refill_only_after_corpse_removal() {
        let Ok(path)=std::env::var("AC2_TEST_GAME_DATA") else{return};
        let content=Content::parse_text(crate::content_db::files::text("arwic-mines-world-v1.tsv"),Path::new(&path)).unwrap();
        let mut runtime=Encounters::parse(crate::content_db::files::text("development-encounters.tsv"),&content).unwrap();
        assert_eq!(runtime.actors.len(),6);assert_eq!(runtime.spawners.group_count(),2);
        assert_eq!(runtime.actors.values().filter(|a|a.entity==crate::arwic_items::OUTCAST).count(),4);
        for actor in runtime.actors.values() {
            assert_eq!((actor.rule.level,actor.health),if actor.entity==crate::arwic_items::OUTCAST {(Some(6),164)}else{(Some(3),59)});
            assert!(actor.ai.aggressive());
            if actor.entity==crate::arwic_items::OUTCAST {
                assert_eq!((actor.rule.damage,actor.rule.armor,actor.rule.xp),(28,4,19));
                assert_eq!((actor.ai.perception,actor.ai.homesick,actor.ai.loot_profile),(30.,Some(60.),948));
                assert!(!actor.ai.wanders(),"No wandering radius was recovered for this template");
            }
        }
        let mut scene=Scene::with_content(content);runtime.initialize_spawners(&mut scene,0).unwrap();
        let actions=runtime.spawners.poll(0);assert_eq!(actions.len(),6);
        for action in actions {let crate::spawners::Action::Spawn(id)=action else{panic!()};scene.respawn_content(id).unwrap();runtime.spawned(id).unwrap();}
        let id=*runtime.actors.keys().next().unwrap();
        assert!(runtime.spawners.died(id,10,60000));
        assert!(runtime.spawners.poll(30010).is_empty(),"A corpse still occupies its child slot");
        assert_eq!(runtime.spawners.poll(60010),vec![crate::spawners::Action::Remove(id)]);
        runtime.spawners.removed(id,60010);
        assert_eq!(runtime.spawners.poll(60010),vec![crate::spawners::Action::Spawn(id)]);
        runtime.spawners.spawned(id);assert!(runtime.spawners.poll(999999).is_empty());
    }
    #[test]
    fn policy_overrides_initial_delay_without_moving_the_observed_anchor() {
        let id=crate::content::NAMESPACE+7;let home=Placement::HUMAN_START;
        let content=Content::fixture(id,home,None);
        let text=format!("{}\nspawner\t{:08x}\t2000\t5000\n",
            crate::content_db::files::text("development-encounters.tsv"),crate::content::IDOL_SPAWN);
        let mut runtime=Encounters::parse(&text,&content).unwrap();
        let mut scene=Scene::with_content(content);
        runtime.initialize_spawners(&mut scene,100).unwrap();
        assert!(scene.content_placement(id).is_none());
        assert!(runtime.spawners.poll(2099).is_empty());
        assert_eq!(runtime.spawners.poll(2100),vec![crate::spawners::Action::Spawn(id)]);
        scene.respawn_content(id).unwrap();runtime.spawned(id).unwrap();
        assert_eq!(scene.content_placement(id),Some(home));
        let mut moved=home;moved.coordinates[0]+=0.5;
        scene.move_creature(id,moved,2200).unwrap();
        assert!(runtime.spawners.poll(10000).is_empty(),"Pursuit is not a vacant population slot");
        scene.despawn_content(id).unwrap();runtime.removed(id,10000);
        assert!(runtime.spawners.poll(14999).is_empty());
        assert_eq!(runtime.spawners.poll(15000),vec![crate::spawners::Action::Spawn(id)]);
        scene.respawn_content(id).unwrap();runtime.spawned(id).unwrap();
        assert_eq!(scene.content_placement(id),Some(home));
        assert!(runtime.spawners.poll(100000).is_empty());
    }
    #[test]
    fn world_rules_override_stats_respawn_and_plan_patrol_legs() {
        let id=crate::content::NAMESPACE+7;let home=Placement::HUMAN_START;
        let content=Content::fixture(id,home,None);
        let rules=crate::world_rules::Rules::parse(&format!("{}\nstats\t{id:016x}\t999\t-\t-\t30\t-\t-\t-\t-\t-\t-\n\
            group\t5\t1\t4000\t{id:016x}\npatrol\t{id:016x}\t0\t3,0\n",crate::world_rules::HEADER)).unwrap();
        let mut runtime=Encounters::parse_with_rules(crate::content_db::files::text("development-encounters.tsv"),&content,&rules).unwrap();
        assert_eq!(runtime.initial_health().collect::<Vec<_>>(),vec![(id,999)]);
        assert!(runtime.inspection_stats().any(|row|row==(id,0x12e,30)));
        let mut scene=Scene::with_content(content);
        runtime.initialize_spawners(&mut scene,0).unwrap();
        assert_eq!(runtime.spawners.poll(0),vec![crate::spawners::Action::Spawn(id)]);
        scene.respawn_content(id).unwrap();runtime.spawned(id).unwrap();
        scene.despawn_content(id).unwrap();runtime.removed(id,1000);
        assert!(runtime.spawners.poll(4999).is_empty());
        assert_eq!(runtime.spawners.poll(5000),vec![crate::spawners::Action::Spawn(id)],"group respawn wins");
        let actor=&runtime.actors[&id];
        assert_eq!(actor.patrol.as_ref().unwrap().waypoints,vec![[3.,0.]]);
    }
    #[test]
    fn policy_cannot_make_a_fixed_service_attackable_or_create_an_unplaced_creature() {
        let content=Content::fixture(crate::content::NAMESPACE+7,Placement::HUMAN_START,None);
        let text=crate::content_db::files::text("development-encounters.tsv");
        for row in ["spawner\t47000001\t0\t1000", "spawner\t470007d1\t0\t0",
            "spawner\t470007d1\t86400001\t1000",
            "spawner\t470007d1\t0\t1000\nspawner\t470007d1\t0\t2000"] {
            assert!(Encounters::parse(&format!("{text}\n{row}\n"),&content).is_err());
        }
        let service=Content::fixture(crate::content::NAMESPACE+8,Placement::HUMAN_START,Some(crate::services::Service::Vendor));
        assert!(Encounters::parse(text,&service).is_err());
    }
}
#[derive(Clone, Copy, PartialEq)]
struct Rule {
    health: u32,
    level: Option<u32>,
    vigor: Option<u32>,
    damage: u32,
    armor: u32,
    interval: u64,
    range: f32,
    respawn: u64,
    xp: u64,
    gold: u32,
    leash: f32,
}
#[derive(Clone)]
struct Actor {
    entity: u32,
    name: Option<String>,
    home: Placement,
    pursuit: bool,
    movement: crate::creature_motion::Profile,
    velocity: f32,
    // Verified steps retraced on disengagement, without inventing a path home.
    route: Vec<Placement>,
    rule: Rule,
    health: u32,
    vigor: Option<u32>,
    target: Option<u64>,
    next_attack: u64,
    next_ability: u64,
    ai: crate::creature_ai::Profile,
    next_idle: u64,
    wander_goal: Option<Placement>,
    evading: bool,
    random: u64,
    /// Operator patrol route. Patrol legs are not recorded in `route`; only a
    /// pursuit is retraced, back to the patrol point where it began.
    patrol: Option<crate::world_rules::Patrol>,
    patrol_next: usize,
    patrol_wait: u64,
    /// Patrolling creatures leash around the point where they engaged.
    engaged_at: Option<Placement>,
}
#[derive(Clone, Copy)]
struct Attack {
    target: u64,
    next: u64,
    repeat: bool,
    category: usize,
    stage: u8,
    started: u64,
    skill: bool,
    strike: Option<Strike>,
    periodic: bool,
}
/// One skill hit: weapon damage plus `bonus`, spending up to `cost` vigor.
/// `behavior` and `fx` are the skill record's story behavior and FX (0 = none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strike {
    pub ratings: Option<(crate::combat::Ratings,u64)>,
    pub skill: u32,
    pub bonus: u32,
    pub vigor_damage: u32,
    pub weapon_damage: bool,
    pub cost: u32,
    pub range: f32,
    pub behavior: u32,
    pub fx: u32,
    pub burn: Option<Over>,
    /// The target-side `Effect` states and durations (DID 0: none) that join the creature's
    /// effect list when the strike hits.
    pub effects: [(u32, u64); 2],
}
mod creature_skills;
mod escort;
mod ledger;
pub use ledger::Applied;
/// A vital change applied every `period_ms` for `duration_ms`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Over {
    pub health: u32,
    pub vigor: u32,
    pub period_ms: u64,
    pub duration_ms: u64,
}
/// Regeneration multipliers from a timed buff (1.0 = unchanged).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Regen {
    pub health: f32,
    pub vigor: f32,
    pub duration_ms: u64,
}
#[derive(Clone)]
struct Burn {
    skill: u32,
    source: u64,
    target: u64,
    per_tick: u32,
    vigor_per_tick: u32,
    period: u64,
    next: u64,
    until: u64,
}
#[derive(Clone)]
struct Mend {
    instant: bool,
    skill: u32,
    source: u64,
    player: u64,
    health: u32,
    vigor: u32,
    period: u64,
    next: u64,
    until: u64,
    /// FX shown on the player when the first restoration lands (0 = none).
    fx: u32,
}
const MAX_TIMED: usize = 512;
#[derive(Clone)]
pub enum Event {
    /// A skill's cast on its caster; `skill` and `target` let the story fire the skill's missile
    /// (`crate::skill_missiles`).
    SkillSwing { player: u64, behavior: u32, skill: u32, target: u64 },
    Fx { target: u64, fx: u32 },
    Loot { actor:u64, player:u64, entity:u32, ai:crate::creature_ai::Profile },
    Move { actor: u64, placement: Placement },
    IdolVisual(u64, crate::combat_visual::IdolAction),
    Swing(u64),
    Projectile { player: u64, target: u64, profile: crate::projectiles::Profile, flight_ms: u64 },
    /// Signed health on the victim. Negative is damage. The vital update is separate.
    Hit {
        victim: u64,
        attacker: u64,
        health_change: i32,
    },
    Item {
        player: u64,
        item: crate::inventory::Item,
        instance: u16,
    },
    Quest {
        player: u64,
        previous: crate::quests::Book,
        book: crate::quests::Book,
    },
    /// A player's kill of a creature, for the fellows' quest credit (`quest_emotes::fellow_kill`).
    Kill { actor: u64, player: u64, entity: u32 },
    /// A kill's experience for the server to split among `player`'s fellowship
    /// (`fellowship_xp`); not yet paid to anyone.
    KillXp { player: u64, xp: u64 },
    /// A creature a player escorted died or left the world (`quest_emotes` EscortLost sets).
    EscortLost { player: u64, entity: u32 },
    /// An escort whose player stayed out of its radar too long gave up (`quest_emotes`
    /// EscortAbandoned sets, else EscortLost).
    EscortAbandoned { player: u64, entity: u32 },
    Health {
        actor: u64,
        health: u32,
    },
    Vigor { actor: u64, vigor: u32 },
    Despawn(u64),
    Respawn {
        actor: u64,
        health: u32,
        vigor: Option<u32>,
    },
    Owner {
        player: u64,
        previous: Progression,
        state: Progression,
    },
    Note {
        player: u64,
        text: String,
    },
    Stop(u64),
    /// A timed effect joined `owner`'s effects (a player's goes to its client as `WM_Effect` 0x17).
    EffectAdded { owner: u64, effect: Applied },
    /// An effect left `owner`: its time ran out, it was refreshed, or `owner` died or left.
    EffectRemoved { owner: u64, eid: u32, expired: bool },
    /// A live rules edit changed a creature's vitals or level.
    Stats { actor: u64, max_health: u32, health: u32, level: Option<u32> },
}
#[derive(Clone)]
pub struct Encounters {
    native_motion: bool,
    dice: crate::combat::Dice,
    burns: Vec<Burn>,
    mends: Vec<Mend>,
    regen: BTreeMap<(u64,u32), (Regen, u64)>,
    ratings: Vec<(u64,u32,crate::combat::Ratings,u64)>,
    /// The timed effects on players and creatures, as the client lists them.
    ledger: Vec<ledger::Entry>,
    /// Escorted creatures (`quest_emotes` StartEscort) and their players (`escort`).
    escorts: escort::Escorts,
    /// The harmful abilities of each creature entity.
    abilities: BTreeMap<u32, Vec<crate::abilities::CreatureAbility>>,
    drains: Vec<creature_skills::Drain>,
    next_eid: u32,
    actors: BTreeMap<u64, Actor>,
    spawners: crate::spawners::Spawners,
    bestiary: Bestiary,
    /// Events from live edits, returned by the next tick at once.
    pending: Vec<Event>,
    attacks: BTreeMap<u64, Attack>,
    cooldowns: BTreeMap<u64, u64>,
    damage: u32,
    cost: u32,
    interval: u64,
    range: f32,
    next_tick: u64,
    regen_health: u32,
    regen_vigor: u32,
    regen_interval: u64,
    next_regen: u64,
    training_loot: bool,
    /// The players whose kill experience the server splits among their fellowship
    /// (`fellowship_xp`): their kills leave it out of the reward and raise [`Event::KillXp`].
    shared_xp: BTreeSet<u64>,
}
fn in_range(a: Option<Placement>, b: Option<Placement>, range: f32) -> bool {
    a.zip(b).and_then(|(a,b)|a.distance(b)).is_some_and(|d|d<=range)
}
use escort::ESCORT_DISTANCE;

impl Encounters {
    /// The players in an experience-sharing fellowship, set before each tick.
    pub fn set_shared_xp(&mut self, players: BTreeSet<u64>) {
        self.shared_xp = players;
    }
    /// The nearest living, unengaged creature of `entity` within 30 m of `player` follows it;
    /// the actor, or None when there is none (Pyrealium policy: the search radius).
    pub fn start_escort(&mut self, player: u64, entity: u32, scene: &Scene) -> Option<u64> {
        let id = self.actors.iter()
            .filter(|(id, a)| a.entity == entity && a.health > 0 && a.target.is_none() && !self.escorts.has(**id))
            .filter_map(|(id, _)| scene.combat_distance(player, *id).filter(|d| *d <= 30.0).map(|d| (*id, d)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?.0;
        self.escorts.start(id, player);
        Some(id)
    }
    /// The player's escorts stop following (where they stand).
    pub fn end_escort(&mut self, player: u64) {
        self.escorts.end_for(player);
    }
    pub fn escorting(&self, player: u64) -> Vec<u64> {
        self.escorts.of(player)
    }
}

fn facing(from:Placement,target:Placement)->Option<Placement> {
    let [dx,dy,_]=from.offset_to(target)?;
    if dx.hypot(dy)<0.001 {return Some(from);}
    // Native Heading::ToUInt8 truncates, using the f32 1/360 constant.
    let degrees=dx.atan2(dy).to_degrees().rem_euclid(360.0);
    Some(Placement{heading:(f64::from(degrees)*f64::from(1f32/360.)*255.).trunc() as u8,..from})
}
fn approach(from:Placement,target:Placement,stop:f32,budget:f32)->Option<Placement> {
    let [dx,dy,_]=from.offset_to(target)?;
    let distance=dx.hypot(dy);
    if distance<=stop+0.01{return None;}
    let step=budget.min(distance-stop);
    if step<=0.0001{return None;}
    let next=from.translated([dx/distance*step,dy/distance*step,0.])?;
    facing(next,target)
}
/// Spend a distance budget through the same short, collision-checked segments
/// used before variable speed. Retain every segment for safe homeward retracing.
/// A walked floor point raised to where the body rests on it ([`crate::creature_motion::Profile::rest_height`]).
fn rested(mut floor:Placement,height:f32)->Placement {floor.coordinates[2]+=height;floor}
fn advance(from:Placement,target:Placement,stop:f32,mut budget:f32,route:&mut Vec<Placement>,
    mut walk:impl FnMut(Placement,Placement)->Option<Placement>)->Option<Placement> {
    let mut point=from;
    for _ in 0..128 {
        if route.len()>=4096 {break;}
        let Some(proposed)=approach(point,target,stop,budget.min(0.2)) else {break};
        let Some(next)=walk(point,proposed) else {break};
        if from.distance(next).is_none_or(|d|d>crate::creature_motion::MAX_UPDATE_DISTANCE) {break;}
        let [x,y,_]=point.offset_to(next)?;let distance=x.hypot(y);
        if distance<0.0001 {break;}
        budget=(budget-distance).max(0.);route.push(point);point=next;
    }
    (point!=from).then_some(point)
}
fn retreat(from:Placement,mut budget:f32,route:&mut Vec<Placement>,
    mut walk:impl FnMut(Placement,Placement)->Option<Placement>)->Option<Placement> {
    let mut point=from;
    for _ in 0..128 {
        while route.last().is_some_and(|p|point.offset_to(*p).is_some_and(|[x,y,z]|x.hypot(y)<0.01 && z.abs()<=0.65)) {route.pop();}
        let Some(&goal)=route.last() else {break};
        let Some(proposed)=approach(point,goal,0.,budget.min(0.2)) else {break};
        let Some(next)=walk(point,proposed) else {break};
        if from.distance(next).is_none_or(|d|d>crate::creature_motion::MAX_UPDATE_DISTANCE) {break;}
        let [x,y,_]=point.offset_to(next)?;let distance=x.hypot(y);
        if distance<0.0001 {break;}
        budget=(budget-distance).max(0.);point=next;
    }
    (point!=from).then_some(point)
}
/// Spawn groups from world rules carry this bit, so a live edit can replace
/// exactly them and leave captured and inferred groups alone.
pub const OPERATOR_GROUP: u64 = 1 << 63;

/// Native creature tables plus the operator rules: everything needed to build or
/// re-rule a creature after startup.
#[derive(Clone)]
struct Bestiary {
    rules: BTreeMap<u32, Rule>,
    native_rules: BTreeMap<u32, Rule>,
    regional_native: bool,
    ai: BTreeMap<u32, crate::creature_ai::Profile>,
    labels: BTreeMap<u32, String>,
    protected: BTreeSet<u32>,
    spawn_policies: BTreeMap<u32, crate::spawners::Policy>,
    world: std::sync::Arc<crate::world_rules::Rules>,
}
/// Development policy, test servers only: `PYREALIUM_DEV_CREATURE_HEALTH` (a whole number, 1 or more)
/// caps every creature's health, so a live test's new character can finish a hunt's kills.
/// Unset, creatures keep their records' health.
fn dev_creature_health() -> Option<u32> {
    std::env::var("PYREALIUM_DEV_CREATURE_HEALTH").ok().and_then(|v| v.parse::<u32>().ok()).filter(|h| *h >= 1)
}

impl Bestiary {
    /// The rule, AI and patrol a placement gets, or None when it is no creature.
    fn ruled(&self, entity: u32, id: u64) -> Option<(Rule, crate::creature_ai::Profile, Option<crate::world_rules::Patrol>)> {
        let custom = self.world.stats_for(entity, id);
        // Operator stats can make any placed non-service object a creature.
        let regional=matches!(id & 0xffffffffff000000, 0x8140c00000000000 | 0x8140c10000000000 | 0x8140d30000000000);
        let selected=if self.regional_native && regional {&self.native_rules}else{&self.rules};
        let mut rule = selected.get(&entity).copied().or_else(|| custom.and_then(|s| Some(Rule {
            health: s.health?, damage: s.damage?, level: None, vigor: None, armor: 0, interval: 1500,
            range: 3.0, respawn: 30_000, xp: 0, gold: 0, leash: 18. })))?;
        let behaviour = self.world.behaviour_for(entity, id);
        let ai = behaviour.apply(self.ai.get(&entity).copied().unwrap_or_default());
        if ai.homesick.is_none() { rule.leash = rule.leash.max(ai.perception); }
        if let Some(radius) = behaviour.homesick.filter(|r| *r > 0.) { rule.leash = radius; }
        if let Some(s) = custom { rule = apply_stats(rule, s); }
        Some((rule, ai, self.world.patrols.get(&id).cloned()))
    }
    fn actor(&self, object: &crate::content::Object) -> Option<Actor> {
        // Authored dialogue roles override inherited combat data: the
        // training mentor's template also carries a monster profile.
        // Use the same identities as the interaction dispatcher.
        if object.service.is_some()
            || self.protected.contains(&object.entity)
            || object.entity == crate::training_dialogue::MENTOR_ENTITY
            // Quest NPCs and things players use (`quest_emotes` Use sets) are not combat actors.
            || crate::quest_emotes::tables().is_some_and(|t| t.reacts(object.entity, object.id, crate::quest_emotes::Trigger::Use))
        {
            return None;
        }
        let (mut rule, ai, patrol) = self.ruled(object.entity, object.id)?;
        if let Some(cap) = dev_creature_health() { rule.health = rule.health.min(cap); }
        Some(Actor {
            entity: object.entity,
            name: object.name.clone().or_else(|| self.labels.get(&object.entity).cloned()),
            home: object.placement,
            pursuit: object.supports_pose_updates(),
            movement: crate::creature_motion::Profile::default(),
            velocity: 0.,
            route: Vec::new(),
            rule,
            health: rule.health,
            vigor: rule.vigor,
            target: None,
            next_attack: 0,
            next_ability: 0,
            ai, next_idle: object.id % 1000, wander_goal: None, evading: false, random: object.id,
            patrol, patrol_next: 0, patrol_wait: 0, engaged_at: None,
        })
    }
    /// Operator groups own their members' timers; operator respawn values win
    /// over development policies for the targeted type or placement.
    fn policy(&self, id: u64, actor: &Actor) -> crate::spawners::Policy {
        let custom = self.world.stats_for(actor.entity, id).and_then(|s| s.respawn_ms);
        let group = self.world.groups.iter().find(|g| g.members.contains(&id)).and_then(|g| g.regeneration_ms);
        let mut policy = self.spawn_policies.get(&actor.entity).copied().unwrap_or(crate::spawners::Policy {
            initial_delay_ms: 0,
            regeneration_ms: if matches!(id & 0xffffffffff000000, 0x8140c00000000000 | 0x8140c10000000000) { 180000 } else if id & 0xffffff0000000000 == 0x8140a00000000000 || id & 0xffffffffff000000 == 0x8140d30000000000 { 120000 } else { actor.rule.respawn } });
        if let Some(ms) = group.or(custom) { policy.regeneration_ms = ms; }
        policy
    }
    fn operator_groups(&self, actors: &BTreeMap<u64, Actor>) -> Vec<(u64, usize, Vec<u64>)> {
        // Members that are not placed creatures (deleted or services) are skipped.
        self.world.groups.iter().map(|g| (g.id, g.capacity,
            g.members.iter().copied().filter(|m| actors.contains_key(m)).collect())).collect()
    }
}

fn apply_stats(mut rule: Rule, s: crate::world_rules::Stats) -> Rule {
    if let Some(v) = s.health { rule.health = v; }
    if let Some(v) = s.damage { rule.damage = v; }
    if let Some(v) = s.armor { rule.armor = v; }
    if let Some(v) = s.level { rule.level = Some(v).filter(|level| *level > 0); }
    if let Some(v) = s.xp { rule.xp = v; }
    if let Some(v) = s.gold { rule.gold = v; }
    if let Some(v) = s.interval_ms { rule.interval = v; }
    if let Some(v) = s.range { rule.range = v; }
    if let Some(v) = s.leash { rule.leash = v; }
    if let Some(v) = s.respawn_ms { rule.respawn = v; }
    rule
}
impl Encounters {
    pub fn initial_health(&self) -> impl Iterator<Item = (u64, u32)> + '_ {
        self.actors
            .iter()
            .map(|(&id, actor)| (id, actor.rule.health))
    }
    pub fn inspection_stats(&self) -> impl Iterator<Item = (u64, u32, u32)> + '_ {
        self.actors.iter().flat_map(|(&id, actor)| {
            [(0x12e,actor.rule.level),(0x108,actor.rule.vigor),(0x103,actor.vigor)]
                .into_iter().filter_map(move |(stat,value)|value.map(|v|(id,stat,v)))
        })
    }
    pub fn spawners(&self) -> &crate::spawners::Spawners { &self.spawners }
    #[cfg(test)]
    pub(crate) fn spawners_mut_for_test(&mut self) -> &mut crate::spawners::Spawners { &mut self.spawners }
    /// Called before any client can observe the scene. Only managed monsters
    /// are withdrawn; vendors, dialogue actors and world objects remain placed.
    pub fn initialize_spawners(&mut self,scene:&mut Scene,now:u64)->io::Result<()> {
        for (&id,actor) in &mut self.actors {
            if scene.content_placement(id).is_some(){scene.despawn_content(id)?;}
            actor.health=0;actor.target=None;actor.route.clear();
            actor.patrol_next=0;actor.patrol_wait=0;actor.engaged_at=None;
        }
        self.spawners.initialize(now);
        Ok(())
    }
    /// Acknowledge only after Scene has created the new network lifetime.
    pub fn spawned(&mut self,id:u64)->io::Result<()> {
        if !self.spawners.spawned(id){return Err(invalid("unexpected spawner child"));}
        let actor=self.actors.get_mut(&id).ok_or_else(||invalid("unknown spawner child"))?;
        actor.health=actor.rule.health;actor.vigor=actor.rule.vigor;actor.target=None;actor.route.clear();actor.next_attack=0;
        actor.velocity=0.;
        actor.evading=false;actor.wander_goal=None;
        self.burns.retain(|b|b.target!=id);
        self.ratings.retain(|(who,_,_,_)|*who!=id);
        self.ledger.retain(|e|e.owner!=id);
        actor.patrol_next=0;actor.patrol_wait=0;actor.engaged_at=None;
        Ok(())
    }
    pub fn removed(&mut self,id:u64,now:u64) { self.spawners.removed(id,now); }
    /// Holds quest spawn slots (a live edit's new ones): they spawn only when summoned.
    pub fn hold_slots(&mut self,ids:impl Iterator<Item=u64>) {
        for id in ids.filter(|id|self.actors.contains_key(id)) {self.spawners.hold(id);}
    }
    /// A held slot's creature is due now (`quest_emotes` Generate): it comes as the spawner's
    /// Respawn. False when the slot has a creature or one coming.
    pub fn summon(&mut self,id:u64,now:u64)->bool { self.actors.contains_key(&id) && self.spawners.summon(id,now) }
    /// Whether the creature `id` is alive (not a corpse, not waiting to spawn).
    pub fn alive(&self,id:u64)->bool { self.actors.get(&id).is_some_and(|a|a.health>0) }
    /// Whether a held slot has no creature in the world and none coming.
    pub fn dormant(&self,id:u64)->bool { self.spawners.dormant(id) }
    /// A held slot's creature (alive or a corpse) leaves the world: the Despawn to carry out.
    /// Its attackers stop.
    pub fn dismiss(&mut self,id:u64)->Option<Event> {
        let action=self.spawners.dismiss(id)?;
        let crate::spawners::Action::Remove(id)=action else {return None};
        let attackers=self.attacks.iter().filter(|(_,a)|a.target==id).map(|(&p,_)|p).collect::<Vec<_>>();
        for player in attackers {self.attacks.remove(&player);self.pending.push(Event::Stop(player));}
        if let Some(actor)=self.actors.get_mut(&id) {actor.health=0;actor.target=None;actor.route.clear();}
        Some(Event::Despawn(id))
    }
    pub fn stop(&mut self, player: u64) {
        self.attacks.remove(&player);
    }
    pub fn attacking_target(&self, player: u64) -> Option<u64> {
        self.attacks.get(&player).map(|attack| attack.target)
    }
    pub fn is_creature(&self, id: u64) -> bool {
        self.actors.contains_key(&id)
    }
    /// The chance, in whole percent, that `player`'s basic melee attack hits the living creature
    /// `target`: the attack and defense ratings the swing rolls with (`tick`: the player's
    /// `combat::player_attack` for melee with their rating effects, against the creature's level
    /// times eight with its defense effects) through `combat::hit_chance`. Pyrealium policy: the
    /// retail server's formula is not known, so the number shown is this server's own roll.
    pub fn hit_percent(&self, store: &Store, player: u64, target: u64) -> Option<u32> {
        let actor = self.actors.get(&target).filter(|actor| actor.health > 0)?;
        let caster = store.characters.get(&player)?;
        let school = crate::combat::School::Melee;
        let rating = crate::combat::adjusted(
            crate::combat::player_attack(caster.progression.level().number, &caster.skills, school),
            self.ratings_for(player).attack(school),
        );
        let defense = crate::combat::adjusted(
            actor.rule.level.unwrap_or(1) * 8,
            self.ratings_for(target).creature_defense(),
        );
        Some((crate::combat::hit_chance(rating, defense) * 100.0).round() as u32)
    }
    /// Local test navigation to actual runtime actors, never a spawn generator.
    /// Use current positions and exclude dead/despawned actors and all rooms.
    pub fn outdoor_survey(&self, scene: &Scene, origin: Placement) -> Vec<String> {
        if !(1..=64).contains(&(origin.cell & 0xffff)) {
            return vec!["Go outdoors to locate imported world creatures.".into()];
        }
        let mut nearby=self.actors.iter().filter_map(|(&id,actor)| {
            if actor.health==0 {return None;}
            let point=scene.content_placement(id)?;
            if !(1..=64).contains(&(point.cell & 0xffff)) {return None;}
            Some((origin.distance(point)?,id,point,actor.name.as_deref().unwrap_or("Creature")))
        }).collect::<Vec<_>>();
        nearby.sort_by(|a,b|a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let close=nearby.iter().filter(|row|row.0<=300.).count();
        let mut lines=vec![format!("Imported outdoor creatures: {} alive; {close} within 300 m. {} managed point spawners ({} live across all areas). Nearest locations (local testing):",nearby.len(),self.spawners.len(),self.spawners.live())];
        for (distance,_,point,name) in nearby.into_iter().take(5) {
            // Same native land-cell coordinates as the client map (20 m per
            // cell, 1020 at the origin, 10.2 cells per compass unit).
            let east=((point.cell>>24) as f32*8.+point.coordinates[0]/20.-1020.)/10.2;
            let north=(((point.cell>>16)&255) as f32*8.+point.coordinates[1]/20.-1020.)/10.2;
            lines.push(format!("{name}: {:.1}{}, {:.1}{} ({distance:.0} m away; cell {:08X}).",
                north.abs(),if north>=0. {'N'}else{'S'},east.abs(),if east>=0. {'E'}else{'W'},point.cell));
        }
        lines
    }
    pub fn includes_entity(&self, entity: u32) -> bool {
        self.actors.values().any(|actor| actor.entity == entity)
    }
    /// Call only after the server validates the authored potion's DAT profile.
    pub fn enable_training_loot(&mut self) {
        self.training_loot = true;
    }
    pub fn start(
        &mut self,
        player: u64,
        target: u64,
        now: u64,
        scene: &Scene,
        store: &Store,
    ) -> io::Result<()> {
        let actor = self
            .actors
            .get(&target)
            .filter(|actor| actor.health > 0)
            .ok_or_else(|| invalid("target is not an attackable creature"))?;
        if actor.evading {
            return Err(invalid("This creature is returning to its home."));
        }
        if !store
            .characters
            .get(&player)
            .is_some_and(|c| !c.deleted && c.progression.alive())
        {
            return Err(invalid("dead or unavailable character cannot attack"));
        }
        let category = crate::starter_gear::equipped_category(&store.characters[&player].inventory);
        let range = if category == 0 { self.range } else { 18.0 };
        let distance = scene.combat_distance(player, target)
            .ok_or_else(|| invalid("Target is not in a visible, shared area."))?;
        if distance > range {
            return Err(invalid(&format!(
                "Target is out of attack range ({distance:.1} m away; reach {range:.1} m). Move closer, or return to peace with Z to equip a ranged weapon."
            )));
        }
        if self.attacks.len() >= 64 && !self.attacks.contains_key(&player)
        {
            return Err(invalid("encounter attack capacity"));
        }
        self.cooldowns.retain(|_, until| *until > now);
        if self.cooldowns.len() >= 128 && !self.cooldowns.contains_key(&player) {
            return Err(invalid("attack cooldown capacity"));
        }
        let next = self.cooldowns.get(&player).copied().unwrap_or(now);
        // Repeated attack input must not restart a windup or an airborne shot.
        if let Some(queued) = self.attacks.get_mut(&player).filter(|a| a.target == target && a.category == category) {
            queued.repeat = true;
            return Ok(());
        }
        self.attacks.insert(
            player,
            Attack {
                target,
                next,
                repeat: true,
                category,
                stage: 0,
                started: next,
                skill: false,
                strike: None,
                periodic: false,
            },
        );
        Ok(())
    }
    /// A racial basic attack starts the ordinary repeat chain. Only its first
    /// strike carries the requested skill; follow-ups retain normal cooldowns.
    pub fn basic_skill(
        &mut self,
        player: u64,
        target: u64,
        now: u64,
        scene: &Scene,
        store: &Store,
    ) -> io::Result<()> {
        if self.attacks.get(&player).is_some_and(|a| a.stage != 0) {
            return Err(invalid("An attack is already in progress."));
        }
        self.start(player, target, now, scene, store)?;
        self.attacks.get_mut(&player).unwrap().skill = true;
        Ok(())
    }
    fn stopped(&mut self, player: u64, events: &mut Vec<Event>) {
        self.attacks.remove(&player);
        events.push(Event::Stop(player));
    }
    pub fn strike(&mut self, player:u64, target:u64, now:u64, scene:&Scene, store:&Store, strike:Strike) -> io::Result<()> {
        if !strike.range.is_finite() || strike.range<0. || !self.actors.get(&target).is_some_and(|a|a.health>0 && !a.evading) {
            return Err(invalid("Select a living attackable creature."));
        }
        if !store.characters.get(&player).is_some_and(|c|!c.deleted && c.progression.alive()) {
            return Err(invalid("The caster must be alive."));
        }
        if !scene.combat_distance(player,target).is_some_and(|d|d<=strike.range.max(self.range)) {
            return Err(invalid("Your target is out of skill range."));
        }
        if self.attacks.get(&player).is_some_and(|a|a.stage!=0) { return Err(invalid("Finish the current attack before casting.")); }
        self.admit_effects()?;
        if self.attacks.len()>=64 && !self.attacks.contains_key(&player) {return Err(invalid("Attack capacity."));}
        let repeat=self.attacks.get(&player).is_some_and(|a|a.repeat && a.target==target);
        let category=crate::starter_gear::equipped_category(&store.characters[&player].inventory);
        self.attacks.insert(player,Attack{target,next:now,repeat,category,stage:0,started:now,skill:true,strike:Some(strike),periodic:false});
        Ok(())
    }

}
mod load;
mod live;
mod tick;
mod mends;
