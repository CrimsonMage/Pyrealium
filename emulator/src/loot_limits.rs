//! Native bounds on generic death-loot quantity. Trophies and quest grants are
//! independent. Selection within the bounds is reconstruction, not a recovered
//! retail distribution. Quality overrides are preserved by the importer only.
use crate::dat::Dat;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io};

#[derive(Clone, Copy, Deserialize)]
pub(crate) struct Quantity {
    pub minimum: u8,
    pub maximum: u8,
    pub level: u16,
}
impl Quantity {
    pub fn roll(self, random: &mut u64) -> usize {
        let span = usize::from(self.maximum - self.minimum) + 1;
        usize::from(self.minimum) + (crate::creature_ai::random(random) * span as f32) as usize
    }
}
#[derive(Default)]
pub(crate) struct Limits(pub BTreeMap<u32, Quantity>);
impl Limits {
    pub fn load(dat: &mut Dat) -> io::Result<Self> {
        Self::parse(include_str!("../content/native-loot-limits.json"), |did| dat.read(did))
    }
    fn parse(text: &str, mut read: impl FnMut(u32) -> io::Result<Vec<u8>>) -> io::Result<Self> {
        #[derive(Deserialize)]
        struct Rule { entity: u32, #[serde(flatten)] quantity: Quantity, sources: BTreeMap<String, String> }
        #[derive(Deserialize)]
        struct File { schema: u32, rules: Vec<Rule> }
        let file: File = serde_json::from_str(text)?;
        if file.schema != 1 || file.rules.len() > 16384 { return Err(io::Error::other("invalid loot limits catalog")); }
        let mut result = Self::default();
        let mut hashes = BTreeMap::new();
        let mut rejected = 0;
        for rule in file.rules {
            if rule.entity >> 24 != 0x47 || rule.quantity.minimum > rule.quantity.maximum || rule.quantity.maximum > 32 || !(1..=150).contains(&rule.quantity.level)
                || rule.sources.len() > 32 || !rule.sources.contains_key("34000000")
                || !rule.sources.contains_key(&format!("{:08x}", rule.entity)) {
                return Err(io::Error::other("invalid native loot quantity/source"));
            }
            let compatible = rule.sources.iter().all(|(key, expected)| {
                let Ok(did) = u32::from_str_radix(key, 16) else { return false };
                let actual = hashes.entry(did).or_insert_with(|| read(did).ok().map(|raw| format!("{:x}", Sha256::digest(raw))));
                actual.as_ref() == Some(expected)
            });
            if !compatible { rejected += 1; continue; }
            if result.0.insert(rule.entity, rule.quantity).is_some() { return Err(io::Error::other("duplicate loot limits")); }
        }
        println!("Native loot quantity: {} compatible creature rules; {rejected} incompatible rules withheld.", result.0.len());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_mismatch_withholds_rules_and_invalid_bounds_are_rejected() {
        let hash = format!("{:x}", Sha256::digest(b"native"));
        let file = serde_json::json!({"schema":1,"rules":[{"entity":0x47000001u32,"minimum":3,"maximum":6,"level":56,
            "sources":{"47000001":hash,"34000000":hash}}]});
        let limits = Limits::parse(&file.to_string(), |_| Ok(b"native".to_vec())).unwrap();
        assert_eq!(limits.0.len(), 1);
        assert!(Limits::parse(&file.to_string(), |_| Ok(b"other build".to_vec())).unwrap().0.is_empty());
        let mut bad = file.clone(); bad["rules"][0]["maximum"] = 2.into();
        assert!(Limits::parse(&bad.to_string(), |_| Ok(b"native".to_vec())).is_err());
        bad["rules"][0]["maximum"] = 33.into();
        assert!(Limits::parse(&bad.to_string(), |_| Ok(b"native".to_vec())).is_err());
    }
    #[test]
    fn bounded_rolls_include_both_endpoints_and_zero() {
        let mut random = 12345;
        for (minimum, maximum) in [(0, 0), (0, 1), (1, 3), (3, 6), (7, 10)] {
            let quantity = Quantity { minimum, maximum, level: 1 };
            let values = (0..1000).map(|_| quantity.roll(&mut random)).collect::<std::collections::BTreeSet<_>>();
            assert_eq!(values, (usize::from(minimum)..=usize::from(maximum)).collect());
        }
    }
}
