//! Server-owned population slots. A captured creature is evidence for one point,
//! not for a retail generator's radius, child mix, density or regeneration time.
//! Each point owns at most one child lifetime (including its lootable corpse).
//! Fixed services and scripted actors never enter this controller.
//! A summoned point (a quest spawn slot, `quest_emotes` Generate) has no child of its own: it
//! waits dormant until it is summoned, and after its child's corpse is removed it is dormant
//! again instead of regenerating.
use std::{cmp::Reverse, collections::{BTreeMap, BinaryHeap}, io};

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub initial_delay_ms: u64,
    pub regeneration_ms: u64,
}
impl Policy {
    pub fn valid(self) -> bool {
        self.initial_delay_ms <= 86_400_000 && (1_000..=86_400_000).contains(&self.regeneration_ms)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Spawn(u64), Remove(u64) }

#[derive(Clone, Copy, Debug)]
enum State {
    Live,
    Waiting { due: u64 },
    Spawning,
    Corpse { remove_at: u64, refill_at: u64 },
    Removing { refill_at: u64 },
    /// A summoned point with no child and nothing scheduled.
    Dormant,
}
impl State {
    fn deadline(self) -> Option<u64> {
        match self {
            Self::Waiting { due } => Some(due),
            Self::Corpse { remove_at, .. } => Some(remove_at),
            _ => None,
        }
    }
}
#[derive(Clone)]
struct Point { policy: Policy, state: State, summoned: bool }
#[derive(Clone)]
struct Group { capacity:usize, members:Vec<u64> }

#[derive(Clone, Default)]
pub struct Spawners {
    points: BTreeMap<u64, Point>,
    // Poll only pending work. Thousands of occupied generators need no timer scan.
    due: BinaryHeap<Reverse<(u64, u64)>>,
    groups:BTreeMap<u64,Group>,
    baseline:BTreeMap<u64,Group>,
    membership:BTreeMap<u64,u64>,
    /// Operator member chances (world_rules::Group::chances), keyed by point.
    chances:BTreeMap<u64,f32>,
    random:u64,
}
impl Spawners {
    /// Adopt the initial content lifetimes. Server bootstrap withdraws those
    /// monsters before exposing the scene, then schedules their initial spawn.
    pub fn new(points: impl Iterator<Item = (u64, Policy)>) -> io::Result<Self> {
        let mut result = Self::default();
        for (id, policy) in points {
            if id == 0 || id == u64::MAX || !policy.valid() || result.points.len() >= 16384
                || result.points.insert(id, Point { policy, state: State::Live, summoned: false }).is_some()
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid or duplicate point spawner"));
            }
        }
        Ok(result)
    }
    pub fn len(&self) -> usize { self.points.len() }
    pub fn group_count(&self) -> usize { self.groups.len() }
    pub fn is_empty(&self) -> bool { self.points.is_empty() }
    pub fn owns(&self, id: u64) -> bool { self.points.contains_key(&id) }
    /// Explicit verified child roster only: capacity never creates coordinates
    /// or duplicates an actor. Several simultaneous children are supported.
    pub fn group(&mut self,id:u64,capacity:usize,members:Vec<u64>)->io::Result<()> {
        self.insert_group(id,capacity,members.clone())?;
        self.baseline.insert(id,Group{capacity,members});
        Ok(())
    }
    fn insert_group(&mut self,id:u64,capacity:usize,members:Vec<u64>)->io::Result<()> {
        let unique=members.iter().copied().collect::<std::collections::BTreeSet<_>>();
        if id==0 || id==u64::MAX || self.groups.contains_key(&id) || capacity==0 || capacity>members.len() || members.len()>64
            || unique.len()!=members.len() || members.iter().any(|id|!self.points.contains_key(id)||self.membership.contains_key(id)) {
            return Err(io::Error::other("invalid spawner group or child roster"));
        }
        for &member in &members{self.membership.insert(member,id);}
        self.groups.insert(id,Group{capacity,members});Ok(())
    }
    fn has_capacity(&self,id:u64)->bool {
        let Some(group)=self.membership.get(&id).and_then(|id|self.groups.get(id)) else{return true};
        group.members.iter().filter(|id|!matches!(self.points[id].state,State::Waiting{..})).count()<group.capacity
    }
    pub fn live(&self) -> usize {
        self.points.values().filter(|p| matches!(p.state, State::Live)).count()
    }
    /// Only at startup, after removing the imported monster lifetimes.
    pub fn initialize(&mut self, now: u64) {
        self.due.clear();
        for (&id, point) in &mut self.points {
            if point.summoned { point.state = State::Dormant; continue; }
            let due = now.saturating_add(point.policy.initial_delay_ms);
            point.state = State::Waiting { due };
            self.due.push(Reverse((due, id)));
        }
    }
    pub fn died(&mut self, id: u64, now: u64, corpse_ms: u64) -> bool {
        let Some(point) = self.points.get_mut(&id) else { return false };
        if !matches!(point.state, State::Live) { return false; }
        let remove_at = now.saturating_add(corpse_ms);
        let refill_at = now.saturating_add(point.policy.regeneration_ms);
        point.state = State::Corpse { remove_at, refill_at };
        self.due.push(Reverse((remove_at, id)));
        true
    }
    /// Acknowledge actual world removal, including a child removed by a script.
    /// Visibility loss, movement, player logout and looting are NOT removal.
    pub fn removed(&mut self, id: u64, now: u64) {
        let Some(point) = self.points.get_mut(&id) else { return };
        let due = match point.state {
            State::Corpse { refill_at, .. } | State::Removing { refill_at } => refill_at.max(now),
            State::Live => now.saturating_add(point.policy.regeneration_ms),
            _ => return, // Repeated notifications must not postpone regeneration.
        };
        if point.summoned { point.state = State::Dormant; return; }
        point.state = State::Waiting { due };
        self.due.push(Reverse((due, id)));
    }
    pub fn spawned(&mut self, id: u64) -> bool {
        let Some(point) = self.points.get_mut(&id) else { return false };
        if !matches!(point.state, State::Spawning) { return false; }
        point.state = State::Live;
        true
    }
    /// Live edit: a new point whose first child is due at `now`.
    pub fn add_point(&mut self, id: u64, policy: Policy, now: u64) -> io::Result<()> {
        if id == 0 || id == u64::MAX || !policy.valid() || self.points.len() >= 16384 || self.points.contains_key(&id) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid or duplicate point spawner"));
        }
        let due = now.saturating_add(policy.initial_delay_ms);
        self.points.insert(id, Point { policy, state: State::Waiting { due }, summoned: false });
        self.due.push(Reverse((due, id)));
        Ok(())
    }
    /// Makes `id` a summoned point: dormant from the next `initialize` (or at once when it has
    /// no child yet), spawning only when [`Self::summon`] asks.
    pub fn hold(&mut self, id: u64) -> bool {
        let Some(point) = self.points.get_mut(&id) else { return false };
        point.summoned = true;
        if matches!(point.state, State::Waiting { .. }) { point.state = State::Dormant; }
        true
    }
    pub fn dormant(&self, id: u64) -> bool { self.points.get(&id).is_some_and(|p| matches!(p.state, State::Dormant)) }
    /// A dormant summoned point's child is due at `now`.
    pub fn summon(&mut self, id: u64, now: u64) -> bool {
        let Some(point) = self.points.get_mut(&id).filter(|p| p.summoned && matches!(p.state, State::Dormant)) else { return false };
        point.state = State::Waiting { due: now };
        self.due.push(Reverse((now, id)));
        true
    }
    /// A summoned point's child (alive or a corpse) leaves the world: `Some(Remove)` for the
    /// caller to carry out (then [`Self::removed`] makes it dormant); one not yet spawned is
    /// dormant at once.
    pub fn dismiss(&mut self, id: u64) -> Option<Action> {
        let point = self.points.get_mut(&id).filter(|p| p.summoned)?;
        match point.state {
            State::Waiting { .. } => { point.state = State::Dormant; None }
            State::Live | State::Corpse { .. } => { point.state = State::Removing { refill_at: 0 }; Some(Action::Remove(id)) }
            _ => None,
        }
    }
    /// Live edit: forget a point and its group membership. Stale heap entries are skipped.
    pub fn remove_point(&mut self, id: u64) {
        self.points.remove(&id);
        if let Some(group) = self.membership.remove(&id) {
            self.leave_group(group, id);
        }
    }
    fn leave_group(&mut self, group: u64, id: u64) {
        let Some(entry) = self.groups.get_mut(&group) else { return };
        entry.members.retain(|member| *member != id);
        entry.capacity = entry.capacity.min(entry.members.len());
        if entry.members.is_empty() { self.groups.remove(&group); }
    }
    /// Live edit: future timers use the new policy; pending deadlines stand.
    pub fn set_policy(&mut self, id: u64, policy: Policy) {
        if let Some(point) = self.points.get_mut(&id).filter(|_| policy.valid()) { point.policy = policy; }
    }
    pub fn policy(&self, id: u64) -> Option<Policy> { self.points.get(&id).map(|point| point.policy) }
    /// Rebuild effective groups from the original rosters and current overrides.
    /// Keep the baseline even when a member is temporarily deleted or overridden.
    /// Group IDs in captured packs also have the high bit: never identify ownership
    /// by that bit alone. A rejected rebuild leaves capacities and timers unchanged.
    pub fn replace_groups(&mut self, tag: u64, groups: Vec<(u64, usize, Vec<u64>)>) -> io::Result<()> {
        let mut next=self.clone();
        next.groups.clear();next.membership.clear();
        let owned=groups.iter().flat_map(|(_,_,m)|m.iter().copied()).collect::<std::collections::BTreeSet<_>>();
        for (&id,group) in &self.baseline {
            let members=group.members.iter().copied().filter(|m|self.points.contains_key(m)&&!owned.contains(m)).collect::<Vec<_>>();
            if !members.is_empty(){next.insert_group(id,group.capacity.min(members.len()),members)?;}
        }
        for (id,capacity,members) in groups {
            if id==0 || id&tag!=0 || id|tag==u64::MAX || capacity==0 {
                return Err(io::Error::other("invalid operator group identity or capacity"));
            }
            let members=members.into_iter().filter(|m|self.points.contains_key(m)).collect::<Vec<_>>();
            if !members.is_empty(){next.insert_group(id|tag,capacity.min(members.len()),members)?;}
        }
        self.groups=next.groups;self.membership=next.membership;
        Ok(())
    }
    /// Live edit or load: the operator groups' member chances.
    pub fn set_chances(&mut self, chances: BTreeMap<u64, f32>) {
        self.chances = chances.into_iter().filter(|(_, c)| c.is_finite() && *c > 0. && *c <= 1.).collect();
    }
    fn unit(&mut self) -> f64 {
        if self.random == 0 {
            let mut seed = [0u8; 8];
            let _ = getrandom::getrandom(&mut seed);
            self.random = u64::from_le_bytes(seed) | 1;
        }
        let mut x = self.random;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.random = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }
    /// ACE WorldObject.SelectAProfile for a free slot: one roll in [0, total) over the
    /// group's chance members that are ready (waiting and due), each taking its share
    /// in roster order. Maxed (alive, corpse) or cooling-down members are left out, so
    /// the shares are relative to what can spawn now, as ACE's GetTotalProbability.
    fn roll(&mut self, id: u64, now: u64) -> u64 {
        if !self.chances.contains_key(&id) { return id; }
        let Some(group) = self.membership.get(&id).and_then(|g| self.groups.get(g)) else { return id };
        let ready = group.members.iter().filter_map(|m| {
            let chance = *self.chances.get(m)?;
            matches!(self.points.get(m)?.state, State::Waiting { due } if due <= now).then_some((*m, f64::from(chance)))
        }).collect::<Vec<_>>();
        let total = ready.iter().map(|(_, c)| c).sum::<f64>();
        let mut roll = self.unit() * total;
        for (member, chance) in &ready {
            if roll < *chance { return *member; }
            roll -= chance;
        }
        ready.last().map_or(id, |(m, _)| *m)
    }
    pub fn poll(&mut self, now: u64) -> Vec<Action> {
        let mut actions = Vec::new();
        while self.due.peek().is_some_and(|Reverse((due, _))| *due <= now) {
            let Reverse((due, id)) = self.due.pop().unwrap();
            if !self.points.contains_key(&id) { continue; } // Removed by a live edit.
            let capacity=self.has_capacity(id);
            let point = self.points.get_mut(&id).unwrap();
            if point.state.deadline() != Some(due) { continue; } // Superseded corpse deadline.
            match point.state {
                State::Waiting { .. } => {
                    if !capacity {
                        let due=now.saturating_add(1000);point.state=State::Waiting{due};self.due.push(Reverse((due,id)));continue;
                    }
                    let chosen = self.roll(id, now);
                    if chosen != id {
                        // Another ready member won this slot; this one waits for the next.
                        let due=now.saturating_add(1000);
                        self.points.get_mut(&id).unwrap().state=State::Waiting{due};self.due.push(Reverse((due,id)));
                    }
                    // The winner's own pending deadline goes stale (deadline() is None while spawning).
                    self.points.get_mut(&chosen).unwrap().state = State::Spawning;
                    actions.push(Action::Spawn(chosen));
                }
                State::Corpse { refill_at, .. } => {
                    point.state = State::Removing { refill_at };
                    actions.push(Action::Remove(id));
                }
                _ => unreachable!(),
            }
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn population() -> Spawners {
        Spawners::new([(7, Policy { initial_delay_ms: 500, regeneration_ms: 3000 }),
            (8, Policy { initial_delay_ms: 0, regeneration_ms: 5000 })].into_iter()).unwrap()
    }
    #[test]
    fn a_summoned_point_spawns_only_when_summoned_and_never_regenerates() {
        let mut p=population();assert!(p.hold(7));p.initialize(0);
        assert_eq!(p.poll(100000),vec![Action::Spawn(8)],"held point stays dormant");
        assert!(p.dormant(7));assert!(p.summon(7,200000));assert!(!p.summon(7,200000));
        assert_eq!(p.poll(200000),vec![Action::Spawn(7)]);assert!(p.spawned(7));
        assert!(p.died(7,201000,60000));assert_eq!(p.poll(261000),vec![Action::Remove(7)]);
        p.removed(7,261000);assert!(p.dormant(7));assert!(p.poll(10_000_000).is_empty(),"no regeneration");
        // Dismissed while alive: removed now, then dormant.
        assert!(p.summon(7,0));p.poll(10_000_001);p.spawned(7);
        assert_eq!(p.dismiss(7),Some(Action::Remove(7)));p.removed(7,10_000_002);assert!(p.dormant(7));
        // Dismissed before it spawned: dormant at once, and the stale deadline is skipped.
        assert!(p.summon(7,20_000_000));assert_eq!(p.dismiss(7),None);assert!(p.poll(20_000_000).is_empty());
        assert_eq!(p.dismiss(8),None,"an ordinary point cannot be dismissed");
    }
    #[test]
    fn overrides_preserve_and_restore_high_bit_native_groups() {
        let mut p=population();
        let baseline=0x8140c00000000100;
        let tag=1<<63;
        p.group(baseline,1,vec![7,8]).unwrap();
        p.replace_groups(tag,vec![]).unwrap();
        assert_eq!(p.membership[&7],baseline);
        p.replace_groups(tag,vec![(1,1,vec![7])]).unwrap();
        assert_eq!(p.membership[&7],tag|1);
        assert_eq!(p.membership[&8],baseline);
        assert!(p.replace_groups(tag,vec![(2,1,vec![7,7])]).is_err());
        assert_eq!(p.membership[&7],tag|1,"failed rebuild is atomic");
        p.remove_point(7);
        p.add_point(7,Policy{initial_delay_ms:0,regeneration_ms:3000},0).unwrap();
        p.replace_groups(tag,vec![]).unwrap();
        assert_eq!(p.membership[&7],baseline);
        p.initialize(0);
        assert_eq!(p.poll(500).len(),1,"original capacity restored after deleting the override");
    }
    #[test]
    fn initial_population_is_delayed_and_capacity_waits_for_world_acknowledgement() {
        let mut p=population();p.initialize(100);
        assert_eq!(p.poll(100),vec![Action::Spawn(8)]);
        assert_eq!(p.live(),0);assert!(p.poll(599).is_empty());
        assert_eq!(p.poll(600),vec![Action::Spawn(7)]);
        assert!(p.poll(100000).is_empty()); // An unacknowledged spawn cannot duplicate.
        assert!(p.spawned(7));assert!(!p.spawned(7));assert!(p.spawned(8));
        assert_eq!(p.live(),2);assert!(p.poll(200000).is_empty());
        assert!(!p.owns(9));assert!(!p.died(9,200000,0));
    }
    #[test]
    fn corpse_and_regeneration_deadlines_are_independent_and_never_overlap_lifetimes() {
        let mut p=population();assert!(p.died(7,1000,60000));
        assert!(!p.died(7,2000,60000)); // Duplicate kill cannot extend timers.
        assert!(p.poll(60999).is_empty());
        assert_eq!(p.poll(61000),vec![Action::Remove(7)]);
        assert!(p.poll(90000).is_empty()); // Must actually remove the corpse first.
        p.removed(7,90000);assert_eq!(p.poll(90000),vec![Action::Spawn(7)]);
        assert!(p.spawned(7));assert_eq!(p.live(),2);
        assert!(p.died(7,91000,500));assert_eq!(p.poll(91500),vec![Action::Remove(7)]);
        p.removed(7,91500);assert!(p.poll(93999).is_empty());
        assert_eq!(p.poll(94000),vec![Action::Spawn(7)]);
    }
    #[test]
    fn early_removal_does_not_leave_a_stale_corpse_event_or_duplicate_child() {
        let mut p=population();p.died(7,1000,60000);p.removed(7,2000);p.removed(7,2500);
        assert_eq!(p.poll(4000),vec![Action::Spawn(7)]);assert!(p.spawned(7));
        assert!(p.poll(61000).is_empty());assert_eq!(p.live(),2);
        p.removed(7,62000);p.removed(7,63000);
        assert_eq!(p.poll(65000),vec![Action::Spawn(7)]);assert!(p.spawned(7));
    }
    #[test]
    fn restart_reinitializes_each_slot_once_and_rejects_invalid_owners() {
        let mut p=population();p.died(7,1000,60000);p.initialize(2000);
        assert_eq!(p.poll(2000),vec![Action::Spawn(8)]);p.spawned(8);
        assert_eq!(p.poll(2500),vec![Action::Spawn(7)]);p.spawned(7);
        assert!(p.poll(61000).is_empty());
        let policy=Policy{initial_delay_ms:0,regeneration_ms:3000};
        assert!(Spawners::new([(7,policy),(7,policy)].into_iter()).is_err());
        assert!(Spawners::new([(0,policy)].into_iter()).is_err());
        assert!(!Policy{initial_delay_ms:0,regeneration_ms:0}.valid());
    }
    #[test]
    fn explicit_groups_fill_multiple_children_without_exceeding_capacity() {
        let policy=Policy{initial_delay_ms:0,regeneration_ms:1000};
        let mut p=Spawners::new([7,8,9].into_iter().map(|id|(id,policy))).unwrap();
        p.group(1,2,vec![7,8,9]).unwrap();p.initialize(0);
        assert_eq!(p.poll(0),vec![Action::Spawn(7),Action::Spawn(8)]);
        assert!(p.poll(5000).is_empty(),"unacknowledged children occupy capacity");
        p.spawned(7);p.spawned(8);assert_eq!(p.live(),2);
        p.died(7,5000,1000);assert_eq!(p.poll(6000),vec![Action::Remove(7)]);
        p.removed(7,6000);assert_eq!(p.poll(6000),vec![Action::Spawn(7)]);p.spawned(7);
        assert!(p.poll(10000).is_empty());assert_eq!(p.live(),2);
        assert!(p.group(2,3,vec![7,8]).is_err());assert!(p.group(2,1,vec![10]).is_err());
        assert!(p.group(2,1,vec![7,7]).is_err());
    }
    #[test]
    fn chance_members_share_each_free_slot_like_ace_select_a_profile() {
        let policy=Policy{initial_delay_ms:0,regeneration_ms:1000};
        let mut tally=BTreeMap::<u64,u32>::new();
        let mut p=Spawners::new([7,8].into_iter().map(|id|(id,policy))).unwrap();
        p.random=0x9e37_79b9_7f4a_7c15;
        p.replace_groups(1<<63,vec![(1,1,vec![7,8])]).unwrap();
        p.set_chances(BTreeMap::from([(7,0.75),(8,0.25)]));
        for round in 0..4000u64 {
            let now=round*10_000;
            p.initialize(now);
            let spawned=p.poll(now);
            assert_eq!(spawned.len(),1,"capacity 1: one slot, one roll");
            let Action::Spawn(id)=spawned[0] else{panic!()};
            *tally.entry(id).or_default()+=1;
            p.spawned(id);
        }
        let share=f64::from(tally[&7])/4000.;
        assert!((0.71..0.79).contains(&share),"75% member won {share}");
    }
    #[test]
    fn a_lone_ready_chance_member_always_fills_its_slot() {
        let policy=Policy{initial_delay_ms:0,regeneration_ms:1000};
        let mut p=Spawners::new([7,8].into_iter().map(|id|(id,policy))).unwrap();
        p.replace_groups(1<<63,vec![(1,1,vec![7,8])]).unwrap();
        p.set_chances(BTreeMap::from([(7,0.9),(8,0.1)]));
        p.initialize(0);let Action::Spawn(first)=p.poll(0)[0] else{panic!()};p.spawned(first);
        let other=if first==7{8}else{7};
        p.died(first,10,0);p.poll(10);p.removed(first,10);
        // The cooling-down winner is out of the roll; the other is ready and alone.
        assert_eq!(p.poll(1000),vec![Action::Spawn(other)]);
    }
    #[test]
    fn live_points_and_groups_change_without_disturbing_living_children() {
        let policy=Policy{initial_delay_ms:0,regeneration_ms:1000};
        let mut p=Spawners::new([7,8].into_iter().map(|id|(id,policy))).unwrap();
        p.initialize(0);assert_eq!(p.poll(0).len(),2);p.spawned(7);p.spawned(8);
        p.add_point(9,policy,100).unwrap();assert!(p.add_point(9,policy,100).is_err());
        assert_eq!(p.poll(100),vec![Action::Spawn(9)]);p.spawned(9);
        const TAG:u64=1<<63;
        p.replace_groups(TAG,vec![(1,1,vec![7,8,9])]).unwrap();
        assert_eq!(p.live(),3,"lowering capacity never kills living children");
        p.died(7,200,0);assert_eq!(p.poll(200),vec![Action::Remove(7)]);p.removed(7,200);
        assert!(p.poll(5000).is_empty(),"group full: no refill");
        p.died(8,6000,0);p.poll(6000);p.removed(8,6000);p.died(9,6000,0);p.poll(6000);p.removed(9,6000);
        assert_eq!(p.poll(7000),vec![Action::Spawn(7)],"one refill under capacity 1");
        p.remove_point(8);p.remove_point(9);assert!(p.poll(100000).is_empty(),"stale deadlines are skipped");
        p.replace_groups(TAG,vec![]).unwrap();assert_eq!(p.group_count(),0);
        p.set_policy(7,Policy{initial_delay_ms:0,regeneration_ms:9000});assert_eq!(p.policy(7).unwrap().regeneration_ms,9000);
    }
}
