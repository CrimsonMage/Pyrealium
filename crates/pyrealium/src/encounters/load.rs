//! Loading the encounter profile and the creatures' native movement (`Encounters::load`).
use super::*;

impl Encounters {
    pub fn load_motion(&mut self,content:&Content,directory:&Path)->io::Result<()> {
        let mut dat=crate::dat::Dat::open(directory.join("portal.dat"))?;
        let mut catalog=crate::creature_motion::Catalog::default();
        let store=ac2_db::ResourceStore::new(ac2_dat::DatDataset::open(ac2_dat::DatOpenOptions::new(directory)).map_err(io::Error::other)?);
        let mut rest=BTreeMap::<(u32,u32),f32>::new();
        for (&id,actor) in &mut self.actors {
            let object=content.object(id).ok_or_else(||invalid("Movement actor missing content"))?;
            actor.movement=catalog.resolve(&mut dat,object.visual,object.scale)?
                .ok_or_else(||io::Error::other(format!("Missing native movement for creature {id:016x}, visual {:08x}",object.visual)))?;
            let key=(object.visual,object.scale.to_bits());
            let height=match rest.get(&key) {Some(h)=>*h,None=>{let h=crate::creature_motion::rest_height(&store,object.visual,object.scale)?;rest.insert(key,h);h}};
            actor.movement.rest_height=height;
        }
        self.native_motion=true;
        Ok(())
    }
    pub fn movement_profile(&self,id:u64)->Option<crate::creature_motion::Profile> {
        self.actors.get(&id).map(|a|a.movement)
    }
    pub fn load(path: &Path, content: &Content) -> io::Result<Self> {
        Self::load_with_rules(path, content, &crate::world_rules::Rules::default())
    }
    pub fn load_with_rules(path: &Path, content: &Content, rules: &crate::world_rules::Rules) -> io::Result<Self> {
        if path == Path::new("training") {
            return Self::parse_profile(
                crate::content_db::files::text("development-encounters.tsv"),
                content,
                true,
                rules,
            );
        }
        let mut bytes = Vec::new();
        File::open(path)?.take(65537).read_to_end(&mut bytes)?;
        if bytes.len() > 65536 {
            return Err(invalid("encounter profile exceeds 64 KiB"));
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid("encounter profile encoding"))?;
        Self::parse_with_rules(text, content, rules)
    }
    #[cfg(test)]
    pub(super) fn parse(text: &str, content: &Content) -> io::Result<Self> {
        Self::parse_with_rules(text, content, &crate::world_rules::Rules::default())
    }
    pub(super) fn parse_with_rules(text: &str, content: &Content, world_rules: &crate::world_rules::Rules) -> io::Result<Self> {
        Self::parse_profile(text, content, false, world_rules)
    }
    pub(super) fn parse_profile(text: &str, content: &Content, regional_native: bool, world_rules: &crate::world_rules::Rules) -> io::Result<Self> {
        if text.len() > 65536 {
            return Err(invalid("encounter profile exceeds 64 KiB"));
        }
        let mut lines = text.lines();
        if lines.next() != Some("pyrealium-encounters-v1") {
            return Err(invalid("encounter profile header"));
        }
        let mut player = None;
        let mut regen = None;
        let mut rules = BTreeMap::new();
        let mut spawn_policies = BTreeMap::new();
        let mut spawn_groups=Vec::new();
        for line in lines {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.len() > if line.starts_with("spawn-group\t") { 2048 } else { 512 } {
                return Err(invalid("encounter row length"));
            }
            let f: Vec<_> = line.split('\t').collect();
            let number = |i: usize| {
                f[i].parse::<u64>()
                    .map_err(|_| invalid("invalid encounter integer"))
            };
            let small = |i: usize| {
                u32::try_from(number(i)?).map_err(|_| invalid("encounter integer overflow"))
            };
            let range = |i: usize| -> io::Result<f32> {
                let value = f[i]
                    .parse::<f32>()
                    .map_err(|_| invalid("encounter range"))?;
                if !value.is_finite() || !(0.5..=30.0).contains(&value) {
                    return Err(invalid("encounter range bounds"));
                }
                Ok(value)
            };
            match f.as_slice() {
                ["spawn-group",_,_,_] => {
                    let id=u64::from_str_radix(f[1],16).map_err(|_|invalid("spawner group identity"))?;
                    let capacity=number(2)? as usize;
                    let members=f[3].split(',').map(|id|u64::from_str_radix(id,16).map_err(|_|invalid("spawner child identity")))
                        .collect::<io::Result<Vec<_>>>()?;
                    spawn_groups.push((id,capacity,members));
                }
                ["spawner", _, _, _] => {
                    let entity=u32::from_str_radix(f[1].trim_start_matches("0x"),16)
                        .map_err(|_|invalid("spawner entity"))?;
                    let policy=crate::spawners::Policy{initial_delay_ms:number(2)?,regeneration_ms:number(3)?};
                    if entity>>24!=0x47 || !policy.valid() || spawn_policies.len()>=4096
                        || spawn_policies.insert(entity,policy).is_some() {
                        return Err(invalid("invalid or duplicate spawner policy"));
                    }
                }
                ["player", _, _, _, _] if player.is_none() => {
                    player = Some((small(1)?, small(2)?, number(3)?, range(4)?));
                }
                ["regen", _, _, _] if regen.is_none() => {
                    regen = Some((small(1)?, small(2)?, number(3)?));
                }
                ["npc", _, _, _, _, _, _, _, _] => {
                    let entity = u32::from_str_radix(f[1].trim_start_matches("0x"), 16)
                        .map_err(|_| invalid("encounter entity"))?;
                    let rule = Rule {
                        health: small(2)?,
                        level: None,
                        vigor: None,
                        damage: small(3)?,
                        armor: 0,
                        interval: number(4)?,
                        range: range(5)?,
                        respawn: number(6)?,
                        xp: number(7)?,
                        gold: small(8)?,
                        leash: 18.,
                    };
                    if entity >> 24 != 0x47
                        || rule.health == 0
                        || rule.health > 1_000_000
                        || rule.damage == 0
                        || rule.damage > 1_000_000
                        || !(250..=60000).contains(&rule.interval)
                        || !(1000..=86400000).contains(&rule.respawn)
                        || rule.xp > i64::MAX as u64
                        || rule.gold > i32::MAX as u32
                        || rules.len() >= 256
                        || rules.insert(entity, rule).is_some()
                    {
                        return Err(invalid("invalid or duplicate encounter rule"));
                    }
                }
                _ => return Err(invalid("unrecognized encounter row")),
            }
        }
        let (damage, cost, interval, range) =
            player.ok_or_else(|| invalid("missing player attack profile"))?;
        let (regen_health, regen_vigor, regen_interval) = regen.unwrap_or((0, 0, 1000));
        if damage == 0
            || damage > 1_000_000
            || cost == 0
            || cost > 1_000_000
            || !(250..=60000).contains(&interval)
            || regen_health > 1_000_000
            || regen_vigor > 1_000_000
            || !(1000..=60000).contains(&regen_interval)
        {
            return Err(invalid("player or regeneration profile bounds"));
        }
        // An authored row wins: the retail Idol Spawn is level 24 and one-hits a
        // level 1 character. Interval 0 means the record had no natural weapon
        // time. A catalog entry alone is not evidence that an NPC is attackable.
        // Only explicit encounter rules or recovered combat records qualify.
        let mut native_rules=BTreeMap::new();
        for line in crate::content_db::files::text("creature-combat.tsv").lines()
            .chain(crate::content_db::files::text("inferred-creature-combat-v1.tsv").lines())
            .chain(crate::content_db::files::text("regional-creature-combat-v1.tsv").lines())
            .chain(crate::content_db::files::text("wilderness-creature-combat-v1.tsv").lines())
            .chain(crate::content_db::files::text("arwic-mines-creature-combat-v1.tsv").lines()) {
            if line.is_empty() || line.starts_with('#') || line.starts_with("pyrealium-") {
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            if ![7,13,14].contains(&f.len()) {
                return Err(invalid("creature combat field count"));
            }
            let entity = u32::from_str_radix(f[0], 16).map_err(|_| invalid("creature combat entity"))?;
            let number = |i: usize| {
                f[i].parse::<u64>()
                    .map_err(|_| invalid("creature combat integer"))
            };
            let small = |i: usize| {
                u32::try_from(number(i)?).map_err(|_| invalid("creature combat overflow"))
            };
            let health = small(1)?;
            let damage = small(2)?;
            let mut interval = number(3)?;
            if interval == 0 {
                interval = 1500;
            }
            let armor = small(4)?;
            let xp = number(5)?;
            if entity >> 24 != 0x47
                || health == 0
                || health > 5_000_000
                || damage == 0
                || damage > 5_000_000
                || armor > 1_000_000
                || !(250..=60000).contains(&interval)
                || xp > i64::MAX as u64
            {
                return Err(invalid("creature combat bounds"));
            }
            let rule = Rule {
                health,
                level: Some(small(6)?).filter(|level| *level>0),
                vigor: if f.len()==14 && !f[13].is_empty(){Some(small(13)?)}else{None},
                damage,
                armor,
                interval,
                range: 3.0,
                respawn: 30_000,
                xp,
                gold: 2,
                leash: if f.len()>=13 {let value=f[11].parse::<f32>().map_err(|_|invalid("native homesick radius"))?;
                    if !value.is_finite() || !(0.0..=500.0).contains(&value){return Err(invalid("native homesick radius bounds"));}
                    if value>0. {value}else{18.}}else{18.},
            };
            native_rules.entry(entity).or_insert(rule);
            if rules.contains_key(&entity) {
                continue;
            }
            if rules.len() >= 4096 || rules.insert(entity, rule).is_some() {
                return Err(invalid("creature combat rule limit"));
            }
        }
        // Catalog labels are presentation only; unlike the old fallback list,
        // they must never confer health, rewards or attackability on an NPC.
        let labels=crate::content_db::files::text("definitions.tsv").lines().filter_map(|line|{
            let mut fields=line.split('\t');let entity=u32::from_str_radix(fields.next()?,16).ok()?;
            fields.next()?;let name=fields.next()?;
            Some((entity,name.split('[').next().unwrap_or(name).trim().to_owned()))
        }).collect::<BTreeMap<_,_>>();
        let protected=crate::content_db::files::text("population-protected.txt").lines()
            .filter_map(|line|u32::from_str_radix(line,16).ok()).collect::<std::collections::BTreeSet<_>>();
        let bestiary=Bestiary{rules,native_rules,regional_native,ai:crate::creature_ai::catalog()?,labels,protected,spawn_policies,
            world:std::sync::Arc::new(world_rules.clone())};
        let actors=content.objects().filter_map(|object|bestiary.actor(object).map(|actor|(object.id,actor)))
            .collect::<BTreeMap<_,_>>();
        if actors.is_empty() || actors.len() > 16384 {
            return Err(invalid("encounters require 1..16384 matching placed actors"));
        }
        if bestiary.spawn_policies.keys().any(|entity|!actors.values().any(|actor|actor.entity==*entity)) {
            return Err(invalid("spawner policy must name a placed combat creature"));
        }
        // Captures keep one point spawner per observation. The separate inferred
        // pack caps its authored rosters as groups; services remain excluded.
        let mut spawners=crate::spawners::Spawners::new(actors.iter().map(|(&id,actor)|(id,bestiary.policy(id,actor))))?;
        for (id,capacity,members) in spawn_groups {spawners.group(id,capacity,members)?;}
        for source in [crate::content_db::files::text("inferred-world-v1.report.json"),
            crate::content_db::files::text("regional-world-v1.report.json"),
            crate::content_db::files::text("wilderness-world-v1.report.json"),
            crate::content_db::files::text("hunting-world-v2.report.json"),
            crate::content_db::files::text("arwic-mines-world-v1.report.json")] {
        let inferred:serde_json::Value=serde_json::from_str(source).map_err(io::Error::other)?;
        for group in inferred["groups"].as_array().ok_or_else(||invalid("inferred spawn groups"))? {
            let members=group["members"].as_array().ok_or_else(||invalid("inferred roster"))?.iter().filter_map(|v|v.as_u64()).collect::<Vec<_>>();
            // External content can select only part of the authored pack.
            let members=members.into_iter().filter(|id|actors.contains_key(id)).collect::<Vec<_>>();
            if !members.is_empty(){let capacity=usize::try_from(group["capacity"].as_u64().ok_or_else(||invalid("inferred capacity"))?)
                .map_err(|_|invalid("inferred capacity overflow"))?.min(members.len());
                spawners.group(group["id"].as_u64().ok_or_else(||invalid("inferred group identity"))?,capacity,members)?;}
        }
        }
        spawners.replace_groups(OPERATOR_GROUP,bestiary.operator_groups(&actors))?;
        // Quest spawn slots spawn only when a quest's Generate summons them (`quest_emotes`).
        if let Some(tables)=crate::quest_emotes::tables() {
            for id in tables.slot_iids().filter(|id|actors.contains_key(id)) {spawners.hold(id);}
        }
        Ok(Self {
            native_motion: false,
            actors,
            spawners,
            bestiary,
            pending: Vec::new(),
            attacks: BTreeMap::new(),
            cooldowns: BTreeMap::new(),
            damage,
            cost,
            interval,
            range,
            next_tick: 0,
            regen_health,
            regen_vigor,
            regen_interval,
            next_regen: 0,
            burns: Vec::new(), mends: Vec::new(), regen: BTreeMap::new(), ratings: Vec::new(), ledger: Vec::new(), escorts: Default::default(), next_eid: 0, abilities: crate::abilities::creature_abilities(), drains: Vec::new(),
            training_loot: false,
            shared_xp: BTreeSet::new(),
            dice:crate::combat::Dice::seeded(),
        })
    }
}
