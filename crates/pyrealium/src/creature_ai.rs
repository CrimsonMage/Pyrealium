//! Inherited DAT controls, with bounded Pyrealium scheduling where scripts are absent.
use std::{collections::BTreeMap,io};
#[derive(Clone,Copy,Debug,Default)]
pub struct Profile {
    pub perception:f32, pub homesick:Option<f32>, pub wandering:Option<bool>,
    pub wander_range:f32, pub probability:Option<f32>, pub friend:u32,
    pub free_attack:Option<bool>, pub idle_only:bool, pub loot_profile:u32, pub loot_proof:bool,
    /// Operator world-rule choice; replaces the native relationship heuristic.
    pub custom_aggressive:Option<bool>,
}
impl Profile {
    pub fn aggressive(self)->bool {
        if let Some(choice)=self.custom_aggressive {return choice && self.perception>0. && !self.idle_only;}
        // The relationship scripts are not present. Generic combat creatures
        // use their native perception; explicitly peaceful/pet roles do not.
        self.perception>0. && self.free_attack!=Some(false) && !self.idle_only &&
            !matches!(self.friend,0|14|23|24|25|27|28|29|30|31|32|35|37|38|45|48|55|56|59|60|61|79)
    }
    pub fn wanders(self)->bool {
        !self.idle_only && self.wandering!=Some(false) && self.wander_range>0. && self.probability!=Some(0.)
    }
}
pub fn catalog()->io::Result<BTreeMap<u32,Profile>> {
    let mut result=BTreeMap::new();let bad=||io::Error::other("invalid native creature AI profile");
    for line in crate::content_db::files::text("creature-ai.tsv").lines().skip(1)
        .chain(crate::content_db::files::text("inferred-creature-ai-v1.tsv").lines().skip(1))
        .chain(crate::content_db::files::text("regional-creature-ai-v1.tsv").lines().skip(1))
        .chain(crate::content_db::files::text("wilderness-creature-ai-v1.tsv").lines().skip(1))
        .chain(crate::content_db::files::text("arwic-mines-creature-ai-v1.tsv").lines().skip(1))
        .filter(|l|!l.starts_with('#') && !l.is_empty()) {
        let f=line.split('\t').collect::<Vec<_>>();if f.len()!=11{return Err(bad());}
        let number=|i:usize|->io::Result<Option<f32>> {
            if f[i]=="-"{return Ok(None)}
            let n=f[i].parse::<f32>().map_err(|_|bad())?;
            if !n.is_finite() || !(0.0..=10000.).contains(&n){return Err(bad())}Ok(Some(n))
        };
        let boolean=|i:usize|->io::Result<Option<bool>> {match number(i)?{None=>Ok(None),Some(0.)=>Ok(Some(false)),Some(1.)=>Ok(Some(true)),_=>Err(bad())}};
        let p=Profile{perception:number(1)?.unwrap_or(0.),homesick:number(2)?,wandering:boolean(3)?,
            wander_range:number(4)?.unwrap_or(0.),probability:number(5)?,friend:number(6)?.unwrap_or(0.) as u32,
            free_attack:boolean(7)?,idle_only:boolean(8)?.unwrap_or(false),loot_profile:number(9)?.unwrap_or(0.) as u32,loot_proof:boolean(10)?.unwrap_or(false),custom_aggressive:None};
        if p.probability.is_some_and(|v|v>1.) || p.perception>500. || p.wander_range>500. {return Err(bad())}
        let id=u32::from_str_radix(f[0],16).map_err(|_|bad())?;
        result.entry(id).or_insert(p);
    }
    Ok(result)
}
pub fn random(state:&mut u64)->f32 {
    *state=state.wrapping_add(0x9e3779b97f4a7c15);let mut z=*state;
    z=(z^(z>>30)).wrapping_mul(0xbf58476d1ce4e5b9);z=(z^(z>>27)).wrapping_mul(0x94d049bb133111eb);
    ((z^(z>>31))>>40) as f32 / 16777216.
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn native_controls_preserve_disabled_and_absent_wandering() {
        let profiles=catalog().unwrap();let beetle=profiles[&0x470008bf];assert_eq!(beetle.perception,25.);assert!(beetle.aggressive());assert!(!beetle.wanders());
        assert!(profiles.values().any(|p|p.wanders()));
        for p in profiles.values().filter(|p|p.wandering==Some(false)){assert!(!p.wanders());}
        assert!(!Profile{friend:35,perception:30.,..Default::default()}.aggressive());
    }
}
