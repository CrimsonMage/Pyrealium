//! Examination profiles (`examine_target`).
use super::*;

pub(super) fn examine_target(
    character: u64,
    target: u64,
    store: &Store,
    scene: &crate::scene::Scene,
    names: &crate::examination::Names,
    loot: &crate::loot::Loot,
    now: u64,
    effect_lines: &[String],
    hit_chance: Option<u32>,
) -> io::Result<Vec<u8>> {
    use crate::{
        examination::{self, Name},
        inventory::Location,
    };
    if let Some(player) = store
        .characters
        .get(&target)
        .filter(|player| !player.deleted && scene.can_observe(character, player.id))
    {
        let state = &player.progression;
        let mut lines = vec![
            format!("Level {}", state.level().number),
            format!("Health: {} / {}", state.health, state.level().health),
        ];
        if let Some(creation)=&player.creation {
            lines.insert(0,match creation.race {1=>"Human",2=>"Lugian",4=>"Tumerok",8=>"Drudge",16=>"Empyrean",_=>"Player"}.into());
        }
        if !state.alive() {
            lines.push("Dead".to_owned());
        }
        if character == target {
            lines.extend([
                format!("Vigor: {} / {}", state.vigor, state.level().vigor),
                format!("Experience: {}", state.xp),
                format!("Available experience: {}", state.available_xp),
                format!("Gold: {}", state.gold),
            ]);
            if player.binding.is_some() {
                lines.push("Bound to a lifestone.".to_owned());
            }
            lines.push(format!("Available skill credits: {}",player.skills.available_credits(state)));
        }
        return examination::response(target, Name::Literal(player.name.clone()), &lines)
            .map_err(other);
    }
    if let Some(item) = store.characters[&character]
        .inventory
        .items()
        .find(|item| item.id == target)
    {
        let location = match item.location {
            Location::Equipped(0x40) => "Equipped in main hand.",
            Location::Equipped(0x80) => "Equipped in off hand.",
            Location::Equipped(_) => "Equipped.",
            Location::Bag(_) => "In your inventory.",
        };
        let mut lines=names.item_lines(item,location);
        let profession=&store.characters[&character].professions;
        if let Some(job)=profession.1.jobs.get(&item.id){lines.push(job.guidance());}
        if let Some(tool)=crate::harvesting::tool(item.definition){lines.push(format!("Tool durability: {} / {}. Select a resource or corpse and double-click the tool.",tool.durability.saturating_sub(crate::harvesting::wear(profession,item)),tool.durability));}
        if let Some(skills)=crate::professions::creation_tool(item.definition){
            let level=item.roll.map_or(1,|r|r.level);let used=item.roll.map_or(0,|r|r.wear);
            lines.push(format!("Creation tool: +{level} effective rank while equipped. Requires matching crafting rank {}. Durability: {} / 20.",u32::from(level)*10,20u16.saturating_sub(used)));
            lines.push(skills.iter().filter_map(|s|crate::professions::skill(*s).map(|s|s.name.as_str())).collect::<Vec<_>>().join(", "));
        }
        return examination::response(
            target,
            names.get(item.definition, "Item"),
            &lines,
        )
        .map_err(other);
    }
    if let Some(item)=loot.inspect_item(target,character,now,|container|scene.can_observe(character,container)&&scene.combat_distance(character,container).is_some_and(|d|d<=4.)) {
        return examination::response(target,names.get(item.definition,"Item"),&names.item_lines(item,"In the loot container.")).map_err(other);
    }
    if let Some(drop)=store.drops.get(&target).filter(|_|scene.can_observe(character,target)) {
        return examination::response(target,names.get(drop.item.definition,"Item"),&names.item_lines(&drop.item,"On the ground. Double-click nearby to pick up.")).map_err(other);
    }
    if let Some(definition) = scene.visible_hand_definition(character, target) {
        return examination::response(target, names.get(definition, "Item"), &[]).map_err(other);
    }
    if let Some(object) = scene.examination_content(character, target) {
        let mut lines = Vec::new();
        let dead=object.stat(0x102)==Some(0) && object.stat(0x107).is_some();
        // A creature's corpse: the retail profile rows (`examination::corpse_nodes`) once the server
        // knows its killer; a body killed before this server started, or whose killer is gone, keeps
        // the plain lines.
        let person=|id:u64|store.characters.get(&id).filter(|c|!c.deleted).map(|c|{
            let female=c.creation.as_ref().is_some_and(|r|r.sex==crate::appearance::FEMALE);
            Name::Literal(examination::tagged_name(&c.name,female))
        });
        if let Some((body,killer))=loot.body(target).filter(|_|dead && object.service.is_none()).and_then(|b|person(b.killer).map(|k|(b,k))) {
            let corpse=examination::Corpse {
                killer,
                butcher:body.butcher.and_then(person),
                first_looter:body.first_looter.and_then(person),
                // Pyrealium policy: a body's loot is its killer's (`loot::Loot::get`).
                can_loot:body.killer==character,
                contents:body.opened.then(||loot.remaining(target,body.killer,now)),
            };
            let victim=object.name.clone().map(Name::Literal).unwrap_or_else(||names.get(object.entity,"Object"));
            return examination::corpse_response(target,victim,&corpse).map_err(other);
        }
        if dead {lines.push("Defeated creature. Double-click to examine its remaining loot.".into());}
        else if let Some(level)=object.stat(0x12e) {lines.push(format!("Level {level}"));}
        if let Some(service)=object.service {
            use crate::services::Service;
            lines.push(match service {
                Service::Vendor=>"Vendor. Double-click nearby to trade.",
                Service::Door{locked:true}=>"Locked door.",Service::Door{locked:false}=>"Door. Double-click nearby to open or close.",
                Service::Chest{locked:true}=>"Locked chest.",Service::Chest{locked:false}=>"Chest. Double-click nearby to open.",
                Service::Sign=>"Sign. Double-click to read.",
                Service::Deposit=>"Resource deposit. Wield the matching tool and double-click it.",
            }.into());
        }
        if let Some((health, maximum)) = object.stat(0x102).zip(object.stat(0x107)) {
            lines.push(format!("Health: {health} / {maximum}"));
        }
        if let Some((vigor, maximum)) = object.stat(0x103).zip(object.stat(0x108)) {
            lines.push(format!("Vigor: {vigor} / {maximum}"));
        }
        // Timed effects on the creature (a burn, a lowered armor), with the time left. Pyrealium's own
        // text: the retail server's profile for another creature's effects is not known.
        lines.extend(effect_lines.iter().take(3).cloned());
        lines.extend(names.description_lines(object.entity));
        if object.entity == TRAINING_MENTOR_ENTITY {
            lines.push("Use to accept training lessons, check progress or deliver supplies.".into());
            lines.push("Nearby conversation topics: @mentor help, gear, combat, healing, supplies, crafting, travel.".into());
        } else if object.entity == crate::merchants::MERCHANT_ENTITY {
            lines.push("Use to trade. Mucor Mushrooms and Tallow bundles each cost 2 gold.".into());
        }
        if let Some(name) = crate::training_dialogue::portal_name(target) {
            lines.push("Use while alive and out of combat, within four metres.".into());
            return examination::response(target, Name::Literal(name.into()), &lines).map_err(other);
        }
        let mut name=object.name.clone().map(Name::Literal).unwrap_or_else(||names.get(object.entity, "Object"));
        if dead {name=Name::Corpse(Box::new(name));}
        // A living creature's profile ends with the examiner's chance to hit it, as retail's did.
        return examination::response_with_hit_chance(target, name, &lines, hit_chance.filter(|_| !dead))
            .map_err(other);
    }
    Err(invalid("That object is not available to examine."))
}
