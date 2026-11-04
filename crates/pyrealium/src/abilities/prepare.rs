//! A cast prepared: its checks, costs and effects before it is carried out (`Runtime::prepare`).
use super::*;

impl Runtime {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare<'a>(
        &'a mut self,
        player: u64,
        skill: u32,
        aim: Aim,
        hands: (Hand, Hand),
        book: &Book,
        state: &Progression,
        now: u64,
    ) -> Result<PreparedCast<'a>, AbilityError> {
        let definition = self
            .definitions
            .get(&skill)
            .ok_or(AbilityError::Unsupported)?
            .clone();
        if definition.unresolved {
            return Err(AbilityError::Unsupported);
        }
        let aim = self.normalize_aim(skill, aim);
        if player == 0 || player == u64::MAX {
            return Err(AbilityError::InvalidCharacter);
        }
        if !book.usable(skill) {
            return Err(AbilityError::NotLearned);
        }
        let rank = book.get(skill).ok_or(AbilityError::NotLearned)?.rank;
        if !state.valid() {
            return Err(AbilityError::InvalidProgression);
        }
        if !state.alive() {
            return Err(AbilityError::Dead);
        }
        if state.vigor < MIN_VIGOR && definition.vigor > 0 {
            return Err(AbilityError::InsufficientVigor);
        }
        if !holds(definition.right, hands.0) {
            return Err(AbilityError::Hand {
                right: true,
                required: definition.right,
            });
        }
        if !holds(definition.left, hands.1) {
            return Err(AbilityError::Hand {
                right: false,
                required: definition.left,
            });
        }
        let cost = definition.vigor.min(state.vigor);
        let over = |side: Side, vigor: bool, sign: f32| -> Option<Over> {
            definition
                .timed
                .iter()
                .find(|t| t.side == side && t.vigor == vigor && t.values.at(rank) * sign > 0.0)
                .map(|t| {
                    let amount = (t.values.at(rank) * sign).round() as u32;
                    Over {
                        health: if vigor { 0 } else { amount },
                        vigor: if vigor { amount } else { 0 },
                        period_ms: t.period_ms,
                        duration_ms: (t.duration.at(rank).max(0.0) * 1000.0) as u64,
                    }
                })
        };
        let combined_over = |side: Side, sign: f32| -> Option<Over> {
            let health = over(side, false, sign);
            let vigor = over(side, true, sign);
            match (health, vigor) {
                (Some(h), Some(v)) => Some(Over {
                    vigor: v.vigor,
                    ..h
                }),
                (h, v) => h.or(v),
            }
        };
        let regen_for = |side: Side| -> Option<Regen> {
            definition
                .buff
                .as_ref()
                .filter(|b| b.side == side)
                .map(|b| Regen {
                    health: if b.health.is_empty() {
                        1.0
                    } else {
                        b.health.at(rank)
                    },
                    vigor: if b.vigor.is_empty() {
                        1.0
                    } else {
                        b.vigor.at(rank)
                    },
                    duration_ms: (b.duration.at(rank).max(0.0) * 1000.0) as u64,
                })
        };
        let restore = || {
            (
                definition.health.at(rank).max(0.0).round() as u32,
                definition.vigor_effect.at(rank).max(0.0).round() as u32,
            )
        };
        for side in [Side::Own, Side::Target] {
            if regen_for(side).is_some_and(|r| {
                !(0.0..=100.0).contains(&r.health) || !(0.0..=100.0).contains(&r.vigor)
            }) {
                return Err(AbilityError::Unsupported);
            }
        }
        let caster_ratings = {
            let mut total = definition.ratings_for(Side::Own, rank);
            if matches!(aim, Aim::Own) {
                if let Some(targeted) = definition.ratings_for(Side::Target, rank) {
                    total = Some(match total {
                        Some((mut ratings, duration)) => {
                            ratings.add(targeted.0);
                            (ratings, duration.max(targeted.1))
                        }
                        None => targeted,
                    });
                }
            }
            total.filter(|(ratings, _)| !ratings.is_empty())
        };
        let other_ratings = match aim {
            Aim::Player(target, _) | Aim::Creature(target) => definition
                .ratings_for(Side::Target, rank)
                .filter(|(ratings, _)| !ratings.is_empty())
                .map(|(ratings, duration)| (target, ratings, duration)),
            _ => None,
        };
        let applied_for = |side_target: Option<u64>| -> Vec<AppliedEffect> {
            definition
                .applied
                .iter()
                .filter_map(|a| {
                    let entity = match (a.side, side_target) {
                        (Side::Own, _) => player,
                        (Side::Target, Some(target)) => target,
                        (Side::Target, None) => return None,
                    };
                    let seconds = a.duration.at(rank).max(1.0);
                    Some(AppliedEffect {
                        entity,
                        did: a.did,
                        duration_ms: (seconds * 1000.0) as u64,
                        power: f32::from(rank),
                        pulsed: a.pulsed,
                    })
                })
                .collect()
        };
        let applied = match aim {
            // A skill used on oneself puts its target-side effects on the caster too.
            Aim::Own => applied_for(Some(player)),
            Aim::Player(target, _) => applied_for(Some(target)),
            // A creature's effects land when the strike does (`Strike::effects`).
            Aim::Creature(_) => applied_for(None),
            Aim::Other => Vec::new(),
        };
        let outcome = if let Some(travel) = definition.travel.clone() {
            Outcome::Travel(travel)
        } else {
            match aim {
                Aim::Creature(target) if definition.harmful() => {
                    if definition.targets & MONSTER == 0 {
                        return Err(AbilityError::InvalidTarget);
                    }
                    let instant = definition.health.at(rank).min(0.0).abs();
                    let bonus = (definition.damage.at(rank).abs() + instant).round() as u32;
                    Outcome::Strike {
                        target,
                        bonus,
                        vigor_damage: definition.vigor_effect.at(rank).min(0.0).abs().round() as u32,
                        weapon_damage: definition.weapon || !definition.damage.is_empty(),
                        cost,
                        range: definition.range,
                        burn: combined_over(Side::Target, -1.0),
                    }
                }
                _ if definition.harmful() => return Err(AbilityError::TargetCreature),
                Aim::Own => {
                    if definition.targets & SELF == 0 {
                        return Err(AbilityError::InvalidTarget);
                    }
                    let (health, vigor) = restore();
                    Outcome::Restore {
                        health,
                        vigor,
                        mend: combined_over(Side::Target, 1.0).or_else(|| combined_over(Side::Own, 1.0)),
                        regen: regen_for(Side::Target).or_else(|| regen_for(Side::Own)),
                    }
                }
                Aim::Player(target, distance) => {
                    if definition.targets & PLAYER == 0 {
                        return Err(AbilityError::InvalidTarget);
                    }
                    if distance > definition.range.max(3.0) {
                        return Err(AbilityError::OutOfRange);
                    }
                    let (health, vigor) = restore();
                    Outcome::Support {
                        target,
                        health,
                        vigor,
                        mend: combined_over(Side::Target, 1.0),
                        regen: regen_for(Side::Target),
                    }
                }
                Aim::Creature(_) | Aim::Other => return Err(AbilityError::InvalidTarget),
            }
        };
        if let Outcome::Support {
            health: 0,
            vigor: 0,
            mend: None,
            regen: None,
            ..
        } = outcome
        {
            if caster_ratings.is_none() && other_ratings.is_none() {
                return Err(AbilityError::Unsupported);
            }
        }
        self.expire(now)?;
        if let Some(&until) = self.skills.get(&(player, skill)) {
            return Err(AbilityError::Cooldown {
                remaining_ms: until - now,
            });
        }
        if let Some(&until) = self.players.get(&player) {
            return Err(AbilityError::GlobalCooldown {
                remaining_ms: until - now,
            });
        }
        if self.skills.len() >= MAX_SKILL_COOLDOWNS || self.players.len() >= MAX_PLAYER_COOLDOWNS {
            return Err(AbilityError::Capacity);
        }
        let until = now
            .checked_add(definition.reset_ms.max(GLOBAL_COOLDOWN_MS))
            .ok_or(AbilityError::Clock)?;
        let global_until = now
            .checked_add(GLOBAL_COOLDOWN_MS)
            .ok_or(AbilityError::Clock)?;
        let mut commands = Vec::new();
        if !matches!(outcome, Outcome::Strike { .. }) && cost > 0 {
            commands.push(Command::SpendVigor(cost));
        }
        if let Outcome::Restore { health, vigor, .. } = outcome {
            if health > 0 {
                commands.push(Command::Heal(health));
            }
            if vigor > 0 {
                commands.push(Command::RecoverVigor(vigor));
            }
        }
        let next = commands.iter().try_fold(state.clone(), |value, command| {
            value
                .apply(command)
                .map_err(|_| AbilityError::InvalidProgression)
        })?;
        if let Outcome::Restore {
            mend: None,
            regen: None,
            ..
        } = outcome
        {
            if next.health == state.health
                && next.vigor + cost <= state.vigor
                && caster_ratings.is_none()
            {
                return Err(AbilityError::NothingToRestore);
            }
        }
        Ok(PreparedCast {
            runtime: self,
            player,
            skill,
            // Basic weapon attacks keep the validated training swing presentation.
            story: if definition.weapon {
                0
            } else {
                definition.story
            },
            fx: if definition.weapon { 0 } else { definition.fx },
            until,
            global_until,
            outcome,
            commands,
            previous: state.clone(),
            next,
            caster_ratings,
            other_ratings,
            applied,
            creature_effects: match aim {
                Aim::Creature(_) => definition
                    .applied
                    .iter()
                    .filter(|a| a.side == Side::Target)
                    .take(2)
                    .map(|a| (a.did, (a.duration.at(rank).max(1.0) * 1000.0) as u64))
                    .collect(),
                _ => Vec::new(),
            },
        })
    }
}
