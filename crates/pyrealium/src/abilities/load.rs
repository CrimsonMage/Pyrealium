//! Reading the skill records the abilities come from (`Runtime::load`).
use super::*;

impl Runtime {
    pub fn load() -> Result<Self, AbilityError> {
        Self::load_scaled(effect_scale())
    }

    /// The skill records at their own durations, whatever `PYREALIUM_DEV_EFFECT_SCALE` says.
    pub(super) fn load_scaled(scale: f32) -> Result<Self, AbilityError> {
        Self::parse(
            crate::data_profile::table(crate::content_db::files::text("skill-effects.tsv"), crate::content_db::files::text("7230/skill-effects.tsv")),
            crate::data_profile::table(crate::content_db::files::text("skill-travel.tsv"), crate::content_db::files::text("7230/skill-travel.tsv")),
            scale,
        )
    }

    pub(super) fn parse(text: &str, travel: &str, scale: f32) -> Result<Self, AbilityError> {
        let mut routes = BTreeMap::new();
        let mut travel_lines = travel.lines();
        if travel_lines.next() != Some("pyrealium-skill-travel-v1") {
            return Err(AbilityError::Catalog);
        }
        for line in travel_lines {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            if f.len() != 3 {
                return Err(AbilityError::Catalog);
            }
            let skill = u32::from_str_radix(f[0], 16).map_err(|_| AbilityError::Catalog)?;
            let route = match f[2] {
                "lifestone" => Travel::Lifestone,
                "portal" => Travel::Portal,
                list => {
                    let mut pads = Vec::new();
                    for pad in list.split(';') {
                        let p: Vec<_> = pad.split(':').collect();
                        if p.len() != 5 || pads.len() >= 32 {
                            return Err(AbilityError::Catalog);
                        }
                        let number =
                            |text: &str| text.parse::<f32>().map_err(|_| AbilityError::Catalog);
                        let placement = Placement {
                            cell: u32::from_str_radix(p[0], 16)
                                .map_err(|_| AbilityError::Catalog)?,
                            coordinates: [number(p[1])?, number(p[2])?, number(p[3])?],
                            heading: p[4].parse().map_err(|_| AbilityError::Catalog)?,
                        };
                        if !placement.valid() {
                            return Err(AbilityError::Catalog);
                        }
                        pads.push(placement);
                    }
                    Travel::Telepads(pads)
                }
            };
            routes.insert(skill, route);
        }

        let mut lines = text.lines();
        if lines.next() != Some("pyrealium-skill-effects-v2") {
            return Err(AbilityError::Catalog);
        }
        let mut definitions = BTreeMap::new();
        for line in lines {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            if f.len() != 13 {
                return Err(AbilityError::Catalog);
            }
            let hex = |text: &str| u32::from_str_radix(text, 16).map_err(|_| AbilityError::Catalog);
            let number = |text: &str| text.parse::<f32>().map_err(|_| AbilityError::Catalog);
            let skill = hex(f[0])?;
            let kind = hex(f[2])?;
            let reset = number(f[3])?;
            let vigor: i64 = f[4].parse().map_err(|_| AbilityError::Catalog)?;
            let range = number(f[5])?;
            if !(0.0..=3600.0).contains(&reset) || !(-1.0..=1000.0).contains(&range) {
                return Err(AbilityError::Catalog);
            }
            let mut definition = Definition {
                applied: Vec::new(),
                kind,
                reset_ms: (reset * 1000.0) as u64,
                vigor: u32::try_from(vigor.unsigned_abs().min(100_000)).unwrap(),
                // -1 means the record gives no range; strikes then use melee range.
                range: range.max(0.0),
                targets: hex(f[6])?,
                right: hex(f[7])?,
                left: hex(f[8])?,
                story: hex(f[9])?,
                fx: hex(f[10])?,
                damage: Table::parse(f[11])?,
                health: Table::default(),
                vigor_effect: Table::default(),
                timed: Vec::new(),
                buff: None,
                ratings: Vec::new(),
                unresolved: false,
                weapon: false,
                travel: routes.remove(&skill),
            };
            if f[12] != "-" {
                for effect in f[12].split(';') {
                    let p: Vec<_> = effect.split('/').collect();
                    if p.len() != 10 {
                        return Err(AbilityError::Catalog);
                    }
                    let side = if p[0] == "self" {
                        Side::Own
                    } else {
                        Side::Target
                    };
                    let kind = hex(p[2])?;
                    if definition.fx == 0 {
                        definition.fx = hex(p[4])?;
                    }
                    let period = number(p[5])?;
                    let duration = Table::parse(p[6])?.scaled(scale);
                    if !duration.is_empty() {
                        let stats = if p[8] == "-" {
                            Vec::new()
                        } else {
                            p[8].split('|')
                                .map(|s| {
                                    let (id, table) =
                                        s.split_once('=').ok_or(AbilityError::Catalog)?;
                                    Ok((hex(id)?, Table::parse(table)?))
                                })
                                .collect::<Result<Vec<_>, AbilityError>>()?
                        };
                        definition.applied.push(Applied {
                            side,
                            did: hex(p[1])?,
                            duration: duration.clone(),
                            pulsed: kind == HEALTH_OVER_TIME || kind == VIGOR_OVER_TIME,
                            label: effect_label(kind, &stats, &Table::parse(p[7])?),
                        });
                    }
                    let values = Table::parse(p[7])?;
                    // Do not spend vigor for a skill while silently dropping its
                    // essential status/summon component. Recognized stat effects
                    // occur under several derived native type codes.
                    let has_stats = p[8] != "-";
                    let stat_ids: Vec<u32> = if has_stats {
                        p[8].split('|')
                            .map(|s| {
                                s.split_once('=')
                                    .ok_or(AbilityError::Catalog)
                                    .and_then(|(id, _)| hex(id))
                            })
                            .collect::<Result<_, _>>()?
                    } else {
                        Vec::new()
                    };
                    let known_stats = has_stats
                        && stat_ids.iter().all(|id| {
                            [HEALTH_REGEN, VIGOR_REGEN, NATURAL_ARMOR, ARMOR_LEVEL].contains(id)
                                || ATTACK_RATING.contains(id)
                                || DEFENSE_RATING.contains(id)
                        });
                    let known_vital = match kind {
                        HEALTH | VIGOR => {
                            side == Side::Target && !values.is_empty()
                        }
                        HEALTH_OVER_TIME | VIGOR_OVER_TIME => {
                            !values.is_empty()
                                && !duration.is_empty()
                                && period.is_finite()
                                && period > 0.0
                                && (side == Side::Target
                                    || values.0.iter().all(|(_, v)| *v >= 0.0))
                        }
                        _ => false,
                    };
                    let container = kind == 0
                        && side == Side::Own
                        && duration.is_empty()
                        && !has_stats
                        && values.is_empty();
                    let weapon = kind == WEAPON_ATTACK
                        && !has_stats
                        && (definition.kind == BASIC_ATTACK_KIND || !definition.damage.is_empty());
                    if !(known_vital
                        || known_stats
                        || container
                        || weapon
                        || definition.travel.is_some())
                        || has_stats && !known_stats
                    {
                        definition.unresolved = true;
                    }
                    match kind {
                        HEALTH => {
                            definition.unresolved |= !definition.health.is_empty();
                            definition.health = values;
                        }
                        VIGOR => {
                            definition.unresolved |= !definition.vigor_effect.is_empty();
                            definition.vigor_effect = values
                        }
                        HEALTH_OVER_TIME | VIGOR_OVER_TIME
                            if !values.is_empty() && !duration.is_empty() && period > 0.0 =>
                        {
                            definition.timed.push(Timed {
                                side,
                                vigor: kind == VIGOR_OVER_TIME,
                                values,
                                period_ms: (period * 1000.0) as u64,
                                duration: duration.clone(),
                            });
                        }
                        WEAPON_ATTACK if definition.kind == BASIC_ATTACK_KIND => {
                            definition.weapon = true
                        }
                        _ => {}
                    }
                    if p[8] != "-" && !duration.is_empty()
                        || p[8].contains("40125")
                        || p[8].contains("40262")
                        || p[8].contains("80000")
                    {
                        if definition.buff.is_some()
                            && stat_ids
                                .iter()
                                .any(|id| [HEALTH_REGEN, VIGOR_REGEN].contains(id))
                        {
                            definition.unresolved = true;
                        }
                        let mut health = Table::default();
                        let mut vigor = Table::default();
                        let mut rating = RatingSpec {
                            side,
                            duration: duration.clone(),
                            armor: Vec::new(),
                            attack: [Vec::new(), Vec::new(), Vec::new()],
                            defense: [Vec::new(), Vec::new(), Vec::new()],
                        };
                        if p[8] != "-" {
                            for stat in p[8].split('|') {
                                let (id, table) =
                                    stat.split_once('=').ok_or(AbilityError::Catalog)?;
                                let id = hex(id)?;
                                let parsed = Table::parse(table)?;
                                match id {
                                    HEALTH_REGEN if definition.buff.is_none() => health = parsed,
                                    VIGOR_REGEN if definition.buff.is_none() => vigor = parsed,
                                    NATURAL_ARMOR | ARMOR_LEVEL => rating.armor.push(parsed),
                                    _ => {
                                        if let Some(index) =
                                            ATTACK_RATING.iter().position(|stat| *stat == id)
                                        {
                                            rating.attack[index].push(parsed);
                                        } else if let Some(index) =
                                            DEFENSE_RATING.iter().position(|stat| *stat == id)
                                        {
                                            rating.defense[index].push(parsed);
                                        }
                                    }
                                }
                            }
                        }
                        if definition.buff.is_none()
                            && (!health.is_empty() || !vigor.is_empty())
                            && !duration.is_empty()
                        {
                            definition.buff = Some(Buff {
                                side,
                                health,
                                vigor,
                                duration,
                            });
                        }
                        if rating.useful() {
                            definition.ratings.push(rating);
                        }
                    }
                }
            }
            // The executor currently carries one effect per vital, a shared
            // healing period, and one rating expiry per side. Withhold compound
            // records that would otherwise lose an effect or lengthen its life.
            for (index, a) in definition.timed.iter().enumerate() {
                for b in &definition.timed[index + 1..] {
                    if a.side != b.side
                        || a.vigor == b.vigor
                        || a.period_ms != b.period_ms
                        || a.duration != b.duration
                    {
                        definition.unresolved = true;
                    }
                }
            }
            for (index, a) in definition.ratings.iter().enumerate() {
                if definition.ratings[index + 1..]
                    .iter()
                    .any(|b| a.side == b.side && a.duration != b.duration)
                {
                    definition.unresolved = true;
                }
            }
            if definition.harmful()
                && (definition.buff.is_some()
                    || definition
                        .health
                        .0
                        .iter()
                        .chain(&definition.vigor_effect.0)
                        .any(|(_, v)| *v > 0.0)
                    || definition
                        .timed
                        .iter()
                        .any(|t| t.values.0.iter().any(|(_, v)| *v > 0.0)))
            {
                definition.unresolved = true;
            }
            if definition.usable() && definitions.insert(skill, definition).is_some() {
                return Err(AbilityError::Catalog);
            }
        }
        if definitions.is_empty() {
            return Err(AbilityError::Catalog);
        }
        Ok(Self {
            definitions,
            skills: BTreeMap::new(),
            players: BTreeMap::new(),
            last_now: 0,
        })
    }
}
