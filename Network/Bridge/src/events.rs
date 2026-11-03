// SPDX-License-Identifier: AGPL-3.0-only
use ac2_net::protocol::*;
use serde_json::{json, Value};

pub(crate) fn string(s: &StringInfo) -> Value {
    let variables:Vec<_>=s.variables.iter().filter_map(|(key,value)|match value {
        StringVariable::Nested(n)=>Some(json!({"key":key,"text":string(n)})),
        _=>None,
    }).collect();
    json!({"literal":s.literal,"table":s.table_id,"key":s.string_id,"variables":variables})
}
fn visual(v: &VisualDesc) -> Value {
    let aspects: Vec<Value> = v.global_mods.iter()
        .chain(v.part_groups.iter().map(|(_, g)|g))
        .flat_map(|g| {
            g.appearance.iter().flat_map(|(table, values)| {
                values
                    .iter()
                    .map(move |(key, value)| json!({"table":table,"key":key,"value":value}))
            })
        })
        .collect();
    json!({"did":v.parent.or(v.did).unwrap_or(0),"scale":v.scale.unwrap_or([1.;3]),"aspects":aspects})
}
fn character(c: &CharacterIdentity) -> Value {
    json!({"id":c.id.to_string(),"name":c.name,"disabled":c.seconds_greyed_out!=0,"visual":visual(&c.visual)})
}
fn behavior(id: u64, instance: u16, p: &BehaviorParams) -> Value {
    json!({"event":"behavior","id":id.to_string(),"instance":instance,"behavior":p.behavior,
        // hold_cycles controls completion callbacks on a native hold track; zero
        // does not turn an arbitrary attack, jump or cast into a held final pose.
        "mode":p.mode,"rate":p.time_scale,"hold":false,"lock":p.lock_actions!=0,
        "impulse":p.impulse,"propagate":p.propagate!=0})
}
fn position(p: &Position) -> Value {
    let [w, x, y, z] = p.rotation;
    // Native Heading is clockwise from +Y; native quaternion is a standard AC z rotation.
    let yaw = (2. * (w * z + x * y))
        .atan2(1. - 2. * (y * y + z * z))
        .to_degrees();
    json!({"cell":p.cell,"position":p.origin,"heading":(-yaw).rem_euclid(360.),"rotation":p.rotation})
}
fn quality(q: QualityUpdate) -> Option<Value> {
    let table = q.value.table();
    let value = match q.value {
        QualityValue::Int(v) | QualityValue::Bool(v) => json!(v),
        QualityValue::Float(v) => json!(v),
        QualityValue::LongInt(v) => json!(v.to_string()),
        QualityValue::DataId(v) => json!(v),
        QualityValue::InstanceId(v) => json!(v.to_string()),
        QualityValue::StringInfo(v) => string(&v),
        _ => return None,
    };
    Some(
        json!({"event":"quality","id":q.object.map(|o|o.id.to_string()),"instance":q.object.map(|o|o.instance),"table":table,"key":q.quality,"value":value}),
    )
}
pub fn convert(event: ServerEvent) -> Vec<Value> {
    let one = match event {
        ServerEvent::CharacterSet(s) => {
            json!({"event":"characters","characters":s.characters.iter().map(character).collect::<Vec<_>>(),"allowed":s.allowed})
        }
        ServerEvent::CharGenVerification {
            response,
            identity,
            weenie_result,
        } => {
            json!({"event":"created","response":response,"result":weenie_result,"character":character(&identity)})
        }
        ServerEvent::CharacterError(code) => json!({"event":"character_error","code":code}),
        ServerEvent::CharacterDeleted(id) => json!({"event":"deleted","id":id.to_string()}),
        ServerEvent::CreatePlayer { id, region } => {
            json!({"event":"player","id":id.to_string(),"region":region})
        }
        ServerEvent::CreateObject(o) => {
            let object=json!({"event":"object","id":o.id.to_string(),"visual":visual(&o.visual),
            "placement":o.physics.position.as_ref().map(position),"instance":o.physics.instance_stamp,"scale":o.weenie.scale.unwrap_or(1.0),
            "entity":o.weenie.entity_did,"icon":o.weenie.icon,"quantity":o.weenie.quantity,
            "name":o.weenie.name.as_ref().map(string),"parent":o.physics.parent.map(|p|p.0.to_string()),"holding":o.physics.parent.map(|p|p.1),
            "container":o.weenie.container.map(|p|p.to_string()),"wielder":o.weenie.wielder.map(|p|p.to_string())});
            let mut out=vec![object];
            // Late observers receive an already moving missile in PhysicsDesc,
            // without a second InitializeMissile event.
            if o.physics.bitfield & 0x80000 != 0 {
                if let (Some(p),Some(velocity))=(o.physics.position,o.physics.velocity) {
                    let mut offset=o.physics.target_position.unwrap_or(Position{cell:0,origin:[0.;3],rotation:[1.,0.,0.,0.]});
                    if o.physics.target.unwrap_or(0)!=0 {offset.origin=o.physics.target_offset.unwrap_or([0.;3]);}
                    out.push(json!({"event":"missile","id":o.id.to_string(),"instance":o.physics.instance_stamp,
                        "placement":position(&p),"velocity":velocity,"target":o.physics.target.unwrap_or(0).to_string(),
                        "offset":position(&offset),"initialized":o.physics.target.is_some() || o.physics.target_position.is_some()}));
                }
            }
            if let Some(mode)=o.physics.mode {out.push(json!({"event":"mode","id":o.id.to_string(),"instance":o.physics.instance_stamp,"mode":mode}));}
            for p in &o.physics.behaviors {out.push(behavior(o.id,o.physics.instance_stamp,p));}
            for (fx,scalar,target) in &o.physics.fx {out.push(json!({"event":"fx","id":o.id.to_string(),"instance":o.physics.instance_stamp,"fx":fx,"strength":scalar,"target":target.to_string()}));}
            for (name,value) in [("velocity",o.physics.velocity_scale),("jump",o.physics.jump_scale)] {
                if let Some(value)=value {out.push(json!({"event":"motion","id":o.id.to_string(),"instance":o.physics.instance_stamp,"parameter":name,"value":value}));}
            }
            return out;
        }
        ServerEvent::UpdatePosition(p) | ServerEvent::PositionCell(p) => {
            json!({"event":"position","id":p.sender.id.to_string(),"instance":p.sender.instance,
            "cell":p.position.cell,"position":p.position.origin,"heading":p.heading_degrees,"motion":p.motion,"time":p.time,"teleport":p.teleport_stamp,"force":p.force_position_stamp})
        }
        ServerEvent::DestroyObject(o) | ServerEvent::Contain(o) => {
            json!({"event":"remove","id":o.id.to_string(),"instance":o.instance})
        }
        ServerEvent::Object(o) => match o.action {
            ObjectAction::InitializeMissile {position:p,velocity,target} => json!({"event":"missile","id":o.sender.id.to_string(),"instance":o.sender.instance,
                "placement":position(&p),"velocity":velocity,"target":target.target.to_string(),"offset":position(&target.offset),"target_velocity":target.velocity,"initialized":target.initialized!=0}),
            ObjectAction::DoFx {fx,scalar} => json!({"event":"fx","id":o.sender.id.to_string(),"instance":o.sender.instance,"fx":fx,"strength":scalar}),
            ObjectAction::DoFxScript(script) => json!({"event":"fx_script","id":o.sender.id.to_string(),"instance":o.sender.instance,"script":script}),
            ObjectAction::DoTargettedFx {fx,scalar,target} => json!({"event":"fx","id":o.sender.id.to_string(),"instance":o.sender.instance,"fx":fx,"strength":scalar,"target":target.to_string()}),
            ObjectAction::StopFx(fx) => json!({"event":"stop_fx","id":o.sender.id.to_string(),"instance":o.sender.instance,"fx":fx}),
            ObjectAction::DoMode(mode) | ObjectAction::SetMode(mode) => {
                json!({"event":"mode","id":o.sender.id.to_string(),"instance":o.sender.instance,"mode":mode})
            }
            ObjectAction::DoBehavior(p) | ObjectAction::ReleaseBehavior {params:p,..} => behavior(o.sender.id,o.sender.instance,&p),
            ObjectAction::StopBehavior(b) => json!({"event":"stop_behavior","id":o.sender.id.to_string(),"instance":o.sender.instance,"behavior":b}),
            ObjectAction::StopAllWeenieBehaviors => json!({"event":"stop_behavior","id":o.sender.id.to_string(),"instance":o.sender.instance,"behavior":0}),
            ObjectAction::DoStory(story) => {
                let mut out=Vec::new();
                if let Some(p)=story.attacker_behavior.as_ref() {
                    let attacker=story.attacker.unwrap_or(o.sender.id);
                    let mut v=behavior(attacker,o.sender.instance,p);
                    // The envelope stamp belongs to the story owner, not a different attacker.
                    if attacker!=o.sender.id {v.as_object_mut().unwrap().remove("instance");}
                    out.push(v);
                }
                for hook in story.hooks {
                    if hook.kind()!=1 {continue;}
                    if let Some(health)=hook.target_health_change {
                        if health<0 {
                            let target=hook.target.unwrap_or(o.sender.id);
                            let mut hit=json!({"event":"hit","id":target.to_string(),"attacker":story.attacker.unwrap_or(o.sender.id).to_string(),"damage":-i64::from(health)});
                            if target==o.sender.id {hit["instance"]=json!(o.sender.instance);}
                            out.push(hit);
                        }
                    }
                }
                return out;
            }
            ObjectAction::AddImpulse {direction,..} => json!({"event":"impulse","id":o.sender.id.to_string(),"instance":o.sender.instance,"direction":direction}),
            ObjectAction::SetVelocityScale(value) => json!({"event":"motion","id":o.sender.id.to_string(),"instance":o.sender.instance,"parameter":"velocity","value":value}),
            ObjectAction::SetAccelerationScale(value) => json!({"event":"motion","id":o.sender.id.to_string(),"instance":o.sender.instance,"parameter":"acceleration","value":value}),
            ObjectAction::SetJumpScale(value) => json!({"event":"motion","id":o.sender.id.to_string(),"instance":o.sender.instance,"parameter":"jump","value":value}),
            ObjectAction::SetGlobalModifier(value) => json!({"event":"motion","id":o.sender.id.to_string(),"instance":o.sender.instance,"parameter":"modifier","value":value}),
            ObjectAction::SetMotionInterp(value) => json!({"event":"motion","id":o.sender.id.to_string(),"instance":o.sender.instance,"parameter":"descriptor","value":value}),
            ObjectAction::UpdateVisualDesc(v) => {
                json!({"event":"visual","id":o.sender.id.to_string(),"instance":o.sender.instance,"visual":visual(&v)})
            }
            ObjectAction::LeaveWorld(_) => {
                json!({"event":"remove","id":o.sender.id.to_string(),"instance":o.sender.instance})
            }
            _ => return vec![],
        },
        ServerEvent::PlayerDescription(p) => {
            let mut out = Vec::new();
            for (key, value) in p.ints {
                out.push(json!({"event":"quality","id":null,"table":4,"key":key,"value":value}));
            }
            for (key, value) in p.floats {
                out.push(json!({"event":"quality","id":null,"table":3,"key":key,"value":value}));
            }
            for (key, value) in p.long_ints {
                out.push(json!({"event":"quality","id":null,"table":12,"key":key,"value":value.to_string()}));
            }
            return out;
        }
        ServerEvent::Quality(q) => return quality(q).into_iter().collect(),
        ServerEvent::DisplayStringInfo { kind, text } => {
            json!({"event":"chat","kind":kind,"text":string(&text)})
        }
        ServerEvent::WorldName(name) => json!({"event":"world_name","text":string(&name)}),
        ServerEvent::AccountBooted(reason) => {
            json!({"event":"disconnected","message":reason.literal.unwrap_or("Account disconnected by server".into())})
        }
        ServerEvent::Interp {
            function,
            arguments,
            object,
        } => {
            if let Some(event)=crate::shop::decode(function,&arguments) {return vec![event];}
            if let Some(event)=crate::journal::decode(function,&arguments) {return vec![event];}
            if let Some(event)=crate::crafting::decode(function,&arguments) {return vec![event];}
            // Native MessageDialog has a priority followed by StringInfo. The
            // bridge reports its readable text as chat instead of discarding
            // the notice as an unhandled script. Retain the diagnostic event.
            if function==0x82120024 {
                if let Ok(values)=unpack_values(&arguments) {
                    if let [PackValue::Int(_),PackValue::Reference(r)]=values.as_slice() {
                        if let [PackedEntry::Heap{object,..}]=r.entries.as_slice() {
                            if let Some(NativeObject::StringInfo(s))=&object.native {
                                return vec![json!({"event":"chat","kind":1,"text":string(s)}),
                                    json!({"event":"script","function":function,"object":null})];
                            }
                        }
                    }
                }
            }
            if function==0x820e0058 {
                if let Ok(values)=unpack_values(&arguments) {
                    if let [PackValue::LongInt(item)]=values.as_slice() {
                        return vec![json!({"event":"inventory_remove","id":item.to_string()})];
                    }
                }
                return vec![json!({"event":"error","message":"Invalid native inventory removal"})];
            }
            if function == 0x8204001b || function == 0x82040019 {
                if let Ok(values) = unpack_values(&arguments) {
                    if let [PackValue::Int(value)] = values.as_slice() {
                        return vec![if function == 0x8204001b {
                            json!({"event":"combat","enabled":*value != 0})
                        } else { json!({"event":"skill_finished","request":value}) }];
                    }
                }
            }
            if let Some(Ok(DeathMessage::PlayerDeath(dead))) =
                DeathMessage::decode(function, &arguments)
            {
                return vec![json!({"event":"death","dead":u32::from(dead)})];
            }
            if function == 0x82120027 {
                if let Err(error) = unpack_values(&arguments) {
                    return vec![json!({"event":"error","message":format!("Cannot decode player inventory: {error}")})];
                }
                if let Ok(values) = unpack_values(&arguments) {
                    if values.len() == 14 {
                        let bag = ContainerContents::from_values(&values[9..12]);
                        let equipment = Equipment::from_values(&values[6..9]);
                        if bag.is_err() || equipment.is_err() {
                            return vec![json!({"event":"error","message":"Cannot decode authoritative inventory slots"})];
                        }
                        if let (Ok(bag), Ok(equipment)) = (bag, equipment) {
                            let gold = match values[0] {
                                PackValue::Int(n) => n,
                                _ => 0,
                            };
                            let mut events=vec![
                                json!({"event":"inventory","items":bag.items.iter().map(u64::to_string).collect::<Vec<_>>(),"equipment":equipment.locations().unwrap_or_default().iter().map(|(slot,id)|json!({"slot":slot,"id":id.to_string()})).collect::<Vec<_>>(),"gold":gold}),
                            ];
                            if let Some(q)=crate::journal::quests(&values[2],true) {events.push(q);}
                            if let PackValue::Reference(r)=&values[4] {
                                if let Some(r)=SkillRepository::decode(r) {events.push(crate::journal::repository(&r));}
                            }
                            return events;
                        }
                    }
                }
            }
            if function==0x820a0009 {
                return vec![crate::examination::convert(&arguments).unwrap_or_else(||
                    json!({"event":"examination_error","message":"Unsupported examination profile from server"}))];
            }
            match ScriptMessage::decode(function, &arguments) {
                Some(Ok(ScriptMessage::OpenContainerDone { error, packs, items, container })) => {
                    json!({"event":"container_state","id":container.to_string(),"open":true,"status":error,
                        "items":packs.into_iter().flatten().chain(items.into_iter().flatten()).map(|id|id.to_string()).collect::<Vec<_>>()})
                }
                Some(Ok(ScriptMessage::CloseContainerDone { error, container })) => {
                    json!({"event":"container_state","id":container.to_string(),"open":false,"status":error})
                }
                Some(Ok(ScriptMessage::ContainerContents(c))) => {
                    json!({"event":"bag","id":object.map(|o|o.id.to_string()),"items":c.items.iter().map(u64::to_string).collect::<Vec<_>>()})
                }
                Some(Ok(ScriptMessage::Equipment(e))) => {
                    json!({"event":"equipment","slots":e.locations().unwrap_or_default().iter().map(|(slot,id)|json!({"slot":slot,"id":id.to_string()})).collect::<Vec<_>>()})
                }
                Some(Ok(ScriptMessage::MoveItemDone(d))) => {
                    json!({"event":"inventory_result","status":d.status(),"id":d.item().to_string(),"slot":d.actual_target_slot()})
                }
                Some(Ok(ScriptMessage::ReorganizeContentsDone(d))) => {
                    // Native drag ordinals are insertion boundaries in the
                    // pre-removal list. The bridge reports final cells.
                    let slot=d.actual_target_slot()-i32::from(d.from()==d.target() && d.actual_from_slot()<d.actual_target_slot());
                    json!({"event":"inventory_result","status":d.status(),"id":d.item().to_string(),"slot":slot})
                }
                Some(Ok(ScriptMessage::EquipItemDone(d))) => {
                    json!({"event":"inventory_result","status":d.status(),"id":d.item().to_string(),"equipped":d.location()})
                }
                Some(Ok(ScriptMessage::UnequipItemDone(d))) => {
                    json!({"event":"inventory_result","status":d.status(),"id":d.item().to_string(),"equipped":0,"slot":d.slot()})
                }
                Some(Ok(ScriptMessage::LootAllDone(d))) => {
                    json!({"event":"inventory_result","status":d.status(),"id":d.container().to_string()})
                }
                Some(Ok(ScriptMessage::UsageError { error, .. })) => {
                    json!({"event":"error","message":format!("Server rejected item use ({error})")})
                }
                Some(Ok(ScriptMessage::HearSpeech {
                    text, speaker_name, ..
                })) => {
                    json!({"event":"chat","kind":0x10,"text":string(&text),"speaker":string(&speaker_name)})
                }
                Some(Ok(ScriptMessage::HearTell {
                    text, teller_name, ..
                })) => {
                    json!({"event":"chat","kind":0x20,"text":string(&text),"speaker":string(&teller_name)})
                }
                Some(Ok(ScriptMessage::DisplayStringInfo { text, kind, .. })) => {
                    json!({"event":"chat","kind":kind,"text":string(&text)})
                }
                Some(Ok(ScriptMessage::DisplayText { text, kind })) => {
                    json!({"event":"chat","kind":kind,"text":{"literal":text}})
                }
                _ => {
                    json!({"event":"script","function":function,"object":object.map(|o|o.id.to_string())})
                }
            }
        }
        ServerEvent::CharExitGame => json!({"event":"selection"}),
        _ => return vec![],
    };
    vec![one]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_notices_are_readable_and_malformed_arguments_are_not_reinterpreted() {
        let text=StringInfo{literal:Some("Overseer: Bring the hammer.".into()),..Default::default()};
        let message=|arguments|ServerEvent::Interp{function:0x82120024,arguments,object:None};
        let args=pack_values(&[PackValue::Int(0),PackValue::Reference(PackedReference::native(NativeObject::StringInfo(text)))]);
        let out=convert(message(args));assert_eq!(out[0]["event"],"chat");
        assert_eq!(out[0]["text"]["literal"],"Overseer: Bring the hammer.");
        assert_eq!(out[1]["event"],"script");
        for args in [vec![],vec![0],pack_values(&[PackValue::Int(0),PackValue::Int(1)])] {
            assert!(!convert(message(args)).iter().any(|e|e["event"]=="chat"));
        }
    }
    #[test]
    fn native_container_open_close_and_failure_keep_identity_and_contents() {
        for error in [0, 17] {
            let message=ScriptMessage::OpenContainerDone{error,packs:Some(vec![]),items:Some(vec![0x8130000000000011]),container:0x8140000000010001};
            let events=convert(ServerEvent::Interp{function:message.function(),arguments:pack_values(&message.arguments()),object:None});
            assert_eq!(events[0]["event"],"container_state");assert_eq!(events[0]["open"],true);
            assert_eq!(events[0]["status"],error);assert_eq!(events[0]["id"],0x8140000000010001u64.to_string());
            assert_eq!(events[0]["items"][0],0x8130000000000011u64.to_string());
        }
        let message=ScriptMessage::CloseContainerDone{error:0,container:0x8140000000010001};
        let events=convert(ServerEvent::Interp{function:message.function(),arguments:pack_values(&message.arguments()),object:None});
        assert_eq!(events[0]["open"],false);assert_eq!(events[0]["status"],0);
    }
    #[test]
    fn consumed_item_removal_is_forwarded_without_losing_its_identity() {
        let id=0x813000000000002au64;
        let mut arguments=2u32.to_le_bytes().to_vec();arguments.extend(id.to_le_bytes());
        let message=|arguments|ServerEvent::Interp{function:0x820e0058,arguments,object:None};
        let event=convert(message(arguments.clone()));
        assert_eq!(event[0],json!({"event":"inventory_remove","id":id.to_string()}));
        arguments.pop();assert_eq!(convert(message(arguments))[0]["event"],"error");
    }
    #[test]
    fn global_creation_traits_and_clothing_survive_the_bridge() {
        let v=VisualDesc {parent:Some(0x1f000023),global_mods:Some(PartGroupDesc {
            appearance:vec![(0x20000014,vec![(0x10,1.0),(2,0.25)])],..Default::default()
        }),..Default::default()};
        let result=visual(&v);
        assert_eq!(result["aspects"].as_array().unwrap().len(),2);
        assert_eq!(result["aspects"][0]["key"],0x10);
        assert_eq!(result["aspects"][1]["value"],0.25);
    }
    fn object(action:ObjectAction)->ServerEvent {
        ServerEvent::Object(ObjectEvent {sender:InstanceIdWithStamp {id:0x8140000000030003,instance:7,other:11},action})
    }
    #[test]
    fn motion_changes_keep_zero_scales_and_descriptor_ids() {
        for (action,name,value) in [(ObjectAction::SetVelocityScale(0.),"velocity",0.),
            (ObjectAction::SetAccelerationScale(0.5),"acceleration",0.5),
            (ObjectAction::SetJumpScale(2.),"jump",2.)] {
            let v=convert(object(action));assert_eq!(v[0]["event"],"motion");
            assert_eq!(v[0]["parameter"],name);assert_eq!(v[0]["value"],value);
            assert_eq!(v[0]["instance"],7);
        }
        let v=convert(object(ObjectAction::SetMotionInterp(0x32000000)));
        assert_eq!(v[0]["parameter"],"descriptor");assert_eq!(v[0]["value"],0x32000000u32);
    }
    #[test]
    fn combat_actions_keep_native_behavior_mode_and_rate() {
        let p=BehaviorParams {behavior:0x4100001f,mode:0x41000005,time_scale:0.75,hold_cycles:0,..Default::default()};
        let v=convert(object(ObjectAction::DoBehavior(Box::new(p))));
        assert_eq!(v[0]["id"],"9313444029402382339");assert_eq!(v[0]["instance"],7);
        assert_eq!(v[0]["behavior"],0x4100001fu32);assert_eq!(v[0]["mode"],0x41000005u32);
        assert_eq!(v[0]["rate"],0.75);assert_eq!(v[0]["hold"],false);
        let v=convert(object(ObjectAction::DoMode(0x41000012)));
        assert_eq!(v[0]["mode"],0x41000012u32);
    }
    #[test]
    fn stories_forward_attack_and_damage_without_misattributing_stamps() {
        let s=PhysicsStory {attacker:Some(22),attacker_behavior:Some(BehaviorParams {behavior:0x41000089,propagate:1,..Default::default()}),
            hooks:vec![StoryHook {bitfield:1,target:Some(33),target_health_change:Some(-9),..Default::default()},
                StoryHook {bitfield:1,target:Some(44),target_health_change:Some(12),..Default::default()}],..Default::default()};
        let v=convert(object(ObjectAction::DoStory(Box::new(s))));
        assert_eq!(v.len(),2);assert_eq!(v[0]["id"],"22");assert!(v[0].get("instance").is_none());
        assert_eq!(v[0]["propagate"],true);
        assert_eq!(v[1]["id"],"33");assert_eq!(v[1]["damage"],9);
    }
    #[test]
    fn object_ids_preserve_all_64_bits() {
        let v = convert(ServerEvent::CreatePlayer {
            id: 0x8140000000030003,
            region: 1,
        });
        assert_eq!(v[0]["id"], "9313444029402382339");
    }
    #[test]
    fn native_missiles_keep_target_offsets_velocity_and_instance() {
        let p=Position{cell:0x02990109,origin:[1.,2.,3.],rotation:[1.,0.,0.,0.]};
        let v=convert(object(ObjectAction::InitializeMissile{position:p,velocity:[10.,0.,0.],target:Box::new(ac2_net::protocol::TargetPosition{target:0x8140000000030043,offset:p,velocity:[0.,1.,0.],initialized:1})}));
        assert_eq!(v[0]["event"],"missile");assert_eq!(v[0]["target"],0x8140000000030043u64.to_string());
        assert_eq!(v[0]["velocity"][0],10.);assert_eq!(v[0]["offset"]["position"][2],3.);assert_eq!(v[0]["initialized"],true);
        let v=convert(ServerEvent::CreateObject(Box::new(CreateObject{id:21,visual:VisualDesc::default(),weenie:WeenieDesc::default(),
            physics:PhysicsDesc{bitfield:0x80000,instance_stamp:3,position:Some(p),velocity:Some([10.,0.,0.]),target:Some(22),target_offset:Some([0.,0.,1.]),..Default::default()}})));
        assert_eq!(v[0]["event"],"object");assert_eq!(v[1]["event"],"missile");assert_eq!(v[1]["offset"]["position"][2],1.);
        assert_eq!(v[1]["instance"],3);assert_eq!(v[1]["target"],"22");
    }
    #[test]
    fn native_position_is_cell_local_and_heading_clockwise() {
        let v = position(&Position {
            cell: 0x02990109,
            origin: [59., 240., -44.5],
            rotation: [1., 0., 0., 0.],
        });
        assert_eq!(v["position"][2], -44.5);
        assert_eq!(v["heading"], 0.0);
        let tilted = position(&Position {cell:0x77b80006, origin:[1.,100.,182.5],
            rotation:[0.9238795,0.3826834,0.,0.]});
        assert_eq!(tilted["rotation"],json!([0.9238795f32,0.3826834f32,0.,0.]));
    }
}
