//! Session fellowships and the native fellowship messages.
//!
//! The retail exchange each request makes is the 2005 captures' (`third_party/ac2-captures`,
//! Legions-era `.elog`): create -> snapshot (`0x2B`) and "You create the '<NAME>' Fellowship.";
//! invite -> invitation (`0x2E`) and "(Waiting for recruit ...)"; accept -> "You join the
//! Fellowship.", snapshot, "You have joined the '<NAME>' Fellowship." to the recruit and add-fellow
//! (`0x27`), its vitals and "<NAME> has joined your Fellowship." to the others; leave -> clear
//! (`0x28`) and "You leave your Fellowship." to the leaver, remove-fellow (`0x26`) and "<NAME> has
//! left your Fellowship." to the others; dismiss -> remove-fellow and "You dismiss <NAME> from the
//! fellowship." to the leader; disband -> clear and "Your Fellowship has been disbanded.". Lines
//! are the client's own (string table `0x250004E2`, chat type 8; refusals type 1). A level change
//! is `0x32`; an invitation that lapses is `0x31` to the recruit (`FellowshipSystem` 0x37 shows
//! "Your offer to join the Fellowship has expired.").
//!
//! The flags word (field 2 of the `Fellowship` package) is the looting method's bit
//! (`1 << method`, none for method 0; `Fellowship` 0x2E reads it back) and bit 0 while experience
//! is shared (`Fellowship` 0x12), as every captured create/snapshot pair has it. Experience
//! sharing is `fellowship_xp`. Membership is session state.
//! Native adapters: 004682c0..00468810, 0060c1a0/0060c230. Interpreted data
//! packages 1f0/1ee/1ef are checked against the supplied WLib by the caller.
use crate::{script, world, world::WorldError};
use std::collections::{BTreeMap, BTreeSet};

pub const CREATE: u32 = 0x820c001b;
pub const INVITE: u32 = 0x820c0015;
pub const ACCEPT: u32 = 0x820c000d;
pub const DECLINE: u32 = 0x820c001a;
pub const LEAVE: u32 = 0x820c001c;
pub const DISBAND: u32 = 0x820c0003;
pub const DISMISS: u32 = 0x820c000e;
pub const SNAPSHOT: u32 = 0x820c002b;
pub const INVITATION: u32 = 0x820c002e;
pub const CLEAR: u32 = 0x820c0028;
/// `WM_Fellowship` 0x27 -> `CPlayer` 0x8A: one fellow joined (its `Fellow` graph, then its id).
pub const ADD: u32 = 0x820c0027;
/// `WM_Fellowship` 0x26 -> `CPlayer` 0xF4: one fellow left (its id).
pub const REMOVE: u32 = 0x820c0026;
/// `WM_Fellowship` 0x32 -> `CPlayer` 0xBC: a fellow's level (level, id).
pub const LEVEL: u32 = 0x820c0032;
/// `WM_Fellowship` 0x31 -> `CPlayer` 0x39 -> `FellowshipSystem` 0x37: the invitation lapsed.
pub const EXPIRED: u32 = 0x820c0031;
pub const LAYOUTS: [(usize, u32); 3] = [(0x1f0, 8), (0x1ee, 5), (0x1ef, 7)];
/// `FellowshipControl` 0x1's table ends at nine fellows (and the 2004 guide: "up to 9 players").
pub const MAX_MEMBERS: usize = 9;
const MAX_GROUPS: usize = 32;
const MAX_INVITATIONS: usize = 64;
const INVITATION_MS: u64 = 30_000;
const NAMESPACE: u64 = 0x8150000000000000;
/// The looting methods (`ID_Fellowship_LootMethod0..4`: None, Shared, Rotational, Group
/// Rotational, Random).
pub const LOOT_METHODS: u32 = 5;

/// The fellowship lines of string table `0x250004E2`.
pub mod text {
    pub const TABLE: u32 = 0x2500_04e2;
    pub const CREATE: u32 = 0x6fa3_556e;
    pub const WAITING: u32 = 0x3568_94a1;
    pub const JOIN: u32 = 0xf9f5_5e5a;
    pub const JOINED: u32 = 0x89cb_54ff;
    pub const HAS_JOINED: u32 = 0x26f1_33cc;
    pub const LEAVE: u32 = 0xae7a_e1dd;
    pub const HAS_LEFT: u32 = 0x1918_3190;
    pub const DECLINE: u32 = 0xdd3e_74fa;
    pub const DECLINES: u32 = 0x6f25_c372;
    pub const DISMISS: u32 = 0x6d68_5dfa;
    pub const DISMISSED: u32 = 0x3abf_b311;
    pub const HAS_BEEN_DISMISSED: u32 = 0xd02c_56c5;
    pub const DISBANDED: u32 = 0x4c0f_644b;
    pub const ALREADY_IN: u32 = 0x06b0_514e;
    pub const TARGET_ALREADY_IN: u32 = 0x7ccb_9e00;
    pub const NOT_IN: u32 = 0x6b9f_0584;
    pub const NOT_LEADER: u32 = 0x2f29_6eb8;
    pub const NOT_LEADER_DISBAND: u32 = 0x6abe_2b14;
    pub const FULL: u32 = 0x4ab6_b80e;
    pub const NOT_ASKED: u32 = 0xe0d0_2bdf;
    pub const ONLY_MEMBERS: u32 = 0x1623_21c3;
    pub const SELF: u32 = 0x3949_c615;
    pub const UNABLE: u32 = 0xe824_13b9;
    pub const RECRUITING: u32 = 0x1a43_0139;
    pub const NO_NAME: u32 = 0x2b8c_15cf;
    pub const BAD_NAME: u32 = 0x455e_192a;
}

/// A fellowship line (chat type 8, as captured).
fn line(key: u32) -> Result<Vec<u8>, WorldError> {
    let mut args = script::localized(text::TABLE, key)?;
    args.extend(script::integer(8));
    script::private_call(script::CHAT_STRING, &args)
}
/// A fellowship line naming `name`.
fn named(key: u32, name: &str, kind: u32) -> Result<Vec<u8>, WorldError> {
    let mut args = script::localized_name(text::TABLE, key, name)?;
    args.extend(script::integer(kind));
    script::private_call(script::CHAT_STRING, &args)
}
/// A refusal line (chat type 1, as captured for "You must be the leader to disband").
fn refusal(key: u32, name: Option<&str>) -> Result<Vec<u8>, WorldError> {
    match name {
        Some(name) => named(key, name, 1),
        None => {
            let mut args = script::localized(text::TABLE, key)?;
            args.extend(script::integer(1));
            script::private_call(script::CHAT_STRING, &args)
        }
    }
}

/// The fellowship's flags word: the looting method's bit, and bit 0 while experience is shared.
pub fn flags(looting: u32, social: bool) -> u32 {
    let loot = if looting == 0 { 0 } else { 1 << looting };
    loot | u32::from(!social)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Create {
        name: String,
        looting: u32,
        social: bool,
    },
    Invite(u64),
    Accept(u64),
    Decline(u64),
    Leave,
    Disband,
    Dismiss(u64),
}

pub fn handles(function: u32) -> bool {
    matches!(
        function,
        CREATE | INVITE | ACCEPT | DECLINE | LEAVE | DISBAND | DISMISS
    )
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn word(&mut self) -> Result<u32, WorldError> {
        let end = self
            .offset
            .checked_add(4)
            .ok_or(WorldError::LimitExceeded)?;
        let data = self
            .bytes
            .get(self.offset..end)
            .ok_or(WorldError::Truncated)?;
        self.offset = end;
        Ok(u32::from_le_bytes(data.try_into().unwrap()))
    }
    fn expect(&mut self, value: u32) -> Result<(), WorldError> {
        if self.word()? != value {
            return Err(WorldError::InvalidValue);
        }
        Ok(())
    }
    fn int(&mut self) -> Result<u32, WorldError> {
        self.expect(1)?;
        self.word()
    }
    fn iid(&mut self) -> Result<u64, WorldError> {
        self.expect(2)?;
        let value = u64::from(self.word()?) | u64::from(self.word()?) << 32;
        if value == 0 || value == u64::MAX {
            return Err(WorldError::InvalidValue);
        }
        Ok(value)
    }
    fn name(&mut self) -> Result<String, WorldError> {
        self.expect(5)?;
        self.expect(1)?;
        if matches!(self.word()?, 0 | u32::MAX) {
            return Err(WorldError::InvalidValue);
        }
        self.expect(0x44000100)?;
        self.expect(0)?;
        self.expect(0xffff0076)?;
        let tail = self.bytes.get(self.offset..).ok_or(WorldError::Truncated)?;
        let count = u16::from_le_bytes(
            tail.get(..2)
                .ok_or(WorldError::Truncated)?
                .try_into()
                .unwrap(),
        ) as usize;
        if !(1..=32).contains(&count) {
            return Err(WorldError::LimitExceeded);
        }
        let size = (2 + count * 2 + 3) & !3;
        let packed = tail.get(..size).ok_or(WorldError::Truncated)?;
        let mut literal = vec![0; 10];
        literal.extend(1u16.to_le_bytes());
        literal.extend(packed);
        let (name, used) = world::read_literal_name(&literal)?;
        if used != literal.len() || !valid_name(&name) {
            return Err(WorldError::InvalidValue);
        }
        self.offset += size;
        self.expect(0)?;
        Ok(name)
    }
}

pub fn decode(function: u32, bytes: &[u8]) -> Result<Request, WorldError> {
    if bytes.len() > 256 {
        return Err(WorldError::LimitExceeded);
    }
    let mut reader = Reader { bytes, offset: 0 };
    let result = match function {
        CREATE => {
            let looting = reader.int()?;
            let social = match reader.int()? {
                0 => false,
                1 => true,
                _ => return Err(WorldError::InvalidValue),
            };
            if looting >= LOOT_METHODS {
                return Err(WorldError::InvalidValue);
            }
            Request::Create {
                name: reader.name()?,
                looting,
                social,
            }
        }
        INVITE => Request::Invite(reader.iid()?),
        ACCEPT => Request::Accept(reader.iid()?),
        DECLINE => {
            reader.expect(1)?;
            reader.expect(0x138d)?;
            Request::Decline(reader.iid()?)
        }
        LEAVE => Request::Leave,
        DISBAND => Request::Disband,
        DISMISS => Request::Dismiss(reader.iid()?),
        _ => return Err(WorldError::UnsupportedOpcode(function)),
    };
    if reader.offset != bytes.len() {
        return Err(WorldError::TrailingBytes);
    }
    Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: u64,
    pub name: String,
    pub level: u32,
    pub health: u32,
    pub max_health: u32,
    pub vigor: u32,
    pub max_vigor: u32,
    pub cell: u32,
}
impl Member {
    fn valid(&self) -> bool {
        self.id != 0
            && self.id != u64::MAX
            && valid_name(&self.name)
            && (1..=150).contains(&self.level)
            && self.max_health > 0
            && self.max_health <= 1_000_000
            && self.max_vigor > 0
            && self.max_vigor <= 1_000_000
            && self.health <= self.max_health
            && self.vigor <= self.max_vigor
            && crate::dat::world_cell_record(self.cell).is_ok()
    }
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name.encode_utf16().count() <= 32
        && !name.chars().any(char::is_control)
}


#[derive(Clone, Debug)]
struct Group {
    name: String,
    leader: u64,
    members: BTreeSet<u64>,
    flags: u32,
}
#[derive(Clone, Debug)]
struct Invitation {
    group: u64,
    inviter: u64,
    until: u64,
}
#[derive(Clone, Debug)]
pub struct Runtime {
    groups: BTreeMap<u64, Group>,
    membership: BTreeMap<u64, u64>,
    invitations: BTreeMap<u64, Invitation>,
    /// Each member's name as it joined, for the lines sent after it has gone offline.
    names: BTreeMap<u64, String>,
    next: u64,
    revision: u64,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            groups: BTreeMap::new(),
            membership: BTreeMap::new(),
            invitations: BTreeMap::new(),
            names: BTreeMap::new(),
            next: NAMESPACE,
            revision: 0,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Delivery {
    pub recipient: u64,
    pub data: Vec<u8>,
}
pub struct Plan {
    next: Runtime,
    base_revision: u64,
    deliveries: Vec<Delivery>,
}
impl Plan {
    pub fn deliveries(&self) -> &[Delivery] {
        &self.deliveries
    }
}

/// A refused request. Each has the client's own line ([`GroupError::wire`]).
#[derive(Clone, Debug)]
pub enum GroupError {
    InvalidMember,
    AlreadyGrouped,
    TargetGrouped(String),
    NotGrouped,
    NotLeader,
    NotLeaderDisband,
    Capacity,
    Full,
    InvalidTarget,
    NotMember,
    RecruitSelf,
    InvitationMissing,
    InvitationPending(String),
    NoName,
    BadName(String),
    StalePlan,
    Wire,
}
impl GroupError {
    /// The line's key and its name, when the client has one for the refusal.
    fn line(&self) -> Option<(u32, Option<&str>)> {
        Some(match self {
            Self::AlreadyGrouped => (text::ALREADY_IN, None),
            Self::TargetGrouped(name) => (text::TARGET_ALREADY_IN, Some(name.as_str())),
            Self::NotGrouped => (text::NOT_IN, None),
            Self::NotLeader => (text::NOT_LEADER, None),
            Self::NotLeaderDisband => (text::NOT_LEADER_DISBAND, None),
            Self::Full => (text::FULL, None),
            Self::InvalidTarget => (text::UNABLE, None),
            Self::NotMember => (text::ONLY_MEMBERS, None),
            Self::RecruitSelf => (text::SELF, None),
            Self::InvitationMissing => (text::NOT_ASKED, None),
            Self::InvitationPending(name) => (text::RECRUITING, Some(name.as_str())),
            Self::NoName => (text::NO_NAME, None),
            Self::BadName(name) => (text::BAD_NAME, Some(name.as_str())),
            _ => return None,
        })
    }
    /// The refusal as the player sees it: the client's line (chat type 1), else plain text.
    pub fn wire(&self) -> Result<Vec<u8>, WorldError> {
        match self.line() {
            Some((key, name)) => refusal(key, name),
            None => script::chat(self.message(), 1),
        }
    }
    pub fn message(&self) -> &'static str {
        match self {
            Self::AlreadyGrouped => "You are already in a Fellowship.",
            Self::TargetGrouped(_) => "That character is already in a Fellowship.",
            Self::NotGrouped => "You are not in a Fellowship.",
            Self::NotLeader => "You are not the leader of your Fellowship.",
            Self::NotLeaderDisband => "You must be the leader to disband a Fellowship.",
            Self::Full => "Your Fellowship is full.",
            Self::Capacity => "The fellowship limit has been reached.",
            Self::InvalidTarget => "Unable to recruit that character.",
            Self::NotMember => "You can only dismiss members of your Fellowship.",
            Self::RecruitSelf => "You cannot recruit yourself.",
            Self::InvitationMissing => "You have not been asked to join a Fellowship.",
            Self::InvitationPending(_) => "Someone else is already trying to recruit that character.",
            Self::NoName => "You must specify a Fellowship name.",
            Self::BadName(_) => "That Fellowship name contains invalid characters.",
            _ => "The fellowship action could not be completed.",
        }
    }
}
impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for GroupError {}
impl From<WorldError> for GroupError {
    fn from(_: WorldError) -> Self {
        Self::Wire
    }
}

/// A member's fellowship for the experience split (`fellowship_xp`).
pub struct Sharing {
    pub leader: u64,
    pub shared: bool,
    pub members: Vec<u64>,
}

struct Out(Vec<Delivery>);
impl Out {
    fn send(&mut self, recipient: u64, data: Result<Vec<u8>, WorldError>) -> Result<(), GroupError> {
        self.0.push(Delivery { recipient, data: data? });
        Ok(())
    }
}

impl Runtime {
    pub fn contains(&self, player: u64) -> bool {
        self.membership.contains_key(&player)
    }
    pub fn members(&self, player: u64) -> Vec<u64> {
        self.membership
            .get(&player)
            .and_then(|id| self.groups.get(id))
            .map(|group| group.members.iter().copied().collect())
            .unwrap_or_default()
    }
    /// `player`'s fellowship as the experience split needs it.
    pub fn sharing(&self, player: u64) -> Option<Sharing> {
        let group = self.groups.get(self.membership.get(&player)?)?;
        Some(Sharing {
            leader: group.leader,
            shared: group.flags & 1 != 0,
            members: group.members.iter().copied().collect(),
        })
    }
    /// The players whose kills the fellowship shares (members of a sharing fellowship).
    pub fn sharing_players(&self) -> BTreeSet<u64> {
        self.membership
            .iter()
            .filter(|(_, id)| self.groups.get(id).is_some_and(|g| g.flags & 1 != 0))
            .map(|(player, _)| *player)
            .collect()
    }
    pub fn member_ids(&self) -> Vec<u64> {
        self.membership.keys().copied().collect()
    }
    pub fn participant_ids(&self) -> Vec<u64> {
        self.membership
            .keys()
            .chain(self.invitations.keys())
            .copied()
            .chain(self.invitations.values().map(|invite| invite.inviter))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    fn name_of(&self, player: u64, online: &BTreeMap<u64, Member>) -> String {
        online
            .get(&player)
            .map(|m| m.name.clone())
            .or_else(|| self.names.get(&player).cloned())
            .unwrap_or_default()
    }
    pub fn prepare(
        &self,
        actor: u64,
        request: &Request,
        now: u64,
        online: &BTreeMap<u64, Member>,
    ) -> Result<Plan, GroupError> {
        if !online.get(&actor).is_some_and(Member::valid) {
            return Err(GroupError::InvalidMember);
        }
        let mut next = self.clone();
        next.invitations.retain(|_, invite| invite.until > now);
        let mut out = Out(Vec::new());
        match request {
            Request::Create {
                name,
                looting,
                social,
            } => {
                if name.is_empty() {
                    return Err(GroupError::NoName);
                }
                if !valid_name(name) {
                    return Err(GroupError::BadName(name.clone()));
                }
                if next.contains(actor) {
                    return Err(GroupError::AlreadyGrouped);
                }
                if next.groups.len() >= MAX_GROUPS {
                    return Err(GroupError::Capacity);
                }
                next.next = next
                    .next
                    .checked_add(1)
                    .filter(|id| id >> 48 == NAMESPACE >> 48)
                    .ok_or(GroupError::Capacity)?;
                let id = next.next;
                next.groups.insert(
                    id,
                    Group {
                        name: name.clone(),
                        leader: actor,
                        members: BTreeSet::from([actor]),
                        flags: flags(*looting, *social),
                    },
                );
                next.membership.insert(actor, id);
                next.names.insert(actor, online[&actor].name.clone());
                next.invitations.remove(&actor);
                out.send(actor, next.snapshot_of(id, online))?;
                out.send(actor, named(text::CREATE, name, 8))?;
            }
            Request::Invite(target) => {
                if *target == actor {
                    return Err(GroupError::RecruitSelf);
                }
                let id = *next.membership.get(&actor).ok_or(GroupError::NotGrouped)?;
                let group = next.groups.get(&id).ok_or(GroupError::NotGrouped)?;
                if group.leader != actor {
                    return Err(GroupError::NotLeader);
                }
                let Some(recruit) = online.get(target).filter(|m| m.valid()) else {
                    return Err(GroupError::InvalidTarget);
                };
                if next.contains(*target) {
                    return Err(GroupError::TargetGrouped(recruit.name.clone()));
                }
                if next.invitations.contains_key(target) {
                    return Err(GroupError::InvitationPending(recruit.name.clone()));
                }
                if next.invitations.len() >= MAX_INVITATIONS {
                    return Err(GroupError::Capacity);
                }
                let reserved = next
                    .invitations
                    .values()
                    .filter(|invite| invite.group == id)
                    .count();
                if group.members.len() + reserved >= MAX_MEMBERS {
                    return Err(GroupError::Full);
                }
                out.send(*target, invitation(&online[&actor].name, &group.name, actor))?;
                out.send(actor, line(text::WAITING))?;
                next.invitations.insert(
                    *target,
                    Invitation {
                        group: id,
                        inviter: actor,
                        until: now.saturating_add(INVITATION_MS),
                    },
                );
            }
            Request::Accept(inviter) => {
                if next.contains(actor) {
                    return Err(GroupError::AlreadyGrouped);
                }
                let invite = next
                    .invitations
                    .get(&actor)
                    .filter(|invite| invite.inviter == *inviter)
                    .cloned()
                    .ok_or(GroupError::InvitationMissing)?;
                let group = next
                    .groups
                    .get_mut(&invite.group)
                    .filter(|group| group.leader == *inviter)
                    .ok_or(GroupError::InvitationMissing)?;
                if group.members.len() >= MAX_MEMBERS {
                    return Err(GroupError::Full);
                }
                let existing: Vec<u64> = group.members.iter().copied().collect();
                group.members.insert(actor);
                let fellowship = group.name.clone();
                next.membership.insert(actor, invite.group);
                next.names.insert(actor, online[&actor].name.clone());
                next.invitations.remove(&actor);
                let joined = &online[&actor];
                out.send(actor, line(text::JOIN))?;
                out.send(actor, next.snapshot_of(invite.group, online))?;
                out.send(actor, named(text::JOINED, &fellowship, 8))?;
                for fellow in existing {
                    out.send(fellow, add(joined))?;
                    for packet in vitals(joined)? {
                        out.send(fellow, Ok(packet))?;
                    }
                    out.send(fellow, named(text::HAS_JOINED, &joined.name, 8))?;
                }
            }
            Request::Decline(inviter) => {
                if next
                    .invitations
                    .get(&actor)
                    .is_none_or(|invite| invite.inviter != *inviter)
                {
                    return Err(GroupError::InvitationMissing);
                }
                next.invitations.remove(&actor);
                out.send(actor, line(text::DECLINE))?;
                if online.contains_key(inviter) {
                    out.send(*inviter, named(text::DECLINES, &online[&actor].name, 8))?;
                }
            }
            Request::Leave => next.depart(actor, Departure::Left, online, &mut out)?,
            Request::Disband => {
                let id = *next.membership.get(&actor).ok_or(GroupError::NotGrouped)?;
                if next.groups[&id].leader != actor {
                    return Err(GroupError::NotLeaderDisband);
                }
                next.disband(id, None, online, &mut out)?;
            }
            Request::Dismiss(target) => {
                let id = *next.membership.get(&actor).ok_or(GroupError::NotGrouped)?;
                if next.groups[&id].leader != actor {
                    return Err(GroupError::NotLeader);
                }
                if *target == actor || next.membership.get(target) != Some(&id) {
                    return Err(GroupError::NotMember);
                }
                next.depart(*target, Departure::Dismissed { by: actor }, online, &mut out)?;
            }
        }
        next.finish(self.revision, out)
    }
    /// The players gone from the world leave their fellowships (a leader's is disbanded), and
    /// their invitations lapse.
    pub fn prepare_disconnect(
        &self,
        actor: u64,
        now: u64,
        online: &BTreeMap<u64, Member>,
    ) -> Result<Plan, GroupError> {
        let mut next = self.clone();
        let mut out = Out(Vec::new());
        next.invitations.retain(|target, invite| {
            *target != actor
                && invite.inviter != actor
                && invite.until > now
                && online.contains_key(target)
                && online.contains_key(&invite.inviter)
        });
        // Several sockets can fail in one poll. Remove every unavailable member
        // before encoding snapshots, otherwise each removal could be blocked by
        // a different disconnected member's missing public state.
        let unavailable = next
            .membership
            .keys()
            .copied()
            .filter(|id| *id == actor || !online.get(id).is_some_and(Member::valid))
            .collect::<Vec<_>>();
        for player in unavailable {
            if next.contains(player) {
                next.depart(player, Departure::Left, online, &mut out)?;
            }
        }
        next.finish(self.revision, out)
    }
    /// The invitations whose time ran out: each recruit's dialog closes (`0x31`, the client's
    /// "Your offer to join the Fellowship has expired."). `None` when none lapsed.
    pub fn prepare_expiry(&self, now: u64, online: &BTreeMap<u64, Member>) -> Result<Option<Plan>, GroupError> {
        let lapsed: Vec<(u64, u64)> = self
            .invitations
            .iter()
            .filter(|(_, invite)| invite.until <= now)
            .map(|(target, invite)| (*target, invite.inviter))
            .collect();
        if lapsed.is_empty() {
            return Ok(None);
        }
        let mut next = self.clone();
        let mut out = Out(Vec::new());
        for (target, inviter) in lapsed {
            next.invitations.remove(&target);
            if online.contains_key(&target) {
                out.send(target, script::private_call(EXPIRED, &script::identity(inviter)))?;
            }
        }
        next.finish(self.revision, out).map(Some)
    }
    /// A member leaves (or is dismissed); a leader's departure disbands the fellowship (no new
    /// leader: the client's lines have none).
    fn depart(
        &mut self,
        player: u64,
        how: Departure,
        online: &BTreeMap<u64, Member>,
        out: &mut Out,
    ) -> Result<(), GroupError> {
        let id = *self.membership.get(&player).ok_or(GroupError::NotGrouped)?;
        if self.groups[&id].leader == player {
            return self.disband(id, Some(player), online, out);
        }
        let name = self.name_of(player, online);
        let group = self.groups.get_mut(&id).ok_or(GroupError::NotGrouped)?;
        group.members.remove(&player);
        let others: Vec<u64> = group.members.iter().copied().collect();
        self.membership.remove(&player);
        self.invitations.remove(&player);
        self.names.remove(&player);
        if online.contains_key(&player) {
            out.send(player, script::private_call(CLEAR, &[]))?;
            out.send(player, line(match how {
                Departure::Left => text::LEAVE,
                Departure::Dismissed { .. } => text::DISMISSED,
            }))?;
        }
        for fellow in others.into_iter().filter(|f| online.contains_key(f)) {
            out.send(fellow, script::private_call(REMOVE, &script::identity(player)))?;
            out.send(fellow, match how {
                Departure::Left => named(text::HAS_LEFT, &name, 8),
                Departure::Dismissed { by } if by == fellow => named(text::DISMISS, &name, 8),
                Departure::Dismissed { .. } => named(text::HAS_BEEN_DISMISSED, &name, 8),
            })?;
        }
        Ok(())
    }
    /// Every member's fellowship is cleared; `leaving` (a leader who left) is told it left.
    fn disband(
        &mut self,
        id: u64,
        leaving: Option<u64>,
        online: &BTreeMap<u64, Member>,
        out: &mut Out,
    ) -> Result<(), GroupError> {
        if let Some(group) = self.groups.remove(&id) {
            for player in group.members {
                self.membership.remove(&player);
                self.names.remove(&player);
                if online.contains_key(&player) {
                    out.send(player, script::private_call(CLEAR, &[]))?;
                    out.send(player, line(if leaving == Some(player) { text::LEAVE } else { text::DISBANDED }))?;
                }
            }
        }
        self.invitations.retain(|_, invite| invite.group != id);
        Ok(())
    }
    fn finish(mut self, base_revision: u64, out: Out) -> Result<Plan, GroupError> {
        self.revision = base_revision.checked_add(1).ok_or(GroupError::Capacity)?;
        Ok(Plan {
            next: self,
            base_revision,
            deliveries: out.0,
        })
    }
    fn snapshot_of(&self, id: u64, online: &BTreeMap<u64, Member>) -> Result<Vec<u8>, WorldError> {
        let group = self.groups.get(&id).ok_or(WorldError::InvalidValue)?;
        let members = group
            .members
            .iter()
            .map(|id| online.get(id).filter(|m| m.valid()).cloned().ok_or(WorldError::InvalidValue))
            .collect::<Result<Vec<_>, _>>()?;
        snapshot(&group.name, group.leader, group.flags, &members)
    }
    pub fn commit(&mut self, plan: Plan) -> Result<Vec<Delivery>, GroupError> {
        if self.revision != plan.base_revision {
            return Err(GroupError::StalePlan);
        }
        *self = plan.next;
        Ok(plan.deliveries)
    }
    /// The whole fellowship again, to every member (a member's name changed).
    pub fn refresh(
        &self,
        player: u64,
        online: &BTreeMap<u64, Member>,
    ) -> Result<Vec<Delivery>, GroupError> {
        let Some(&id) = self.membership.get(&player) else {
            return Ok(Vec::new());
        };
        let data = self.snapshot_of(id, online).map_err(|_| GroupError::InvalidMember)?;
        Ok(self.groups[&id]
            .members
            .iter()
            .map(|&recipient| Delivery {
                recipient,
                data: data.clone(),
            })
            .collect())
    }
    pub fn member_updates(
        &self,
        player: u64,
        member: &Member,
    ) -> Result<Vec<Delivery>, GroupError> {
        self.member_changes(player, member, None)
    }
    /// Emit only changed native member fields: a level (`0x32`) and the vitals. A name change
    /// needs a full snapshot; callers must publish an admitted batch before caching it.
    pub fn member_changes(
        &self,
        player: u64,
        member: &Member,
        previous: Option<&Member>,
    ) -> Result<Vec<Delivery>, GroupError> {
        if member.id != player || !member.valid() {
            return Err(GroupError::InvalidMember);
        }
        if previous.is_some_and(|old| !old.valid() || old.id != player || old.name != member.name) {
            return Err(GroupError::InvalidMember);
        }
        let mut packets = Vec::new();
        if previous.is_some_and(|old| old.level != member.level) {
            let mut args = script::integer(member.level);
            args.extend(script::identity(player));
            packets.push(script::private_call(LEVEL, &args)?);
        }
        packets.extend(vital_changes(member, previous)?);
        Ok(self
            .members(player)
            .into_iter()
            .flat_map(|recipient| {
                packets
                    .iter()
                    .cloned()
                    .map(move |data| Delivery { recipient, data })
            })
            .collect())
    }
}

#[derive(Clone, Copy)]
enum Departure {
    Left,
    Dismissed { by: u64 },
}

/// A member's vitals that differ from `previous` (all of them without one). Maxima precede
/// current values so a growing bar never temporarily receives a current value larger than its
/// old maximum.
fn vital_changes(member: &Member, previous: Option<&Member>) -> Result<Vec<Vec<u8>>, WorldError> {
    let mut packets = Vec::new();
    for (function, first, value, old_value) in [
        (0x820c0033, false, member.max_health, previous.map(|old| old.max_health)),
        (0x820c0030, true, member.health, previous.map(|old| old.health)),
        (0x820c0021, false, member.max_vigor, previous.map(|old| old.max_vigor)),
        (0x820c002d, true, member.vigor, previous.map(|old| old.vigor)),
        (0x820c003f, false, member.cell, previous.map(|old| old.cell)),
    ] {
        if old_value == Some(value) {
            continue;
        }
        let mut args = Vec::new();
        if first {
            args.extend(script::integer(0));
        }
        args.extend(script::integer(value));
        args.extend(script::identity(member.id));
        packets.push(script::private_call(function, &args)?);
    }
    Ok(packets)
}
fn vitals(member: &Member) -> Result<Vec<Vec<u8>>, WorldError> {
    vital_changes(member, None)
}

fn invitation(inviter: &str, name: &str, owner: u64) -> Result<Vec<u8>, WorldError> {
    let mut arguments = script::literal(inviter)?;
    arguments.extend(script::literal(name)?);
    arguments.extend(script::identity(owner));
    script::private_call(INVITATION, &arguments)
}

struct Object {
    kind: u32,
    native: Vec<u8>,
    fields: Vec<(u32, u8)>,
}
impl Object {
    fn native(kind: u16, bytes: Vec<u8>) -> Self {
        Self {
            kind: 0xffff0000 | u32::from(kind),
            native: bytes,
            fields: Vec::new(),
        }
    }
    fn interpreted(package: u16, fields: Vec<(u32, u8)>) -> Self {
        Self {
            kind: u32::from(package) << 16,
            native: Vec::new(),
            fields,
        }
    }
}
fn word(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend(value.to_le_bytes());
}
fn graph(objects: &[Object]) -> Vec<u8> {
    let mut bytes = Vec::new();
    word(&mut bytes, 5);
    word(&mut bytes, objects.len() as u32);
    for index in 1..=objects.len() {
        word(&mut bytes, index as u32);
    }
    for object in objects {
        for value in [0x44000100, 0, object.kind] {
            word(&mut bytes, value);
        }
        bytes.extend(&object.native);
        word(&mut bytes, object.fields.len() as u32);
        for &(value, _) in &object.fields {
            word(&mut bytes, value);
        }
        for &(_, tag) in &object.fields {
            bytes.push(tag);
        }
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }
    bytes
}
/// One fellow's objects: `Fellow` (0x1EE: two words the captures fill and Pyrealium leaves at
/// the constructor's 0, the level, and its two parts), `FellowVitals` (0x1EF) and its name.
fn fellow_objects(member: &Member, root: u32, vitals: Vec<(u32, u8)>) -> Result<[Object; 3], WorldError> {
    Ok([
        Object::interpreted(
            0x1ee,
            vec![(0, 0x84), (0, 0xcd), (member.level, 4), (root + 1, 1), (root + 2, 1)],
        ),
        Object::interpreted(0x1ef, vitals),
        Object::native(0x6a, world::literal_name(&member.name)?),
    ])
}
/// `0x27`: the fellow that joined, then its id. As captured, its `FellowVitals` carry only the
/// cell; the vitals follow as their own calls.
fn add(member: &Member) -> Result<Vec<u8>, WorldError> {
    if !member.valid() {
        return Err(WorldError::InvalidValue);
    }
    let vitals = vec![(0, 2), (member.cell, 4), (0, 2), (0, 2), (0, 2), (0, 2), (0, 2)];
    let mut bytes = graph(&fellow_objects(member, 1, vitals)?);
    bytes.extend(script::identity(member.id));
    script::private_call(ADD, &bytes)
}
fn snapshot(name: &str, leader: u64, flags: u32, members: &[Member]) -> Result<Vec<u8>, WorldError> {
    if !valid_name(name)
        || members.is_empty()
        || members.len() > MAX_MEMBERS
        || !members.iter().any(|member| member.id == leader)
    {
        return Err(WorldError::InvalidValue);
    }
    let name_id = 3 + members.len() as u32 * 3;
    // Pinned 7148 WLib: 81f1003d -> 01f00002 constructs the native
    // FellowshipAdaptor from fields 7, 5/6, 2 and 4. Fields 0/1 and 3 are
    // retained at their native 01f0001e constructor defaults (the captures carry a per-server
    // counter in 3 and a changing word in 4); field 2 is the flags word ([`flags`]).
    let mut objects = vec![Object::interpreted(
        0x1f0,
        vec![
            (0, 0x82),
            (0, 0xcd),
            (flags, 2),
            (0, 2),
            (2, 1),
            (leader as u32, 0x84),
            ((leader >> 32) as u32, 0xcd),
            (name_id, 1),
        ],
    )];
    let mut ordered = members.to_vec();
    ordered.sort_unstable_by_key(|member| (member.id % 11, member.id));
    let mut hash = Vec::new();
    word(&mut hash, 0x02000000 | ordered.len() as u32);
    for (index, member) in ordered.iter().enumerate() {
        word(&mut hash, member.id as u32);
        word(&mut hash, (member.id >> 32) as u32);
        word(&mut hash, 3 + index as u32 * 3);
    }
    objects.push(Object::native(0x40, hash));
    for member in ordered {
        if !member.valid() {
            return Err(WorldError::InvalidValue);
        }
        let root = objects.len() as u32 + 1;
        let vitals = vec![
            (member.health, 4),
            (member.cell, 4),
            (member.max_health, 4),
            (0, 4),
            (member.vigor, 4),
            (member.max_vigor, 4),
            (0, 4),
        ];
        objects.extend(fellow_objects(&member, root, vitals)?);
    }
    let literal = world::literal_name(name)?;
    objects.push(Object::native(0x76, literal[12..].to_vec()));
    script::private_call(SNAPSHOT, &graph(&objects))
}

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
