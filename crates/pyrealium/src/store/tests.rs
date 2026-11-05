    use super::*;
    #[test]
    fn invested_skill_untraining_refunds_earned_xp_and_survives_restart() {
        use crate::{skills::Command as S,progression::Command as P};
        let dir=crate::db::testdb::fresh();
        let partial;
        {
            let mut s=Store::open(&dir).unwrap();let mut creation=request("Skill Hero");creation.race=1;creation.sex=4096;
            s.create_native("local",&creation).unwrap();
            s.progression_command("local",1,P::AwardXp(10000)).unwrap();
            s.ensure_starter_skills("local",1).unwrap();
            for id in [0x59,0x55] {s.skill_command("local",1,S::Train(id)).unwrap();}
            s.raise_skill("local",1,0x55).unwrap();
            let invested=s.characters[&1].skills.get(0x55).unwrap().xp;assert!(invested>1);
            assert!(s.skill_command("local",1,S::Untrain(0x59)).is_err());
            s.skill_command("local",1,S::Untrain(0x55)).unwrap();
            assert!(!s.characters[&1].skills.usable(0x55));
            assert!(s.raise_skill("local",1,0x55).is_err());
            assert!(!s.debit_untraining(1).unwrap());
            let available=s.characters[&1].progression.available_xp;
            s.progression_command("local",1,P::AwardXp(1)).unwrap();
            assert!(s.debit_untraining(1).unwrap());
            assert_eq!(s.characters[&1].skills.get(0x55).unwrap().xp,invested-1);
            assert_eq!(s.characters[&1].progression.available_xp,available+2);
            assert!(!s.debit_untraining(1).unwrap());
            partial=s.characters[&1].clone();
        }
        {
            let mut s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1],partial);
            s.skill_command("local",1,S::CancelUntrain).unwrap();assert!(s.characters[&1].skills.usable(0x55));
            let remaining=s.characters[&1].skills.get(0x55).unwrap().xp;
            s.progression_command("local",1,P::AwardXp(10)).unwrap();assert!(!s.debit_untraining(1).unwrap());
            s.skill_command("local",1,S::Untrain(0x55)).unwrap();
            s.progression_command("local",1,P::AwardXp(remaining+20)).unwrap();s.debit_untraining(1).unwrap();
            assert!(s.characters[&1].skills.get(0x55).is_none());assert_eq!(s.characters[&1].skills.untraining_id(),0);
            assert_eq!(s.characters[&1].progression.available_xp,s.characters[&1].progression.xp);
        }
        {let s=Store::open(&dir).unwrap();assert!(s.characters[&1].skills.get(0x55).is_none());}
    }
    #[test]
    fn quest_emote_exchanges_replay_as_recorded() {
        use crate::{inventory::Command as I,quest_emotes::Trigger as T,shelter};
        let t=&crate::quest_emotes::shipped();
        let dir=crate::db::testdb::fresh();
        let room=crate::movement::Placement{cell:0x0299014d,coordinates:[1.0,1.0,1.19],heading:0};
        let final_state;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Shelter Hero")).unwrap();
            let step=|s:&mut Store,trigger,entity,iid,item:Option<u32>|{
                let ch=&s.characters[&c.id];
                let o=t.run(trigger,entity,iid,item,&ch.quests,&ch.inventory,&ch.progression,s.next_item_id().unwrap(),0.0,1_800_000_000,&shelter::item).unwrap().unwrap();
                s.emote_transaction("local",c.id,&o).unwrap();o
            };
            for npc in [0x4700157d,0x4700157e,0x47001581] {step(&mut s,T::Use,npc,1,None);}
            assert!(s.emote_transaction("other",c.id,&crate::quest_emotes::Outcome::default()).is_err());
            let id=s.next_item_id().unwrap();s.inventory_command("local",c.id,I::Grant(shelter::item(shelter::TOOL,id,0).unwrap())).unwrap();
            let xp=s.characters[&c.id].progression.xp;
            step(&mut s,T::Use,0x47001580,1,None);
            assert_eq!(s.characters[&c.id].progression.xp,xp+40);
            let id=s.next_item_id().unwrap();s.inventory_command("local",c.id,I::Grant(shelter::item(shelter::GLYPH,id,s.characters[&c.id].inventory.free_slot().unwrap()).unwrap())).unwrap();
            step(&mut s,T::ItemUsedOn,0x47001451,0x814002990005000d,Some(shelter::GLYPH));
            assert_eq!(s.placement(c.id),Some(room));
            step(&mut s,T::Use,0x4700157f,1,None);
            assert_eq!(s.characters[&c.id].progression.xp,xp+105);
            final_state=s.characters[&c.id].clone();
            assert_eq!(final_state.quests.shelter_phase(),6);
            // The run's start and solve times are part of the records and replay with them.
            assert_eq!(final_state.quests.timer(0x4000ff05),crate::quests::Timer{started:1_800_000_000,completed:1_800_000_000,solves:1});
        }
        {let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&1],final_state);assert_eq!(s.placement(1),Some(room));}
    }
    #[test]
    fn a_quest_effect_record_replays() {
        let dir=crate::db::testdb::fresh();
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Cursed One")).unwrap();
            let on=crate::quest_emotes::Outcome{quests:vec![crate::quests::Command::EffectOn{effect:0x6f00020f,at:1000,until:87_400}],..Default::default()};
            s.emote_transaction("local",c.id,&on).unwrap();
            saved=s.characters[&c.id].clone();
            assert_eq!(saved.quests.quest_effects(2000),[(0x6f00020f,87_400)]);
        }
        {let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&saved.id],saved);}
        {
            let mut s=Store::open(&dir).unwrap();
            let off=crate::quest_emotes::Outcome{quests:vec![crate::quests::Command::EffectOff{effect:0x6f00020f}],..Default::default()};
            s.emote_transaction("local",saved.id,&off).unwrap();
        }
        {let s=Store::open(&dir).unwrap();assert!(s.characters[&saved.id].quests.quest_effects(2000).is_empty());}
    }
    #[test]
    fn legacy_hunts_migrate_once_and_the_migration_replays() {
        let t=crate::quest_emotes::shipped();
        let dir=crate::db::testdb::fresh();
        let (marsh,mucor)=(0x4000fc20,0x4000fc23);
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Hunt Mover")).unwrap();
            // State an older server left for hunts that were code: one now an AC2 Vault quest
            // (a stage per kill), one now a hand-written hunt (underway / goal met / done).
            s.characters.get_mut(&c.id).unwrap().quests.hunting.0.insert(marsh,crate::legacy_quests::HuntState{count:3,started:500,completed:0,rewarded_xp:0});
            s.characters.get_mut(&c.id).unwrap().quests.hunting.0.insert(mucor,crate::legacy_quests::HuntState{count:2,started:600,completed:0,rewarded_xp:0});
            // An inferred quest underway and one finished.
            s.characters.get_mut(&c.id).unwrap().quests.recovered.0.insert(0x40000097,crate::legacy_quests::RunState{count:2,started:700,completed:0});
            s.characters.get_mut(&c.id).unwrap().quests.recovered.0.insert(0x4000000a,crate::legacy_quests::RunState{count:10,started:100,completed:900});
            // An obelisk search two obelisks in, and the Arwic lesson at its craft with the recipe.
            s.characters.get_mut(&c.id).unwrap().quests.exploration.0.insert(0x40000016,crate::legacy_quests::SearchState{phase:3,started:800,completed:0});
            s.characters.get_mut(&c.id).unwrap().quests.arwic=crate::legacy_quests::Lesson{phase:3,started:850,completed:0,learned:true};
            assert_eq!(s.migrate_legacy_quests(&t).unwrap(),1);
            assert_eq!(s.migrate_legacy_quests(&t).unwrap(),0,"once");
            saved=s.characters[&c.id].clone();
            assert_eq!((saved.quests.stage(marsh),saved.quests.timer(marsh).started),(4,500));
            assert_eq!((saved.quests.stage(mucor),saved.quests.count(mucor),saved.quests.timer(mucor).started),(1,2,600));
            assert!(saved.quests.hunting.0.is_empty() && saved.quests.recovered.0.is_empty());
            assert_eq!((saved.quests.stage(0x40000097),saved.quests.count(0x40000097)),(1,2));
            assert!(t.solved(&saved.quests,0x4000000a) && saved.quests.timer(0x4000000a).completed==900);
            assert_eq!((saved.quests.stage(0x40000016),saved.quests.stage(0x400000c1)),(3,3));
            assert!(saved.quests.knows(0x6f001f0e) && saved.quests.exploration.0.is_empty() && saved.quests.arwic.phase==0);
        }
        // The migration marked the character changed, so its whole state was saved.
        let mut s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&saved.id],saved);
        assert_eq!(s.migrate_legacy_quests(&t).unwrap(),0,"once, across a restart");
    }
    #[test]
    fn a_full_bag_refuses_the_scout_without_taking_the_tool() {
        use crate::{inventory::{Command as I,BAG_SLOTS},quest_emotes::Trigger as T,shelter};
        let t=&crate::quest_emotes::shipped();
        let dir=crate::db::testdb::fresh();
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Full Pack")).unwrap();
            for slot in 0..BAG_SLOTS {
                let id=s.next_item_id().unwrap();
                s.inventory_command("local",c.id,I::Grant(shelter::item(if slot==0 {shelter::TOOL}else{0x47001583},id,slot).unwrap())).unwrap();
            }
            let ch=&s.characters[&c.id];
            let book=ch.quests.apply(&crate::quests::Command::Stage{quest:0x4000ff05,stage:3}).unwrap();
            assert!(t.run(T::Use,0x47001580,1,None,&book,&ch.inventory,&ch.progression,s.next_item_id().unwrap(),0.0,0,&shelter::item).is_err());
        }
    }
    #[test]
    fn deletion_restoration_keeps_identity_appearance_and_saved_placement() {
        let dir=crate::db::testdb::fresh();
        {
            let mut store = Store::open(&dir).unwrap();
            for i in 1..=5 {
                store
                    .create_native("local", &request(&format!("Hero {i}")))
                    .unwrap();
            }
            store.set_placement(5, Placement::HUMAN_START).unwrap();
            store.save_now().unwrap();
            assert!(store.delete("other", 5).is_err());
            store.delete("local", 5).unwrap();
            assert!(store.set_placement(5, Placement::HUMAN_START).is_err());
            assert!(store.delete("local", 5).is_err());
            assert_eq!(store.create("local", "Sixth Hero").unwrap().id, 6);
            assert!(store.restore("local", 5).is_err());
            store.delete("local", 6).unwrap();
            store.restore("local", 5).unwrap();
            assert!(store.restore("other", 6).is_err());
            assert!(store.create("other", "Sixth Hero").is_err());
        }
        {
            let store = Store::open(&dir).unwrap();
            assert!(!store.characters[&5].deleted);
            assert!(store.characters[&6].deleted);
            assert_eq!(store.characters[&5].creation, Some(request("Hero 5")));
            assert_eq!(store.placement(5), Some(Placement::HUMAN_START));
        }
    }
    #[test]
    fn character_names_take_every_client_language_s_letters() {
        for name in ["Hero 5", "D'Artagnan", "Émile", "Jürgen", "한글이", "ㅎㅏㄴ", "龍", &"한".repeat(32)] {
            assert!(super::valid_character_name(name), "{name}");
        }
        for name in ["", "Zoë", "a\tb", "Ñandú", &"한".repeat(33)] {
            assert!(!super::valid_character_name(name), "{name}");
        }
        assert!(!super::valid_name("Jürgen"), "accounts stay ASCII");
    }
    fn request(name: &str) -> CharacterCreationRequest {
        CharacterCreationRequest {
            name: name.into(),
            entity_did: 0x10001234,
            start_area: 7,
            race: 2,
            sex: 1,
            class: 3,
            implement: 4,
            physical_values: vec![(9, (-0.0f32).to_bits()), (2, 0.1234567f32.to_bits())],
        }
    }
    #[test]
    fn an_inferred_quest_and_rolled_armor_survive_restart_atomically() {
        use crate::{inventory::{Item,Roll,Location,Command as I,ITEM_NAMESPACE},progression::Command as P};
        let t=crate::quest_emotes::shipped();
        // Quest 0x40000097 (the inferred catalog's evidence): a starter kill bestows it, its targets count.
        let catalog:serde_json::Value=serde_json::from_str(crate::content_db::files::text("inferred-content-v1.json")).unwrap();
        let q=catalog["quests"].as_array().unwrap().iter().find(|q|q["id"].as_u64()==Some(0x40000097)).unwrap();
        let (starter,target)=(q["kill_starters"][0].as_u64().unwrap() as u32,q["targets"][0].as_u64().unwrap() as u32);
        let (goal,xp)=(q["goal"].as_u64().unwrap(),q["xp"].as_u64().unwrap());
        let dir=crate::db::testdb::fresh();
        let armor=Item{roll:Some(Roll{armor:17,tint:731,level:7,..Default::default()}),id:ITEM_NAMESPACE+1,
            definition:0x47000fdc,visual:0x1f00058c,quantity:1,stack_limit:1,equipment_mask:16,location:Location::Bag(0)};
        let before;let saved;
        {
            let mut s=Store::open(&dir).unwrap();s.create("local","Recovered Hero").unwrap();
            s.progression_command("local",1,P::AwardXp(10000)).unwrap();
            s.inventory_command("local",1,I::Grant(armor.clone())).unwrap();
            s.inventory_command("local",1,I::Wear{item:armor.id,slot:16}).unwrap();
            assert_eq!(crate::apparel::armor(&s.characters[&1].inventory,1),17);
            assert!(crate::apparel::aspects(&s.characters[&1].inventory,1,0x1000).iter().any(|(_,a,v)|*a==2&&(*v-0.731).abs()<0.0001));
            before=s.characters[&1].progression.xp;
            let normal=[P::AwardXp(1)];
            s.encounter_reward("local",1,starter,0,&normal).unwrap();
            assert_eq!(s.characters[&1].quests.stage(0x40000097),1,"the starter bestows it");
            for _ in 0..goal {s.encounter_reward("local",1,target,0,&normal).unwrap();}
            assert_eq!(s.characters[&1].progression.xp,before+goal+1+xp);
            assert!(t.solved(&s.characters[&1].quests,0x40000097));
            // A starter kill within the repeat delay bestows nothing.
            s.encounter_reward("local",1,starter,0,&normal).unwrap();
            assert_eq!(s.characters[&1].quests.stage(0x40000097),2);
            s.save_now().unwrap();
            saved=s.characters[&1].clone();
        }
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&1],saved);
        assert_eq!(crate::apparel::armor(&s.characters[&1].inventory,1),17);
    }
    #[test]
    fn mixed_versions_retain_exact_native_appearance_after_restart() {
        let dir=crate::db::testdb::fresh();
        let original = request("Native Hero");
        {
            let mut store = Store::open(&dir).unwrap();
            assert_eq!(store.create("local", "Legacy Hero").unwrap().creation, None);
            let created = store.create_native("local", &original).unwrap();
            assert_eq!(created.creation.as_ref(), Some(&original));
            assert!(
                store
                    .create_native("other", &request("native hero"))
                    .is_err()
            );
            for i in 0..3 {
                store
                    .create_native("local", &request(&format!("Extra {i}")))
                    .unwrap();
            }
            assert!(store.create_native("local", &request("Sixth")).is_err());
        }
        {
            let store = Store::open(&dir).unwrap();
            assert_eq!(store.characters.len(), 5);
            assert_eq!(store.characters[&1].creation, None);
            assert_eq!(store.characters[&2].creation.as_ref(), Some(&original));
        }
    }
    #[test]
    fn periodic_saves_run_when_due_and_a_crash_keeps_the_last_save() {
        let dir=crate::db::testdb::fresh();
        let mut s=Store::open(&dir).unwrap();
        let id=s.create("local","Saver").unwrap().id;
        s.progression_command("local",id,crate::progression::Command::AwardXp(100)).unwrap();
        assert!(!s.save_due().unwrap(),"the interval has not passed");
        s.set_save_interval(std::time::Duration::ZERO);
        assert!(s.save_due().unwrap());
        s.writer().drain().unwrap();
        s.progression_command("local",id,crate::progression::Command::AwardXp(50)).unwrap();
        s.abandon();
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&id].progression.xp,100,"the unsaved award is lost with the crash, the saved one is kept");
    }
    #[test]
    fn ground_items_and_both_owners_save_in_one_snapshot() {
        let dir=crate::db::testdb::fresh();
        let (a,b,ground,expected);
        {
            let mut s=Store::open(&dir).unwrap();
            a=s.create("local","Dropper").unwrap().id;
            b=s.create("local","Finder").unwrap().id;
            let item=s.next_item_id().unwrap();
            s.inventory_command("local",a,crate::inventory::Command::Grant(crate::item_services::potion(item,0))).unwrap();
            let dropped=s.drop_item("local",a,item,Placement::HUMAN_START).unwrap();
            ground=dropped.id();
            s.save_now().unwrap();
            {let r=Store::open(&dir);assert!(r.is_err(),"a second server on the same database is refused");}
            s.pickup_item("local",b,ground).unwrap();
            expected=(s.characters[&a].clone(),s.characters[&b].clone());
        }
        let s=Store::open(&dir).unwrap();
        assert!(s.drops.is_empty() && !s.drops.contains_key(&ground));
        assert_eq!((s.characters[&a].clone(),s.characters[&b].clone()),expected);
        assert_eq!(s.characters[&b].inventory.items().count(),1);
        assert_eq!(s.characters[&a].inventory.items().count(),0);
    }
    #[test]
    fn offline_credit_is_saved_where_sql_can_read_it() {
        let dir=crate::db::testdb::fresh();
        let id;
        {
            let mut s=Store::open(&dir).unwrap();
            id=s.create("local","Vassal").unwrap().id;
            s.credit_offline(id,250).unwrap();
            assert!(s.credit_offline(id+1,1).is_err());
        }
        let client=dir.connect().unwrap();
        let row:(i64,i64)=client.query_row("SELECT xp, available_xp FROM characters WHERE id=?1",[id as i64],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(row,(250,250));
        // An external SQL credit of an offline character is read at the next start.
        client.execute("UPDATE characters SET xp=xp+10, available_xp=available_xp+10 WHERE id=?1",[id as i64]).unwrap();
        drop(client);
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&id].progression.xp,260);
    }
    #[test]
    fn names_are_unique_and_creation_requests_are_validated() {
        let dir=crate::db::testdb::fresh();
        {
            let mut s=Store::open(&dir).unwrap();
            assert_eq!(s.create("local","Explorer").unwrap().id,1);
            assert!(s.create("local","explorer").is_err());
            let mut bad=request("Bad Values");
            bad.physical_values.push(bad.physical_values[0]);
            assert!(s.create_native("local",&bad).is_err(),"duplicate physical value");
            bad.physical_values=vec![(1,f32::NAN.to_bits())];
            assert!(s.create_native("local",&bad).is_err(),"non-finite physical value");
            bad.physical_values=(0..257).map(|i|(i,0)).collect();
            assert!(s.create_native("local",&bad).is_err(),"too many physical values");
            assert_eq!(s.characters.len(),1,"refused requests create nothing");
        }
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&1].name,"Explorer");
        assert_eq!(s.characters.len(),1);
    }
    #[test]
    fn placements_save_and_unknown_characters_are_refused() {
        let dir=crate::db::testdb::fresh();
        let mut last=Placement::HUMAN_START;
        {
            let mut s=Store::open(&dir).unwrap();
            s.create("local","Walker").unwrap();
            assert!(s.set_placement(2,last).is_err());
            s.set_placement(1,last).unwrap();
            s.save_now().unwrap();
            last.coordinates[0]+=4.0;
            s.set_placement(1,last).unwrap();
        }
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.placement(1),Some(last));
    }
    #[test]
    fn chat_options_survive_restart() {
        let dir=crate::db::testdb::fresh();
        let chat=crate::preferences::ChatOptions{filters:std::collections::BTreeMap::from([(1,0x60016f)]),..Default::default()};
        {
            let mut s=Store::open(&dir).unwrap();
            s.create("local","Talker").unwrap();
            assert!(s.update_chat_options("other",1,chat.clone()).is_err());
            s.update_chat_options("local",1,chat.clone()).unwrap();
            s.save_now().unwrap();
        }
        let s=Store::open(&dir).unwrap();
        assert_eq!(s.characters[&1].chat_options.as_ref(),Some(&chat));
    }
    #[test]
    fn a_trade_swap_changes_both_packs_or_neither() {
        use crate::inventory::{Command, Item, Location, ITEM_NAMESPACE};
        let dir=crate::db::testdb::fresh();
        let item=|id:u64,quantity:u32|Item{roll:None,id,definition:0x4700_1108,visual:0x1f00_01cf,quantity,stack_limit:1000,equipment_mask:0,location:Location::Bag(0)};
        let mut s=Store::open(&dir).unwrap();
        s.create("a","Giver").unwrap();
        s.create("b","Taker").unwrap();
        let given=ITEM_NAMESPACE+1;
        s.player_transaction("a",1,&[Command::GrantFront(item(given,10))],&[]).unwrap();
        let first=s.next_item_id().unwrap();
        // Part of the stack to the other side.
        s.swap_items(1,&[Command::ConsumePart{item:given,quantity:4}],2,&[Command::GrantFront(item(first,4))],first).unwrap();
        assert_eq!(s.characters[&1].inventory.items().next().unwrap().quantity,6);
        assert_eq!(s.characters[&2].inventory.items().map(|i|(i.id,i.quantity)).collect::<Vec<_>>(),vec![(first,4)]);
        // The giver no longer has 7: neither pack changes.
        let next=s.next_item_id().unwrap();
        let before=(s.characters[&1].inventory.clone(),s.characters[&2].inventory.clone());
        assert!(s.swap_items(2,&[Command::GrantFront(item(next,7))],1,&[Command::ConsumePart{item:given,quantity:7}],next).is_err());
        assert_eq!((s.characters[&1].inventory.clone(),s.characters[&2].inventory.clone()),before);
    }
    #[test]
    fn avatar_lifetimes_survive_restart() {
        let dir=crate::db::testdb::fresh();
        {
            let mut store=Store::open(&dir).unwrap();
            store.create("local","Hero").unwrap();
            assert!(store.begin_instance("other",1).is_err());
            assert_eq!(store.begin_instance("local",1).unwrap(),1);
            store.delete("local",1).unwrap();
            assert!(store.begin_instance("local",1).is_err());
            store.restore("local",1).unwrap();
        }
        let mut store=Store::open(&dir).unwrap();
        assert_eq!(store.begin_instance("local",1).unwrap(),2);
        store.characters.get_mut(&1).unwrap().instance=u16::MAX;
        assert!(store.begin_instance("local",1).is_err());
    }
    #[test]
    fn an_encounter_payout_round_trips() {
        let dir=crate::db::testdb::fresh();
        let saved;
        {
            let mut s=Store::open(&dir).unwrap();let c=s.create_native("local",&request("Kill Time")).unwrap();
            let id=s.next_item_id().unwrap();
            let items=[crate::inventory::Command::Grant(crate::item_services::potion(id,0))];
            s.encounter_transaction("local",c.id,0x4700158b,0,&items,&[crate::progression::Command::AwardXp(5)]).unwrap();
            saved=s.characters[&c.id].clone();
        }
        let s=Store::open(&dir).unwrap();assert_eq!(s.characters[&saved.id],saved);
    }
