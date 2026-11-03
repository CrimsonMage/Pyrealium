//! Butchering a corpse with a hunting knife: the DAT side (which creature has a butchery profile
//! and the five butchered traits) and the family policy that turns a profile into a trait.
//!
//! What the data gives (portal.dat of 7230):
//! * the tool: the Hunting Knives (`wsl_toolnew_huntingknife_starter`, `wsl_tool_huntingknife_tier1..10`,
//!   `wsl_toolnew_huntingknife`), "double-click on the Hunting Knife, then target a suitable
//!   corpse" (string table 25007d2c); the client's `CUsageSystem` keeps the knife in the usage list
//!   until a corpse is used, then sends `WM_Usage` 0x22 with the knife as item and the corpse as
//!   target (client test `butchery_7160`);
//! * the five Butchered Traits: bone, chitin, hide, sinew, tallow (guide page 25007a50);
//! * a creature's profile: its entity (or the template it inherits from) names a
//!   `*butcheryprofile.wc` record in property `0x410002ce` (`ButcheryProfile_DIDStat`, 304);
//!   there are 67 such records and every one of them is the same empty `0x3532` object, so no
//!   record says what a profile yields;
//! * the messages (string tables 25000014 and 25007ace), the errors (`Usage_Butchery_*`, 10061..)
//!   and the quest text "butcher 100 units of tallow from a monster" (250074b1).
//!
//! What the AC2Hero player guide (2013 mirror, `butcher.html` and `traits.html`) settles: the two
//! traits each creature family gives ([`LISTED`]), that a corpse eight levels or more below the
//! butcher cannot be butchered ([`too_far_below`]), and that a knife wears out (not modelled:
//! no number in the guide and Pyrealium keeps no per-item durability).
//!
//! Policy (retail's server is unknown, the amount is in no record): ten units of each trait a use,
//! a corpse butchers once; a profile the guide does not list gives the one trait of its family
//! (the guide's words: chitin from hard-shelled creatures, bone from the undead and golems, hide
//! from beasts, sinew from the humanoids that bind and string things, tallow from any other
//! monster).
use crate::dat::Dat;
use std::collections::BTreeMap;

/// `Usage_TargetType` (int quality 721) of a body, and the `SkillValidTargets` bit the hunting
/// knives' `Usage_ValidTargetTypes` (722) names: `MonsterItem`, 8.
pub const TARGET_TYPE_QUALITY: u32 = 721;
pub const BUTCHERABLE_TARGET: u32 = 8;
/// The entity property that names a creature's butchery profile.
pub const PROFILE_PROPERTY: u32 = 0x4100_02ce;
/// `wsl_toolnew_huntingknife_starter`, `wsl_tool_huntingknife_tier1..10` and `wsl_toolnew_huntingknife`.
pub fn is_knife(definition: u32) -> bool {
    matches!(definition, 0x4700_2f73 | 0x4700_2c2f..=0x4700_2c38 | 0x4700_2de8)
}
/// The butchered traits' definitions.
pub const TRAITS: [u32; 5] = [0x4700_1400, 0x4700_27a0, 0x4700_27a3, 0x4700_141f, 0x4700_27a7];
/// The butchered traits (`wsl_mote_*` of the same names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trait {
    Bone,
    Chitin,
    Hide,
    Sinew,
    Tallow,
}
impl Trait {
    pub fn definition(self) -> u32 {
        match self {
            Self::Bone => 0x4700_1400,
            Self::Chitin => 0x4700_27a0,
            Self::Hide => 0x4700_27a3,
            Self::Sinew => 0x4700_141f,
            Self::Tallow => 0x4700_27a7,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Bone => "bone",
            Self::Chitin => "chitin",
            Self::Hide => "hide",
            Self::Sinew => "sinew",
            Self::Tallow => "tallow",
        }
    }
}
/// Profiles of the bone family (policy, see the module doc).
const BONE: [u32; 7] = [
    0x6f00_2884, 0x6f00_2885, 0x6f00_2892, 0x6f00_28a3, 0x6f00_28a4, 0x6f00_28a9, 0x6f00_28aa,
];
/// Profiles of the chitin family (policy, see the module doc).
const CHITIN: [u32; 16] = [
    0x6f00_285c, 0x6f00_2878, 0x6f00_2895, 0x6f00_2896, 0x6f00_2897, 0x6f00_2898, 0x6f00_2899,
    0x6f00_289a, 0x6f00_289b, 0x6f00_289c, 0x6f00_289d, 0x6f00_289e, 0x6f00_289f, 0x6f00_28a0,
    0x6f00_28a1, 0x6f00_28a7,
];
/// Profiles of the hide family (policy, see the module doc).
const HIDE: [u32; 18] = [
    0x6f00_2877, 0x6f00_287d, 0x6f00_2881, 0x6f00_2886, 0x6f00_288d, 0x6f00_288f, 0x6f00_2894,
    0x6f00_28a2, 0x6f00_28a5, 0x6f00_28a8, 0x6f00_28ad, 0x6f00_28ae, 0x6f00_28af, 0x6f00_3d3b,
    0x6f00_3d3c, 0x6f00_3d3d, 0x6f00_3d3e, 0x6f00_3d40,
];
/// Profiles of the sinew family (policy, see the module doc).
const SINEW: [u32; 19] = [
    0x6f00_2879, 0x6f00_287a, 0x6f00_287b, 0x6f00_287c, 0x6f00_287e, 0x6f00_287f, 0x6f00_2880,
    0x6f00_2882, 0x6f00_2883, 0x6f00_2887, 0x6f00_2888, 0x6f00_2889, 0x6f00_288c, 0x6f00_288e,
    0x6f00_2890, 0x6f00_2893, 0x6f00_28a6, 0x6f00_28ac, 0x6f00_28bc,
];

/// The traits a butchery profile yields, from AC2Hero's "Butchery Traits Listing"
/// (`traits.html`, the table by TheWizardMoriah): the two traits it lists for the creature, by the
/// creature each `*butcheryprofile.wc` record is named for. A row of dashes (golems of the
/// uncharneled kind) yields nothing. Profiles the page does not list (the big monster, the
/// falatacot, grievver, kallikan, levannath, slithis and urkhad, the lodestone and test profiles)
/// keep the single trait of the family policy below.
pub const LISTED: &[(u32, &[Trait])] = {
    use Trait::*;
    &[
        (0x6f00_285c, &[Chitin, Tallow]),      // armordillo
        (0x6f00_2877, &[Bone, Hide]),          // banderling
        (0x6f00_2878, &[Chitin, Sinew]),       // beetle
        (0x6f00_2879, &[Hide, Tallow]),        // burun guard
        (0x6f00_287a, &[Hide, Tallow]),        // burun leader
        (0x6f00_287b, &[Hide, Tallow]),        // burun minion
        (0x6f00_287c, &[Bone, Hide]),          // crone
        (0x6f00_287d, &[Sinew, Tallow]),       // darkenfowl
        (0x6f00_287e, &[Hide, Sinew]),         // drudge mage
        (0x6f00_287f, &[Hide, Sinew]),         // drudge warrior
        (0x6f00_28bc, &[Hide, Sinew]),         // drudge worker
        (0x6f00_2880, &[Hide, Tallow]),        // eli
        (0x6f00_2881, &[Bone, Sinew]),         // ero-bal
        (0x6f00_2882, &[Bone, Sinew]),         // flayer
        (0x6f00_2883, &[Hide, Tallow]),        // gigurath
        (0x6f00_2884, &[Bone]),                // shattered bone golem
        (0x6f00_2885, &[]),                    // golem
        (0x6f00_2886, &[Hide, Tallow]),        // grutt
        (0x6f00_2887, &[Bone, Hide]),          // gurog mage
        (0x6f00_2888, &[Bone, Hide]),          // gurog minion
        (0x6f00_2889, &[Bone, Hide]),          // gurog soldier
        (0x6f00_288a, &[Bone, Hide]),          // player class monsters
        (0x6f00_288b, &[Bone, Hide]),          // player class monsters (shreth)
        (0x6f00_288c, &[Hide, Sinew]),         // lasher
        (0x6f00_288d, &[Chitin, Tallow]),      // mimbu
        (0x6f00_288e, &[Bone, Tallow]),        // moarsman
        (0x6f00_288f, &[Bone, Hide]),          // monouga
        (0x6f00_2890, &[Hide, Tallow]),        // mosswart
        (0x6f00_2891, &[Chitin, Tallow]),      // mucor
        (0x6f00_2892, &[Bone, Sinew]),         // mu-miyah
        (0x6f00_2893, &[Chitin, Tallow]),      // nefane
        (0x6f00_2894, &[Chitin, Tallow]),      // niffis
        (0x6f00_2895, &[Chitin, Sinew]),       // olthoi noble
        (0x6f00_2896, &[Chitin, Sinew]),       // olthoi soldier
        (0x6f00_2897, &[Chitin, Sinew]),       // olthoi warrior
        (0x6f00_2898, &[Chitin, Sinew]),       // olthoi worker
        (0x6f00_2899, &[Chitin, Tallow]),      // olthoi grub worker
        (0x6f00_289a, &[Chitin, Tallow]),      // olthoi grub noble
        (0x6f00_289b, &[Chitin, Tallow]),      // olthoi grub soldier
        (0x6f00_289c, &[Chitin, Tallow]),      // olthoi grub warrior
        (0x6f00_289d, &[Chitin, Sinew]),       // olthoi nymph noble
        (0x6f00_289e, &[Chitin, Sinew]),       // olthoi nymph soldier
        (0x6f00_289f, &[Chitin, Sinew]),       // olthoi nymph warrior
        (0x6f00_28a0, &[Chitin, Sinew]),       // olthoi nymph worker
        (0x6f00_28a1, &[Chitin, Sinew]),       // phyntos wasp
        (0x6f00_28a2, &[Hide, Sinew]),         // rabbit
        (0x6f00_28a3, &[Bone]),                // undead warrior
        (0x6f00_28a4, &[Bone, Sinew]),         // reaper
        (0x6f00_28a5, &[Bone, Chitin]),        // reedshark
        (0x6f00_28a6, &[Bone, Sinew]),         // sclavus
        (0x6f00_28a7, &[Bone, Sinew]),         // scraven
        (0x6f00_28a8, &[Hide, Tallow]),        // shreth
        (0x6f00_28a9, &[Bone]),                // skeleton
        (0x6f00_28aa, &[Bone]),                // skeleton rider
        (0x6f00_28ab, &[Sinew, Tallow]),       // spawn
        (0x6f00_28ac, &[Bone, Hide]),          // thornling
        (0x6f00_28ad, &[Chitin, Tallow]),      // tyrant
        (0x6f00_28ae, &[Bone, Hide]),          // vermin
        (0x6f00_28af, &[Hide, Tallow]),        // warath
    ]
};
/// The traits a use of the knife gives from a corpse with `profile`: AC2Hero's listed pair, else
/// the one trait of the family policy ([`trait_of`]). Empty for a creature the page lists with
/// dashes.
pub fn traits_of(profile: u32) -> Vec<Trait> {
    LISTED
        .iter()
        .find(|(p, _)| *p == profile)
        .map_or_else(|| vec![trait_of(profile)], |(_, traits)| traits.to_vec())
}

/// AC2Hero, `butcher.html`: a corpse eight or more levels below the butcher's level is refused.
pub const LEVEL_GAP: u32 = 8;
/// Whether a corpse of level `creature` is too far below a butcher of level `level`
/// (AC2Hero `butcher.html`: "less than (or exactly) eight levels below your character level").
pub fn too_far_below(level: u32, creature: u32) -> bool {
    level >= creature + LEVEL_GAP
}

/// The trait a butchery profile yields.
pub fn trait_of(profile: u32) -> Trait {
    if BONE.contains(&profile) {
        Trait::Bone
    } else if CHITIN.contains(&profile) {
        Trait::Chitin
    } else if HIDE.contains(&profile) {
        Trait::Hide
    } else if SINEW.contains(&profile) {
        Trait::Sinew
    } else {
        Trait::Tallow
    }
}

/// Every creature entity with a butchery profile, by the profile (own or inherited from its
/// template chain: word 3 of an entity record names the entity it extends).
pub fn profiles(dat: &mut Dat) -> BTreeMap<u32, u32> {
    let mut own: BTreeMap<u32, (Option<u32>, Option<u32>)> = BTreeMap::new();
    for entity in 0x4700_0000u32..0x4700_4800 {
        let Ok(bytes) = dat.read(entity) else { continue };
        let words: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let profile = words.windows(2).find(|w| w[0] == PROFILE_PROPERTY && w[1] >> 24 == 0x6f).map(|w| w[1]);
        let parent = words.get(3).copied().filter(|p| p >> 24 == 0x47);
        own.insert(entity, (profile, parent));
    }
    let mut found = BTreeMap::new();
    for (&entity, _) in &own {
        let mut at = entity;
        for _ in 0..8 {
            let Some(&(profile, parent)) = own.get(&at) else { break };
            if let Some(profile) = profile {
                found.insert(entity, profile);
                break;
            }
            match parent {
                Some(p) => at = p,
                None => break,
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_corpse_eight_levels_below_the_butcher_is_refused() {
        assert!(!too_far_below(9, 2));
        assert!(too_far_below(10, 2));
        assert!(too_far_below(30, 22));
        assert!(!too_far_below(29, 22));
    }

    #[test]
    fn the_listed_creatures_give_ac2heros_traits() {
        // AC2Hero traits.html: Spawn gives sinew and tallow, Banderling bone and hide, Skeleton
        // bone, Golem nothing; a profile the page omits keeps the family policy's one trait.
        assert_eq!(traits_of(0x6f00_28ab), vec![Trait::Sinew, Trait::Tallow]);
        assert_eq!(traits_of(0x6f00_2877), vec![Trait::Bone, Trait::Hide]);
        assert_eq!(traits_of(0x6f00_28a9), vec![Trait::Bone]);
        assert!(traits_of(0x6f00_2885).is_empty());
        assert_eq!(traits_of(0x6f00_3d3b), vec![Trait::Hide]);
        let mut seen = std::collections::BTreeSet::new();
        for (profile, traits) in LISTED {
            assert!(seen.insert(*profile), "{profile:08x} listed twice");
            assert!(traits.len() <= 2);
        }
    }

    #[test]
    fn the_families_are_disjoint_and_default_to_tallow() {
        for p in HIDE {
            assert!(!BONE.contains(&p) && !CHITIN.contains(&p) && !SINEW.contains(&p));
        }
        assert_eq!(trait_of(0x6f00_2877), Trait::Hide);
        assert_eq!(trait_of(0x6f00_28ab), Trait::Tallow);
        assert_eq!(trait_of(0x6f00_28a9), Trait::Bone);
        assert!(is_knife(0x4700_2f73) && !is_knife(0x4700_2f72));
    }
}
