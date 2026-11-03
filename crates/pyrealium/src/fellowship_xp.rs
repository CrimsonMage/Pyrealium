// SPDX-License-Identifier: AGPL-3.0-only
//! A fellowship's share of a member's kill experience.
//!
//! Traced from the 7230 player script (WLib `0x56000005`), the code the client runs to show each
//! fellow's share on the fellowship page (`gmCWInterface::GetFellowXPHash` -> `FellowshipSystem`
//! 0x41):
//! - `Fellowship` 0x12: experience is shared only while bit 0 of the fellowship's flags is set
//!   (the create request's `social` flag clear; see `groups::flags`).
//! - `Fellow` 0xE: a fellow shares when its level is within five of the leader's (either way), or
//!   when both are level 150 or more. `Fellowship` 0x21 counts those fellows.
//! - `FellowshipControl` 0x1: each sharing fellow's part by that count ([`share`]).
//!
//! Not in the client (the server's own functions, `FellowshipControl` 0x4/0x6/0x7, are stripped
//! from the shipped script), so Pyrealium policy (the user, 2026-11-03):
//! - A sharing fellow receives its part only while near the member who earned the kill: both
//!   outdoors within 300 m (the only fellowship range on record, the July 2004 notes' pulse buffs:
//!   "300 meters above-ground or all in the same dungeon"), or both indoors in the same landblock.
//!   A fellow out of range receives nothing; the count stays the level rule's, so the parts the
//!   panel shows are the parts paid.
//! - A member outside the level rule earns its own kills alone, as it would outside a fellowship.

use crate::movement::Placement;

/// `FellowshipControl` 0x1: each sharing fellow's part of the experience, by the number sharing.
pub fn share(sharing: usize) -> f32 {
    match sharing {
        1 => 1.0,
        2 => 0.6,
        3 => 0.45,
        4 => 0.35,
        5 => 0.3,
        6 => 0.25,
        7 => 0.214,
        8 => 0.1875,
        9 => 0.1667,
        _ => 0.1,
    }
}

/// The highest level (`CharacterLevelTable`'s 150 levels).
const LEVEL_CAP: u32 = 150;
/// `Fellow` 0xE's level window.
const LEVEL_WINDOW: u32 = 5;
/// Pyrealium policy: the outdoor range of a share (see the module notes).
pub const OUTDOOR_RANGE: f32 = 300.0;

/// `Fellow` 0xE: whether a fellow of `level` shares under a leader of `leader`.
pub fn eligible(leader: u32, level: u32) -> bool {
    leader.abs_diff(level) <= LEVEL_WINDOW || (leader >= LEVEL_CAP && level >= LEVEL_CAP)
}

/// Pyrealium policy: whether a fellow at `fellow` is near enough to `earner` to share.
pub fn in_range(earner: Placement, fellow: Placement) -> bool {
    let outdoor = |cell: u32| (1..=64).contains(&(cell & 0xffff));
    let indoor = |cell: u32| cell & 0xffff >= 0x100;
    if outdoor(earner.cell) && outdoor(fellow.cell) {
        return earner.distance(fellow).is_some_and(|d| d <= OUTDOOR_RANGE);
    }
    indoor(earner.cell) && indoor(fellow.cell) && earner.cell >> 16 == fellow.cell >> 16
}

/// A fellow as the split needs it.
#[derive(Clone, Copy, Debug)]
pub struct Fellow {
    pub id: u64,
    pub level: u32,
    pub placement: Option<Placement>,
}

/// The experience each fellow receives of `xp` earned by `earner`: the earner alone when it is
/// not sharing (no fellowship, sharing off, outside the level rule), else every sharing fellow
/// in range its part, rounded to the nearest point (the table's parts are `float`s: 0.45 is
/// 0.44999999, so 1000 would floor to 449; the server's rounding is not in the client); a part
/// of 0 is left out.
pub fn split(earner: u64, xp: u64, leader: u64, shared: bool, fellows: &[Fellow]) -> Vec<(u64, u64)> {
    let alone = vec![(earner, xp)];
    let Some(leader_level) = fellows.iter().find(|f| f.id == leader).map(|f| f.level) else {
        return alone;
    };
    let Some(from) = fellows.iter().find(|f| f.id == earner) else {
        return alone;
    };
    if !shared || !eligible(leader_level, from.level) {
        return alone;
    }
    let sharing: Vec<&Fellow> = fellows
        .iter()
        .filter(|f| eligible(leader_level, f.level))
        .collect();
    let part = f64::from(share(sharing.len()));
    sharing
        .into_iter()
        .filter(|f| {
            f.id == earner
                || matches!((from.placement, f.placement), (Some(a), Some(b)) if in_range(a, b))
        })
        .map(|f| (f.id, (xp as f64 * part).round() as u64))
        .filter(|(_, amount)| *amount > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(cell: u32, x: f32) -> Option<Placement> {
        Some(Placement { cell, coordinates: [x, 10.0, 0.0], heading: 0 })
    }
    fn fellow(id: u64, level: u32, placement: Option<Placement>) -> Fellow {
        Fellow { id, level, placement }
    }

    #[test]
    fn the_parts_are_fellowship_controls_table() {
        let table: Vec<f32> = (1..=10).map(share).collect();
        assert_eq!(table, [1.0, 0.6, 0.45, 0.35, 0.3, 0.25, 0.214, 0.1875, 0.1667, 0.1]);
    }

    #[test]
    fn fellows_share_within_five_levels_of_the_leader_or_both_at_the_cap() {
        assert!(eligible(20, 25) && eligible(20, 15) && eligible(20, 20));
        assert!(!eligible(20, 26) && !eligible(20, 14));
        assert!(eligible(150, 150) && eligible(155, 150));
        assert!(!eligible(150, 144));
    }

    #[test]
    fn range_is_300_m_outdoors_and_the_landblock_indoors() {
        let out = at(0x7ab10010, 10.0).unwrap();
        assert!(in_range(out, at(0x7ab10010, 300.0).unwrap()));
        assert!(!in_range(out, at(0x7ab10010, 320.0).unwrap()));
        // The next outdoor landblock east, 160 m on.
        assert!(in_range(out, at(0x7bb10010, 100.0).unwrap()));
        let room = at(0x02990109, 0.0).unwrap();
        assert!(in_range(room, at(0x0299013a, 900.0).unwrap()));
        assert!(!in_range(room, at(0x0298013a, 0.0).unwrap()));
        assert!(!in_range(room, out) && !in_range(out, room));
    }

    #[test]
    fn a_kill_is_split_among_the_sharing_fellows_in_range() {
        let here = at(0x7ab10010, 10.0);
        let fellows = [
            fellow(1, 20, here),
            fellow(2, 24, here),
            fellow(3, 30, here), // outside the level rule
            fellow(4, 16, at(0x7ab10010, 500.0)), // sharing but out of range
        ];
        // Three share (1, 2, 4): 0.45 each; 4 is too far to receive.
        assert_eq!(split(1, 1000, 1, true, &fellows), vec![(1, 450), (2, 450)]);
        // Sharing off, or an earner outside the level rule: alone.
        assert_eq!(split(1, 1000, 1, false, &fellows), vec![(1, 1000)]);
        assert_eq!(split(3, 1000, 1, true, &fellows), vec![(3, 1000)]);
        // Not a member of the list: alone.
        assert_eq!(split(9, 1000, 1, true, &fellows), vec![(9, 1000)]);
    }
}
