//! Bounded server-authored placements. DAT presence is checked before publication.
use crate::{
    dat::Dat,
    movement::Placement,
    world::{self, StationaryObject},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, Read},
    path::Path,
    sync::Arc,
};

fn duplicate_landmark(a: Placement, b: Placement) -> bool {
    // Different outdoor subcells (and blocks) can contain one revised snapshot
    // of a landmark. Never merge overlapping but unrelated dungeon rooms.
    let outdoors = |p: Placement| (1..=64).contains(&(p.cell & 0xffff));
    (a.cell == b.cell || outdoors(a) && outdoors(b))
        && a.distance(b).is_some_and(|distance| distance <= 12.0)
}

#[cfg(test)]
mod landmark_tests {
    use super::*;
    #[test]
    fn arwic_snapshots_cross_cell_boundary_but_separate_rooms_do_not_merge() {
        let a = Placement {cell:0x76b80022,coordinates:[85.5575,29.5398,182.505],heading:0};
        let b = Placement {cell:0x76b8001a,coordinates:[77.260666,27.62573,182.5],heading:0};
        assert!(duplicate_landmark(a,b));
        assert!(!duplicate_landmark(a,Placement {coordinates:[100.,29.5,182.5],..a}));
        assert!(!duplicate_landmark(Placement {cell:0x02990109,..a},Placement {cell:0x0299010a,..a}));
        assert!(duplicate_landmark(Placement {cell:0x76b80040,coordinates:[159.,20.,182.5],..a},
            Placement {cell:0x77b80001,coordinates:[1.,20.,182.5],..a}));
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;
    fn spawn(point: Placement, instance:u16) -> Vec<u8> {
        world::create_stationary_object(&StationaryObject {character_id:NAMESPACE+1,
            parent_visual_did:0x1f0003d1,entity_did:crate::quests::TARGET_ENTITY,
            cell_id:point.cell,position:point.coordinates,orientation:point.orientation(),
            instance_stamp:instance,visual_ordering_stamp:0,timestamps:[0;4]}).unwrap()
    }
    #[test]
    fn pursuit_updates_late_observers_and_respawn_returns_to_authored_home() {
        let home=Placement::HUMAN_START;
        let id=NAMESPACE+1;
        let mut content=Content::default();
        content.objects.insert(id,Object {id,entity:crate::quests::TARGET_ENTITY,visual:0x1f0003d1,scale:1.,placement:home,home,home_orientation:home.orientation(),
            spawn:spawn(home,1).into(),destroy:vec![0;16].into(),active:true,instance:1,
            visual_stamp:0,position_stamp:0,stats:BTreeMap::new(),service:None,name:None,corpse_name:None,door_open:false,door_mode:None});
        content.set_stat(id,0x107,60).unwrap();
        let mut next=home;next.coordinates[0]+=0.5;next.heading=64;
        content.move_creature(id,next,2500).unwrap();
        assert_eq!(&*content.object(id).unwrap().spawn,spawn(next,1));
        let mut wrong=next;wrong.cell+=0x10000;
        assert!(content.move_creature(id,wrong,2600).is_err());
        assert_eq!(content.object(id).unwrap().placement,next);
        content.despawn(id).unwrap();content.respawn(id).unwrap();
        assert_eq!(&*content.object(id).unwrap().spawn,spawn(home,2));
        assert_eq!(content.object(id).unwrap().placement,home);
    }
    #[test]
    fn captured_creature_layouts_preserve_pose_scale_name_and_respawn() {
        for visual in [0x1f000031,0x1f000deb,0x1f0003d1] {
            let home=Placement::HUMAN_START;let id=NAMESPACE+2;
            let mut content=Content::fixture(id,home,None);
            let rotation=[-0.06770791,0.,0.,0.99770516];
            let raw=world::create_named_stationary_object(&StationaryObject {
                character_id:id,parent_visual_did:visual,entity_did:0x4700088c,
                cell_id:home.cell,position:home.coordinates,orientation:rotation,
                instance_stamp:1,visual_ordering_stamp:0,timestamps:[0;4]},Some(0.75),Some("Captured creature")).unwrap();
            let offset=Content::position_offset(&raw).unwrap();
            let tail=raw[offset+32..].to_vec();
            content.objects.get_mut(&id).unwrap().spawn=raw.into();
            content.objects.get_mut(&id).unwrap().home_orientation=rotation;
            assert!(content.object(id).unwrap().supports_pose_updates());
            let mut next=home;next.coordinates[0]+=0.5;next.heading=32;
            let update=content.move_creature(id,next,1000).unwrap();
            assert_eq!(&update[40..44],&0x207f7f7fu32.to_le_bytes(),
                "authoritative steps must not advertise backwards/downward input");
            assert!(!content.idol_presentation(id,crate::combat_visual::IdolAction::Swing).unwrap().is_empty());
            let spawn=&content.object(id).unwrap().spawn;
            let offset=Content::position_offset(spawn).unwrap();
            assert_eq!(&spawn[offset+32..],tail);
            assert_eq!(&spawn[offset+4..offset+8],next.coordinates[0].to_le_bytes());
            assert_eq!(&spawn[28..32],0x41000012u32.to_le_bytes());
            // Fresh observers get the same mode/pose; respawn returns to the
            // captured home without corrupting trailing name/scale properties.
            content.idol_presentation(id,crate::combat_visual::IdolAction::Death).unwrap();
            content.despawn(id).unwrap();content.respawn(id).unwrap();
            let spawn=&content.object(id).unwrap().spawn;
            assert_eq!(content.object(id).unwrap().placement,home);
            assert_eq!(&spawn[24..26],2u16.to_le_bytes());
            assert_eq!(&spawn[28..32],crate::combat_visual::PEACE.to_le_bytes());
            assert_eq!(&spawn[offset+4..offset+8],home.coordinates[0].to_le_bytes());
            for (i,v) in rotation.iter().enumerate(){
                assert_eq!(&spawn[offset+16+i*4..offset+20+i*4],v.to_le_bytes(),"Respawn must retain the exact captured quaternion");
            }
            assert_eq!(&spawn[offset+40..],&tail[8..]);
        }
        for bytes in [vec![0;24],vec![0;75]] {assert!(Content::position_offset(&bytes).is_err());}
    }
}

pub const NAMESPACE: u64 = 0x8140000000000000;
impl Object {
    /// A stationary world object created while the server runs (a prospected deposit): the same
    /// CreateObject a placement row builds, with `service` set by the caller.
    pub fn world_object(id: u64, entity: u32, visual: u32, point: Placement, name: Option<&str>, service: Option<crate::services::Service>) -> io::Result<Object> {
        let spawn = world::create_named_stationary_object(
            &StationaryObject {
                character_id: id,
                parent_visual_did: visual,
                entity_did: entity,
                cell_id: point.cell,
                position: point.coordinates,
                orientation: point.orientation(),
                instance_stamp: 1,
                visual_ordering_stamp: 0,
                timestamps: [0; 4],
            },
            None,
            name,
        )
        .map_err(|error| invalid(format!("world object: {error}")))?;
        let mut destroy = 0x100c2u32.to_le_bytes().to_vec();
        destroy.extend(id.to_le_bytes());
        destroy.extend(1u16.to_le_bytes());
        destroy.extend(0u16.to_le_bytes());
        Ok(Object {
            id, entity, visual, scale: 1., name: name.map(str::to_owned), corpse_name: None, service,
            door_open: false, door_mode: None, placement: point, home: point,
            home_orientation: point.orientation(), spawn: spawn.into(), destroy: destroy.into(),
            active: true, instance: 1, visual_stamp: 0, position_stamp: 0, stats: BTreeMap::new(),
        })
    }
}
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_OBJECTS: usize = 16384;
const MAX_CELL_OBJECTS: usize = 64;
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
#[derive(Clone)]
pub struct Object {
    pub id: u64,
    pub entity: u32,
    pub visual: u32,
    pub scale: f32,
    pub name: Option<String>,
    corpse_name: Option<Arc<[u8]>>,
    pub service: Option<crate::services::Service>,
    door_open: bool,
    door_mode: Option<Arc<[u8]>>,
    pub placement: Placement,
    home: Placement,
    // Keep the authored quaternion; the movement heading is only eight bits.
    home_orientation: [f32; 4],
    pub spawn: Arc<[u8]>,
    pub destroy: Arc<[u8]>,
    active: bool,
    instance: u16,
    visual_stamp: u16,
    position_stamp: u16,
    stats: BTreeMap<u32, (u32, Arc<[u8]>)>,
}
impl Object {
    /// The orientation the content placed the object at (w, x, y, z).
    pub fn home_orientation(&self) -> [f32; 4] {
        self.home_orientation
    }
    pub fn resource(id:u64,entity:u32,visual:u32,p:Placement)->io::Result<Self>{
        let spawn=world::create_stationary_object(&StationaryObject{character_id:id,parent_visual_did:visual,entity_did:entity,cell_id:p.cell,position:p.coordinates,orientation:p.orientation(),instance_stamp:1,visual_ordering_stamp:0,timestamps:[0;4]}).map_err(io::Error::other)?;
        Ok(Self{id,entity,visual,scale:1.,name:None,corpse_name:None,service:None,door_open:false,door_mode:None,placement:p,home:p,home_orientation:p.orientation(),spawn:spawn.into(),destroy:world::destroy_inventory_item(id,1).into(),active:true,instance:1,visual_stamp:0,position_stamp:0,stats:BTreeMap::new()})
    }
    pub fn instance_stamp(&self)->u16{self.instance}
    /// Packet capability, not an AI classification. A mode/scale slider is
    /// optional in captured PhysicsDesc records and must not gate pursuit.
    pub fn supports_pose_updates(&self) -> bool {
        Content::position_offset(&self.spawn).is_ok()
    }
    pub fn stat(&self, key: u32) -> Option<u32> {
        self.stats.get(&key).map(|(value, _)| *value)
    }
    pub fn updates(&self) -> impl Iterator<Item = &Arc<[u8]>> {
        self.corpse_name.iter().chain(self.door_mode.iter()).chain(self.stats
            .get(&0x107)
            .map(|(_, wire)| wire)
            .into_iter()
            .chain(
                self.stats
                    .iter()
                    .filter(|(key, _)| **key != 0x107)
                    .map(|(_, (_, wire))| wire),
            ))
    }
    fn stat_wire(&self, key: u32, value: u32) -> Arc<[u8]> {
        world::visual_int(self.id, self.instance, key, value).into()
    }
}
#[derive(Default)]
pub struct Content {
    objects: BTreeMap<u64, Object>,
    cells: BTreeMap<u32, Vec<u64>>,
    /// Last instance of each object a live edit removed, so a re-added object
    /// is a newer lifetime to clients that saw the old one.
    retired: BTreeMap<u64, u16>,
}
impl Content {
    #[cfg(test)]
    pub(crate) fn fixture(id:u64,point:Placement,service:Option<crate::services::Service>)->Self {
        let spawn=world::create_stationary_object(&StationaryObject {character_id:id,parent_visual_did:0x1f0003d1,
            entity_did:crate::quests::TARGET_ENTITY,cell_id:point.cell,position:point.coordinates,orientation:point.orientation(),
            instance_stamp:1,visual_ordering_stamp:0,timestamps:[0;4]}).unwrap();
        let mut result=Self::default();
        result.objects.insert(id,Object{id,entity:crate::quests::TARGET_ENTITY,visual:0x1f0003d1,scale:1.,name:None,corpse_name:None,service,door_open:false,door_mode:None,
            placement:point,home:point,home_orientation:point.orientation(),spawn:spawn.into(),destroy:world::destroy_inventory_item(id,1).into(),active:true,
            instance:1,visual_stamp:0,position_stamp:0,stats:BTreeMap::new()});
        result.cells.insert(point.cell,vec![id]);
        if service.is_none(){result.set_stat(id,0x107,60).unwrap();}
        result
    }
    pub fn set_chest_open(&mut self,id:u64,open:bool)->io::Result<Option<Arc<[u8]>>> {
        let object=self.objects.get_mut(&id).filter(|o|o.active).ok_or_else(||invalid("Chest unavailable."))?;
        if object.service!=Some(crate::services::Service::Chest{locked:false}) {return Err(invalid("Chest cannot be opened."));}
        if object.door_open==open{return Ok(None);}
        let stamp=object.visual_stamp.checked_add(2).ok_or_else(||invalid("Chest state limit reached."))?;
        let wire:Arc<[u8]>=crate::services::door_mode(id,object.instance,stamp,open).into();
        object.visual_stamp=stamp;object.door_open=open;object.door_mode=Some(wire.clone());
        Ok(Some(wire))
    }
    pub fn toggle_door(&mut self,id:u64)->io::Result<Arc<[u8]>> {
        let object=self.objects.get_mut(&id).filter(|o|o.active).ok_or_else(||invalid("Door unavailable."))?;
        match object.service {
            Some(crate::services::Service::Door{locked:false})=>{},
            Some(crate::services::Service::Door{locked:true})=>return Err(invalid("This door is locked or requires a quest/key.")),
            _=>return Err(invalid("This object is not a door.")),
        }
        object.visual_stamp=object.visual_stamp.checked_add(2).ok_or_else(||invalid("Door state limit reached."))?;
        object.door_open=!object.door_open;
        let wire:Arc<[u8]>=crate::services::door_mode(id,object.instance,object.visual_stamp,object.door_open).into();
        object.door_mode=Some(wire.clone());Ok(wire)
    }

    pub fn validate_drop(&self,dropped:&crate::store::DroppedItem) -> io::Result<()> {
        if !dropped.placement.valid() || self.objects.contains_key(&dropped.id()) || self.objects.len()>=MAX_OBJECTS {
            return Err(invalid("Ground item capacity exhausted"));
        }
        Ok(())
    }
    pub fn insert_drop(&mut self, dropped: &crate::store::DroppedItem) -> io::Result<()> {
        self.validate_drop(dropped)?;
        let id=dropped.id();let p=dropped.placement;
        let spawn=world::create_stationary_object(&StationaryObject {
            character_id:id,parent_visual_did:dropped.item.visual,entity_did:dropped.item.definition,
            cell_id:p.cell,position:p.coordinates,orientation:p.orientation(),instance_stamp:1,
            visual_ordering_stamp:1,timestamps:[0;4]}).map_err(io::Error::other)?;
        self.objects.insert(id,Object {id,entity:dropped.item.definition,visual:dropped.item.visual,scale:1.,placement:p,home:p,home_orientation:p.orientation(),spawn:spawn.into(),
            destroy:world::destroy_inventory_item(id,1).into(),active:true,instance:1,visual_stamp:1,position_stamp:0,stats:BTreeMap::new(),service:None,name:None,corpse_name:None,door_open:false,door_mode:None});
        self.cells.entry(p.cell).or_default().push(id);Ok(())
    }
    pub fn remove_drop(&mut self,id:u64) -> io::Result<Arc<[u8]>> {
        if id>>48!=0x8180 {return Err(invalid("Not a ground item"));}
        let object=self.objects.remove(&id).ok_or_else(||invalid("Ground item unavailable"))?;
        if let Some(cell)=self.cells.get_mut(&object.placement.cell) {cell.retain(|v|*v!=id);}
        Ok(object.destroy)
    }
    /// Bounded creature pursuit. Retain the new pose in CreateObject
    /// too, so late observers receive the same location as existing observers.
    pub fn move_creature(&mut self, id: u64, placement: Placement, now: u64) -> io::Result<Arc<[u8]>> {
        self.place_creature(id, placement, now, true)
    }
    /// An unbounded creature relocation (an escort rejoining its player after a portal).
    pub fn warp_creature(&mut self, id: u64, placement: Placement, now: u64) -> io::Result<Arc<[u8]>> {
        self.place_creature(id, placement, now, false)
    }
    fn place_creature(&mut self, id: u64, placement: Placement, now: u64, bounded: bool) -> io::Result<Arc<[u8]>> {
        let object = self.objects.get_mut(&id).filter(|o| o.active && o.stat(0x107).is_some() && o.service.is_none())
            .ok_or_else(|| invalid("unsupported moving creature"))?;
        if bounded && object.placement.distance(placement).is_none_or(|d|d>crate::creature_motion::MAX_UPDATE_DISTANCE) {
            return Err(invalid("creature movement exceeds bounded validated update"));
        }
        let old_cell=object.placement.cell;
        let stamp=object.position_stamp.checked_add(1).ok_or_else(|| invalid("creature position stamp exhausted"))?;
        Self::set_pose(object, placement)?;
        object.position_stamp=stamp;
        let update = crate::movement::ClientPosition {
            // Position is authoritative here. Zero motion is encoded at the
            // midpoint of each signed byte, not 0 (which means -1 on all axes).
            time: now as f64 / 1000.0, placement,
            motion: (u32::from(placement.heading) << 24) | 0x007f7f7f,
            cancel_moveto: false, stamps: [object.instance,0,0,0], jump_velocity: None,
        };
        let wire=crate::movement::server_position(id,object.instance,object.position_stamp,&update)
            .map(Arc::from).map_err(io::Error::other)?;
        self.reindex(id,old_cell,placement.cell);
        Ok(wire)
    }
    fn reindex(&mut self,id:u64,old:u32,new:u32) {
        if old==new{return;}
        if let Some(ids)=self.cells.get_mut(&old){ids.retain(|entry|*entry!=id);if ids.is_empty(){self.cells.remove(&old);}}
        let ids=self.cells.entry(new).or_default();if !ids.contains(&id){ids.push(id);}
    }
    fn set_pose(object: &mut Object, placement: Placement) -> io::Result<()> {
        // Captured creatures can omit mode/sliders in their initial descriptor.
        // Use the layouts emitted by our serializer, retaining every other field.
        let offset=Self::position_offset(&object.spawn)?;
        let spawn = Arc::make_mut(&mut object.spawn);
        spawn[offset..offset+4].copy_from_slice(&placement.cell.to_le_bytes());
        for (i,v) in placement.coordinates.iter().enumerate() { spawn[offset+4+i*4..offset+8+i*4].copy_from_slice(&v.to_le_bytes()); }
        for (i,v) in placement.orientation().iter().enumerate() {spawn[offset+16+i*4..offset+20+i*4].copy_from_slice(&v.to_le_bytes());}
        object.placement=placement;
        Ok(())
    }
    fn position_offset(spawn:&[u8])->io::Result<usize> {
        let flags=spawn.get(20..24).and_then(|s|s.try_into().ok()).map(u32::from_le_bytes);
        let offset=match flags {
            Some(0x8000)=>28,
            Some(0x8080)=>32,
            Some(0x8082) if spawn.get(32..36)==Some(1u32.to_le_bytes().as_slice())=>48,
            _=>return Err(invalid("unsupported creature physics layout")),
        };
        if spawn.len()<offset+48{return Err(invalid("truncated creature physics layout"));}
        Ok(offset)
    }
    fn ensure_mode(object:&mut Object)->io::Result<()> {
        if Self::position_offset(&object.spawn)?==28 {
            // PhysicsDesc mode bit precedes Position. Adding a mode must not
            // invent a scale slider or overwrite the creature's captured pose.
            let mut spawn=object.spawn.to_vec();
            spawn[20..24].copy_from_slice(&0x8080u32.to_le_bytes());
            spawn.splice(28..28,crate::combat_visual::PEACE.to_le_bytes());
            object.spawn=spawn.into();
        }
        Ok(())
    }
    /// The shipped snapshot works independently of the current directory.
    pub fn bundled(game_data: &Path) -> io::Result<Self> {
        Self::parse(include_str!("../content/world.tsv"), game_data)
    }
    pub fn observed(game_data: &Path) -> io::Result<Self> {
        Self::parse(
            include_str!("../content/world-observed-expanded.tsv"),
            game_data,
        )
    }
    pub fn training(game_data: &Path) -> io::Result<Self> {
        let mut combined = String::from("pyrealium-world-v3\n");
        // Keep the retained actor snapshot when enabling the training preset;
        // replacing it with the portal/lifestone-only base leaves known actors
        // absent even though the server reports that world content is enabled.
        for line in include_str!("../content/world-observed-expanded.tsv")
            .lines()
            .skip(1)
        {
            combined.push_str(line);
            combined.push('\n');
        }
        for line in include_str!("../content/training-world.tsv")
            .lines()
            .skip(1)
        {
            combined.push_str(line);
            combined.push('\n');
        }
        // Raw captures span client builds with reused visual DIDs. They must go
        // through Network/prepare_content.py against the operator's DAT set.
        // The normal launcher passes that validated manifest via --content.
        Self::parse(&combined, game_data)
    }
    pub fn len(&self) -> usize {
        self.objects.len()
    }
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
    pub fn cell(&self, id: u64) -> Option<u32> {
        self.objects
            .get(&id)
            .filter(|object| object.active)
            .map(|object| object.placement.cell)
    }
    pub fn objects(&self) -> impl Iterator<Item = &Object> {
        self.objects.values()
    }
    pub fn object(&self, id: u64) -> Option<&Object> {
        self.objects.get(&id)
    }
    pub fn corpse_name(&mut self,id:u64,names:&crate::examination::Names)->io::Result<Arc<[u8]>> {
        let o=self.objects.get_mut(&id).filter(|o|o.active && o.stat(0x102)==Some(0) && o.service.is_none()).ok_or_else(||invalid("not a creature corpse"))?;
        if let Some(wire)=&o.corpse_name {return Ok(wire.clone());}
        let name=crate::examination::Name::Corpse(Box::new(o.name.clone().map(crate::examination::Name::Literal).unwrap_or_else(||names.get(o.entity,"Creature"))));
        let mut wire=0x100bdu32.to_le_bytes().to_vec();
        wire.extend(o.id.to_le_bytes());wire.extend(o.instance.to_le_bytes());wire.extend(0u16.to_le_bytes());wire.extend(0x100u32.to_le_bytes());
        wire.extend(name.wire().map_err(|e|invalid(format!("corpse name: {e}")))?);
        let wire:Arc<[u8]>=wire.into();o.corpse_name=Some(wire.clone());Ok(wire)
    }
    pub fn set_stat(&mut self, id: u64, key: u32, value: u32) -> io::Result<Option<Arc<[u8]>>> {
        let object = self
            .objects
            .get_mut(&id)
            .ok_or_else(|| invalid("unknown content object"))?;
        if object.stats.get(&key).is_some_and(|(old, _)| *old == value) {
            return Ok(None);
        }
        if object.stats.len() >= 16 && !object.stats.contains_key(&key) {
            return Err(invalid("content stat limit"));
        }
        let wire = object.stat_wire(key, value);
        object.stats.insert(key, (value, wire.clone()));
        Ok(object.active.then_some(wire))
    }
    pub fn despawn(&mut self, id: u64) -> io::Result<Arc<[u8]>> {
        let object = self
            .objects
            .get_mut(&id)
            .filter(|object| object.active)
            .ok_or_else(|| invalid("content object not active"))?;
        object.active = false;
        Ok(object.destroy.clone())
    }
    pub fn idol_presentation(
        &mut self,
        id: u64,
        action: crate::combat_visual::IdolAction,
    ) -> io::Result<Vec<Arc<[u8]>>> {
        let object = self
            .objects
            .get_mut(&id)
            .filter(|object| object.active)
            .ok_or_else(|| invalid("inactive creature presentation"))?;
        // Mode/behavior identifiers are shared; each client resolves its body's
        // own motion and behavior graphs. Only live encounter actors qualify.
        if object.stat(0x107).is_none() || object.service.is_some() {
            return Ok(Vec::new());
        }
        Self::ensure_mode(object)?;
        let (stamp, mode, events) =
            crate::combat_visual::idol(id, object.instance, object.visual_stamp, action)
                .map_err(|error| invalid(format!("creature presentation: {error}")))?;
        let spawn = Arc::make_mut(&mut object.spawn);
        spawn[26..28].copy_from_slice(&stamp.to_le_bytes());
        spawn[28..32].copy_from_slice(&mode.to_le_bytes());
        object.visual_stamp = stamp;
        Ok(events.into_iter().map(Arc::from).collect())
    }
    /// Next visual stamp on the victim. The story is presentation only.
    pub fn damage_story(
        &mut self,
        victim: u64,
        attacker: u64,
        health_change: i32,
    ) -> io::Result<Arc<[u8]>> {
        let object = self
            .objects
            .get_mut(&victim)
            .filter(|object| object.active)
            .ok_or_else(|| invalid("inactive creature damage story"))?;
        let stamp = object
            .visual_stamp
            .checked_add(1)
            .ok_or_else(|| invalid("creature visual stamp exhausted"))?;
        let wire = crate::combat_visual::damage_story(
            victim,
            object.instance,
            stamp,
            attacker,
            health_change,
        )
        .map_err(|error| invalid(format!("creature damage story: {error}")))?;
        if object.spawn.len() >= 28 {
            Arc::make_mut(&mut object.spawn)[26..28].copy_from_slice(&stamp.to_le_bytes());
        }
        object.visual_stamp = stamp;
        Ok(wire.into())
    }
    pub fn effect(&mut self,id:u64,fx:u32) -> io::Result<Arc<[u8]>> {
        let object=self.objects.get_mut(&id).filter(|o|o.active).ok_or_else(||invalid("inactive effect owner"))?;
        let stamp=object.visual_stamp.checked_add(1).ok_or_else(||invalid("effect stamp exhausted"))?;
        let wire=crate::combat_visual::effect(id,object.instance,stamp,fx).map_err(|e|invalid(e.to_string()))?;
        Arc::make_mut(&mut object.spawn)[26..28].copy_from_slice(&stamp.to_le_bytes());object.visual_stamp=stamp;Ok(wire.into())
    }
    pub fn respawn(&mut self, id: u64) -> io::Result<()> {
        let object = self
            .objects
            .get_mut(&id)
            .filter(|object| !object.active)
            .ok_or_else(|| invalid("content object already active"))?;
        let old_cell=object.placement.cell;
        if object.placement != object.home { Self::set_pose(object,object.home)?; }
        object.position_stamp=0;
        object.corpse_name=None;
        object.instance = object
            .instance
            .checked_add(1)
            .ok_or_else(|| invalid("content lifetime exhausted"))?;
        Arc::make_mut(&mut object.spawn)[24..26].copy_from_slice(&object.instance.to_le_bytes());
        object.visual_stamp = 0;
        Arc::make_mut(&mut object.spawn)[26..28].copy_from_slice(&0u16.to_le_bytes());
        if object.stat(0x107).is_some() {
            Self::ensure_mode(object)?;
            Arc::make_mut(&mut object.spawn)[28..32]
                .copy_from_slice(&crate::combat_visual::PEACE.to_le_bytes());
        }
        let offset=Self::position_offset(&object.spawn)?;
        for (i,v) in object.home_orientation.iter().enumerate() {
            Arc::make_mut(&mut object.spawn)[offset+16+i*4..offset+20+i*4].copy_from_slice(&v.to_le_bytes());
        }
        Arc::make_mut(&mut object.destroy)[12..14].copy_from_slice(&object.instance.to_le_bytes());
        let keys = object
            .stats
            .iter()
            .map(|(&key, (value, _))| (key, *value))
            .collect::<Vec<_>>();
        for (key, value) in keys {
            let wire = object.stat_wire(key, value);
            object.stats.insert(key, (value, wire));
        }
        object.active = true;
        let home_cell=object.home.cell;
        self.reindex(id,old_cell,home_cell);
        Ok(())
    }
    pub fn at(&self, cell: u32) -> impl Iterator<Item = &Object> {
        self.cells
            .get(&cell)
            .into_iter()
            .flatten()
            .map(|id| &self.objects[id])
            .filter(|object| object.active)
    }
    /// V1 uses IID/entity/visual/cell/X/Y/Z/heading; v2 replaces heading with
    /// quaternion W/X/Y/Z. Numeric identifiers are hex; positions are decimal.
    pub fn load(path: &Path, game_data: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(invalid("world content exceeds 4 MiB"));
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid("world content is not UTF-8"))?;
        Self::parse(text, game_data)
    }
    /// World-builder text parsed off the game thread; DAT checks use their own handles.
    pub fn parse_text(text: &str, game_data: &Path) -> io::Result<Self> {
        Self::parse(text, game_data)
    }
    pub fn into_objects(self) -> BTreeMap<u64, Object> {
        self.objects
    }
    /// Live edit precheck: after `removed` leave, can every `added` object be placed?
    pub fn check_live(&self, removed: &[u64], added: &[Object]) -> io::Result<()> {
        let gone = removed.iter().copied().collect::<BTreeSet<_>>();
        let fresh=added.iter().map(|o|o.id).collect::<BTreeSet<_>>();
        if gone.len()!=removed.len() || fresh.len()!=added.len() {
            return Err(invalid("duplicate live edit identity"));
        }
        if added.iter().any(|o|o.spawn.len()<26 || o.destroy.len()<14) {
            return Err(invalid("truncated live object messages"));
        }
        if let Some(id) = gone.iter().find(|id| !self.objects.contains_key(id)) {
            return Err(invalid(format!("object {id:016x} is not placed")));
        }
        let mut counts = BTreeMap::<u32, usize>::new();
        for object in added {
            if self.objects.contains_key(&object.id) && !gone.contains(&object.id) {
                return Err(invalid(format!("object {:016x} is already placed by the base world", object.id)));
            }
            *counts.entry(object.placement.cell).or_default() += 1;
        }
        for (cell, extra) in counts {
            let kept = self.cells.get(&cell).map_or(0, |ids| ids.iter().filter(|id| !gone.contains(id)).count());
            if kept + extra > MAX_CELL_OBJECTS {
                return Err(invalid(format!("cell {cell:08x} would hold more than 64 objects")));
            }
        }
        if self.objects.len() - gone.len() + added.len() > MAX_OBJECTS {
            return Err(invalid("the world object limit would be exceeded"));
        }
        Ok(())
    }
    /// Live edit: add a prepared object. An inactive one waits for its spawner.
    pub fn insert_live(&mut self, mut object: Object, active: bool) -> io::Result<()> {
        let id = object.id;
        if self.objects.len() >= MAX_OBJECTS || self.objects.contains_key(&id) {
            return Err(invalid(format!("object {id:016x} duplicates content or exceeds the world limit")));
        }
        let cell = object.placement.cell;
        if self.cells.get(&cell).is_some_and(|ids| ids.len() >= MAX_CELL_OBJECTS) {
            return Err(invalid(format!("cell {cell:08x} already holds 64 objects")));
        }
        object.instance = self.retired.remove(&id).map_or(1, |old| old.saturating_add(1)).max(1);
        Arc::make_mut(&mut object.spawn)[24..26].copy_from_slice(&object.instance.to_le_bytes());
        Arc::make_mut(&mut object.destroy)[12..14].copy_from_slice(&object.instance.to_le_bytes());
        object.active = active;
        self.cells.entry(cell).or_default().push(id);
        self.objects.insert(id, object);
        Ok(())
    }
    /// Live edit: forget an object; returns its destroy message if clients see it.
    pub fn remove_live(&mut self, id: u64) -> io::Result<Option<Arc<[u8]>>> {
        let object = self.objects.remove(&id).ok_or_else(|| invalid(format!("object {id:016x} is not placed")))?;
        if let Some(ids) = self.cells.get_mut(&object.placement.cell) {
            ids.retain(|entry| *entry != id);
            if ids.is_empty() { self.cells.remove(&object.placement.cell); }
        }
        self.retired.insert(id, object.instance);
        Ok(object.active.then_some(object.destroy))
    }
    /// Operator overlay placements join the selected base pack. Identities and
    /// the per-cell limit are checked against the combined world.
    pub fn extend(&mut self, path: &Path, game_data: &Path) -> io::Result<usize> {
        let overlay = Self::load(path, game_data)?;
        let count = overlay.objects.len();
        for (id, object) in overlay.objects {
            if self.objects.len() >= MAX_OBJECTS || self.objects.contains_key(&id) {
                return Err(invalid(format!("overlay object {id:016x} duplicates base content or exceeds the world limit")));
            }
            let cell = self.cells.entry(object.placement.cell).or_default();
            if cell.len() >= MAX_CELL_OBJECTS {
                return Err(invalid(format!("overlay cell {:08x} exceeds 64 objects", object.placement.cell)));
            }
            cell.push(id);
            cell.sort_unstable();
            self.objects.insert(id, object);
        }
        Ok(count)
    }
    fn parse(text: &str, game_data: &Path) -> io::Result<Self> {
        if text.len() as u64 > MAX_BYTES {
            return Err(invalid("world content exceeds 4 MiB"));
        }
        let mut lines = text.lines();
        let version = match lines.next() {
            Some("pyrealium-world-v1") => 1,
            Some("pyrealium-world-v2") => 2,
            Some("pyrealium-world-v3") => 3,
            Some("pyrealium-world-v4") => 4,
            _ => return Err(invalid("unsupported world content header")),
        };
        let mut content = Self::default();
        let mut portal = Dat::open(game_data.join("portal.dat"))?;
        let mut profiles=if portal.find(0x34000000)?.is_some(){Some(crate::services::Profiles::open(&mut portal)?)}else{None};
        let mut cells = Dat::open(game_data.join("cell_1.dat"))?;
        let mut assets = BTreeSet::new();
        let mut checked_cells = BTreeSet::new();
        for (line, text) in lines.enumerate() {
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            let parse = || -> io::Result<(u64, u32, u32, Placement, [f32; 4], Option<f32>)> {
                if text.len() > 512 {
                    return Err(invalid("placement exceeds 512 bytes"));
                }
                let fields: Vec<_> = text.split('\t').collect();
                if fields.len()
                    != match version {
                        1 => 8,
                        2 => 11,
                        3 => 12,
                        _ => 13,
                    }
                {
                    return Err(invalid(
                        "placement field count does not match manifest version",
                    ));
                }
                let hex = |value: &str| {
                    u64::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16)
                        .map_err(|_| invalid("invalid hex identifier"))
                };
                let narrow = |index: usize| {
                    u32::try_from(hex(fields[index])?)
                        .map_err(|_| invalid("identifier exceeds u32"))
                };
                let id = hex(fields[0])?;
                let entity = narrow(1)?;
                let visual = narrow(2)?;
                let placement = Placement {
                    cell: narrow(3)?,
                    coordinates: [
                        fields[4].parse().map_err(|_| invalid("invalid X"))?,
                        fields[5].parse().map_err(|_| invalid("invalid Y"))?,
                        fields[6].parse().map_err(|_| invalid("invalid Z"))?,
                    ],
                    heading: if version >= 2 {
                        0
                    } else {
                        fields[7].parse().map_err(|_| invalid("invalid heading"))?
                    },
                };
                let orientation = if version >= 2 {
                    let mut rotation = [0.0f32; 4];
                    for (value, text) in rotation.iter_mut().zip(&fields[7..11]) {
                        *value = text.parse().map_err(|_| invalid("invalid quaternion"))?;
                    }
                    rotation
                } else {
                    placement.orientation()
                };
                let norm: f32 = orientation.iter().map(|value| value * value).sum();
                if !orientation.iter().all(|value| value.is_finite())
                    || !norm.is_finite()
                    || (norm - 1.0).abs() > 0.01
                {
                    return Err(invalid("invalid or non-normalized content quaternion"));
                }
                if id >> 48 != NAMESPACE >> 48
                    || id == NAMESPACE
                    || entity >> 24 != 0x47
                    || visual >> 24 != 0x1f
                    || !placement.valid()
                {
                    return Err(invalid(
                        "invalid placement identity, asset type or position",
                    ));
                }
                let scale = if version >= 3 && fields[11] != "-" {
                    let scale: f32 = fields[11]
                        .parse()
                        .map_err(|_| invalid("invalid content scale"))?;
                    if !scale.is_finite() || scale <= 0.0 || scale > 100.0 {
                        return Err(invalid("content scale outside local bounds"));
                    }
                    Some(scale)
                } else {
                    None
                };
                Ok((id, entity, visual, placement, orientation, scale))
            };
            let (id, entity, visual, placement, orientation, scale) = parse()
                .map_err(|error| invalid(format!("world content line {}: {error}", line + 2)))?;
            if content.objects.len() >= MAX_OBJECTS || content.objects.contains_key(&id) {
                return Err(invalid("world content object limit or duplicate IID"));
            }
            // Expanded and captured snapshots contain the same lifestones with
            // different IIDs and slightly revised coordinates. Keep the first
            // IID so persisted bindings remain valid; never coalesce creatures.
            if entity == 0x470007de && content.objects.values().any(|old| {
                old.entity == entity && duplicate_landmark(old.placement, placement)
            }) { continue; }
            let cell_ids = content.cells.entry(placement.cell).or_default();
            if cell_ids.len() >= MAX_CELL_OBJECTS {
                return Err(invalid("world content exceeds 64 objects in one cell"));
            }
            for did in [entity, visual] {
                if assets.insert(did) {
                    portal
                        .read(did)
                        .map_err(|error| invalid(format!("content asset {did:#010x}: {error}")))?;
                }
            }
            let cell_record = crate::dat::world_cell_record(placement.cell)?;
            if checked_cells.insert(cell_record) {
                cells.read(cell_record).map_err(|error| {
                    invalid(format!("content cell {:#010x}: {error}", placement.cell))
                })?;
            }
            let captured_name = if version==4 {text.split('\t').nth(12).filter(|n|*n!="-")}else{None};
            if captured_name.is_some_and(|n|n.is_empty()||n.len()>256||n.chars().any(char::is_control)) {
                return Err(invalid("invalid captured object name"));
            }
            let name = captured_name.or(match id {
                0x8140000000030002 | 0x8140000000030004 | 0x8140000000030049 => Some("Arwic Outland Portal"),
                0x8140000000030005 | 0x8140000000030022 | 0x8140000000030025 => Some("Human Shelter Portal"),
                0x8140000000030030 => Some("Lugian Shelter Portal"),
                0x8140000000030031 => Some("Tumerok Shelter Portal"),
                0x8140000000030047 => Some("Menhir Vault Portal"),
                _ if entity == 0x4700084d => Some("Portal"),
                _ => None,
            });
            let spawn = world::create_named_stationary_object(
                &StationaryObject {
                    character_id: id,
                    parent_visual_did: visual,
                    entity_did: entity,
                    cell_id: placement.cell,
                    position: placement.coordinates,
                    orientation,
                    instance_stamp: 1,
                    visual_ordering_stamp: 0,
                    timestamps: [0; 4],
                },
                scale,
                name,
            )
            .map_err(|error| invalid(format!("content object: {error}")))?;
            let mut destroy = 0x100c2u32.to_le_bytes().to_vec();
            destroy.extend(id.to_le_bytes());
            destroy.extend(1u16.to_le_bytes());
            destroy.extend(0u16.to_le_bytes());
            let service=profiles.as_mut().and_then(|p|p.get(&mut portal,entity));
            let door_mode=matches!(service,Some(crate::services::Service::Door{..}|crate::services::Service::Chest{..})).then(||crate::services::door_mode(id,1,1,false).into());
            content.objects.insert(
                id,
                Object {
                    id,
                    entity,
                    visual,
                    scale:scale.unwrap_or(1.),
                    name:name.map(str::to_owned),corpse_name:None,
                    placement,
                    home: placement,
                    home_orientation: orientation,
                    spawn: spawn.into(),
                    destroy: destroy.into(),
                    active: true,
                    instance: 1,
                    visual_stamp: 0,
                    position_stamp: 0,
                    stats: BTreeMap::new(),service,door_open:false,door_mode,
                },
            );
            cell_ids.push(id);
        }
        for ids in content.cells.values_mut() {
            ids.sort_unstable();
        }
        Ok(content)
    }
}
