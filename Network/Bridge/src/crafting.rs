// Native WM_Craft registry, ingredient checks, and execution. No chat-command shim.
use ac2_net::protocol::*;
use serde_json::{json, Value};

pub fn decode(function: u32, arguments: &[u8]) -> Option<Value> {
    let mut result=match CraftMessage::decode(function, arguments)? {
        Ok(CraftMessage::SetRegistry { skills, recipes }) =>
            json!({"event":"craft_registry","replace":true,"skills":skills,"recipes":recipes}),
        Ok(CraftMessage::LearnRecipe { recipe }) =>
            json!({"event":"craft_registry","replace":false,"recipes":[recipe]}),
        Ok(CraftMessage::SetCraftSkills { skills }) =>
            json!({"event":"craft_skills","skills":skills}),
        Ok(CraftMessage::SetCraftSkill { skill }) =>
            json!({"event":"craft_skills","skills":[skill]}),
        Ok(CraftMessage::UpdateRecipe { recipe }) =>
            json!({"event":"craft_recipe_update","recipes":[recipe]}),
        Ok(CraftMessage::UpdateRecipes { recipes }) =>
            json!({"event":"craft_recipe_update","recipes":recipes}),
        Ok(CraftMessage::AddIngredientDone { recipe, ordinal, ingredient, spinner, error }) =>
            json!({"event":"craft_ingredient","recipe":recipe,"ordinal":ordinal,"item":ingredient.to_string(),"spinner":spinner,"error":error}),
        Ok(CraftMessage::ExecuteCraftDone { notify, recipe, error }) =>
            json!({"event":"craft_complete","notify":notify,"recipe":recipe,"error":error}),
        Err(error) => json!({"event":"error","message":format!("Invalid native crafting message: {error}")}),
    };
    enrich(&mut result, arguments);Some(result)
}

pub fn command(v: &Value) -> Result<ClientEvent, String> {
    if super::text(v,"command")?=="craft_raise" {let skill=super::word(v,"skill")?;if skill>>24!=0x6f{return Err("Invalid profession".into())}let request=ScriptRequest::CraftRaiseSkill(skill);return Ok(ClientEvent::Interp{function:request.function(),arguments:pack_values(&request.arguments()),encrypted:false});}
    let spinner=v["spinner"].as_u64().unwrap_or(0);if spinner>1000{return Err("Invalid crafting level".into())}
    let recipe=super::word(v,"recipe")?;
    if recipe>>24!=0x6f {return Err("Invalid recipe identity".into());}
    let request=match super::text(v,"command")? {
        "craft_validate" => {
            let ordinal=super::word(v,"ordinal")?;let object=super::id(v,"item")?;
            if !(1..=16).contains(&ordinal) || [0,u64::MAX].contains(&object) {return Err("Invalid ingredient".into());}
            ScriptRequest::CraftAddIngredient{recipe,ordinal,object,spinner:spinner as u32}
        }
        "craft_execute" => {
            let entries=v["ingredients"].as_array().filter(|e|e.len()<=16).ok_or("Invalid ingredients")?;
            let mut ingredients=Vec::new();let mut ids=std::collections::BTreeSet::new();
            for (i,value) in entries.iter().enumerate() {
                let ordinal=value.get("ordinal").and_then(Value::as_u64).unwrap_or(i as u64+1);if !(1..=16).contains(&ordinal){return Err("Invalid ingredient ordinal".into())}
                let item=value.as_str().or_else(||value["item"].as_str()).and_then(|s|s.parse::<u64>().ok()).filter(|n|![0,u64::MAX].contains(n)).ok_or("Invalid ingredient identity")?;
                if !ids.insert(item) {return Err("Duplicate ingredient identity".into());}
                if ingredients.iter().any(|(o,_)|*o==ordinal as u32){return Err("Duplicate ingredient ordinal".into())}
                ingredients.push((ordinal as u32,item));
            }
            let helper=if ingredients.is_empty(){super::id(v,"helper")?}else{ingredients[0].1};
            if [0,u64::MAX].contains(&helper){return Err("Invalid crafting helper".into())}
            ScriptRequest::CraftExecute{helper,recipe,spinner:spinner as u32,ingredients}
        }
        _=>return Err("Unknown crafting command".into()),
    };
    Ok(ClientEvent::Interp{function:request.function(),arguments:pack_values(&request.arguments()),encrypted:false})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_registry_replaces_entitlements_and_empty_registry_clears_them() {
        for recipes in [vec![0x6f001f17,0x6f001f18],vec![]] {
            let m=CraftMessage::SetRegistry{skills:vec![0x6f0028d7],recipes:recipes.clone()};
            let j=decode(m.function(),&pack_values(&m.arguments())).unwrap();
            assert_eq!(j["recipes"],json!(recipes));assert_eq!(j["replace"],true);
        }
        assert!(decode(0x82060019,&[0]).unwrap()["message"].as_str().unwrap().contains("Invalid"));
    }
    #[test]
    fn execution_preserves_unsigned_iids_and_rejects_ambiguous_ingredients() {
        let mut v=json!({"command":"craft_execute","recipe":0x6f001f17u32,"ingredients":["9308940437337407489","9308940437337407490"]});
        let ClientEvent::Interp{function,arguments,..}=command(&v).unwrap() else {panic!()};
        let ScriptRequest::CraftExecute{helper,ingredients,..}=ScriptRequest::decode(function,&arguments).unwrap().unwrap() else {panic!()};
        assert_eq!(helper,9308940437337407489);assert_eq!(ingredients,vec![(1,helper),(2,helper+1)]);
        v["ingredients"]=json!([helper.to_string(),helper.to_string()]);assert!(command(&v).is_err());
        v["ingredients"]=json!([helper]);assert!(command(&v).is_err());
    }
}

fn enrich(event:&mut Value,arguments:&[u8]){
    use ac2_net::protocol::{unpack_values,PackValue,PackedEntry};
    static DATA:std::sync::OnceLock<Value>=std::sync::OnceLock::new();
    let data=DATA.get_or_init(||serde_json::from_str(include_str!("../../../emulator/content/professions-v1.json")).expect("profession catalog"));
    if !matches!(event["event"].as_str(),Some("craft_registry"|"craft_skills")){return}
    if let Ok(values)=unpack_values(arguments){
        let mut progress=Vec::new();
        for v in values {if let PackValue::Reference(r)=v{for entry in r.entries {if let PackedEntry::Heap{object,..}=entry{if object.pkg_id==0x345f&&object.slots.len()==6{
            let s=&object.slots;progress.push(json!({"id":s[2],"level":s[5],"earned":u64::from(s[0])|(u64::from(s[1])<<32),"available":u64::from(s[3])|(u64::from(s[4])<<32)}));
        }}}}}
        event["progress"]=json!(progress);
    }
    if event["event"]=="craft_registry" {
        let ids:std::collections::BTreeSet<u64>=event["recipes"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
        event["catalog"]=json!(data["recipes"].as_array().unwrap().iter().filter(|r|ids.contains(&r["id"].as_u64().unwrap())).collect::<Vec<_>>());
        event["professions"]=data["skills"].clone();
        event["success"]=data["success"].clone();
    }
}
