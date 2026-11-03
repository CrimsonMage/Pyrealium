//! The attack hooks of the active skills that fire a missile (`content/skill-missiles.tsv`,
//! `tools/import_skill_missiles.py`: root field 65 of each portal.dat skill record lists the
//! skill's attack hooks; a hook naming an EntityDesc is a missile hook).
//!
//! The server does not make these missiles. The client does: a story on the caster that names the
//! skill (`PhysicsStory` bit 0x10) and a fire-missile hook (`StoryHookData` type 3) with the
//! target and the hook's number has its player script name the missile
//! (`gmCWInterface::GetMissileInformationFromWSL(skill, hook, attacker, target)`), and
//! `StoryManager::FireMissile` makes, places and fires it under its missile manager
//! (`CPhysicsObj::MissilePlaceAndFire`); it detonates on the target client side.
use std::{collections::BTreeMap, sync::OnceLock};

fn table() -> &'static BTreeMap<u32, Vec<u32>> {
    static TABLE: OnceLock<BTreeMap<u32, Vec<u32>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        parse(crate::data_profile::table(
            include_str!("../content/skill-missiles.tsv"),
            include_str!("../content/7230/skill-missiles.tsv"),
        ))
    })
}

fn parse(text: &str) -> BTreeMap<u32, Vec<u32>> {
    let mut hooks: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("pyrealium-skill-missiles-v1"), "skill-missiles.tsv header");
    for line in lines.filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<_> = line.split('\t').collect();
        let parsed = (f.len() == 4)
            .then(|| Some((u32::from_str_radix(f[0], 16).ok()?, f[2].parse::<u32>().ok()?)))
            .flatten();
        let (skill, hook) = parsed.expect("skill-missiles.tsv row");
        hooks.entry(skill).or_default().push(hook);
    }
    hooks
}

/// The first attack hook of `skill` that fires a missile.
pub fn missile_hook(skill: u32) -> Option<u32> {
    table().get(&skill).and_then(|hooks| hooks.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flame_bolt_fires_its_bolt_on_hook_one_and_starter_magic_fires_none() {
        for text in [
            include_str!("../content/skill-missiles.tsv"),
            include_str!("../content/7230/skill-missiles.tsv"),
        ] {
            let hooks = parse(text);
            assert_eq!(hooks.get(&0x59), Some(&vec![1]), "Flame Bolt");
            assert_eq!(hooks.get(&0xc7), Some(&vec![0]), "the human bow attack");
            for starter in [0x60, 0x111, 0x2a0] {
                assert!(!hooks.contains_key(&starter), "{starter:#x}");
            }
        }
    }
}
