//! Retail CObjCell::LOSCheck over native terrain, interior BSPs and static physics.
//! This validates contacts/reach; it does not claim an authoritative movement solver.
use crate::movement::Placement;
use ac2_core::CellId;
use ac2_physics::{Frame,LosParameters,Position,Quaternion,Ray,Terrain,Vector};
use ac2_world::Ac2WorldSource;
use std::{collections::BTreeMap,io,path::Path,sync::Arc};

pub struct Geometry {
    world: Arc<Ac2WorldSource>,
    terrain: Terrain,
    blocks: BTreeMap<u32,(u64,bool)>,
    clock: u64,
    loads: u32,
}
fn error(e:impl std::fmt::Display)->io::Error {io::Error::other(e.to_string())}
impl Geometry {
    pub fn open(directory:&Path)->io::Result<Self> {
        let dataset=ac2_dat::DatDataset::open(ac2_dat::DatOpenOptions::new(directory)).map_err(error)?;
        let store=Arc::new(ac2_db::ResourceStore::new(dataset));
        let world=Ac2WorldSource::open(store).map_err(error)?.ok_or_else(||error("Missing AC2 world definitions"))?;
        Ok(Self{world,terrain:Terrain::new(),blocks:BTreeMap::new(),clock:0,loads:0})
    }
    fn resident(&mut self,cell:u32)->bool {
        let block=cell&0xffff0000;self.clock+=1;
        if let Some(entry)=self.blocks.get_mut(&block){entry.0=self.clock;return entry.1;}
        // The pinned Terrain keeps vacant object slots after remove_block.
        // Periodically rebuild this query-only cache so long journeys do not
        // accumulate an unbounded slot array. No live simulation state is here.
        if self.loads>=64 {self.terrain=Terrain::new();self.blocks.clear();self.loads=0;}
        self.loads+=1;
        if self.blocks.len()>=16 {
            if let Some((&old,_))=self.blocks.iter().min_by_key(|(_,e)|e.0){self.terrain.remove_block(old);self.blocks.remove(&old);}
        }
        let id=CellId::new(block|0xffff);
        let result=(||{
            // Dungeon-only blocks can have interior geometry without an outdoor mesh.
            let land=ac2_physics::load_terrain_block(&self.world,&mut self.terrain,id)?;
            let rooms=ac2_physics::load_block_interiors(&self.world,&mut self.terrain,id)?;
            ac2_physics::load_block_objects(&self.world,&mut self.terrain,id)?;
            Ok::<_,ac2_db::ResourceError>(land||rooms>0)
        })();
        let ready=result.unwrap_or_else(|e|{eprintln!("Native collision unavailable for {block:08x}: {e}");false});
        if !ready{self.terrain.remove_block(block);}
        self.blocks.insert(block,(self.clock,ready));ready
    }
    fn position(p:Placement,up:f32)->Position {
        Position::new(p.cell,Frame::new(Vector::new(p.coordinates[0],p.coordinates[1],p.coordinates[2]+up),Quaternion::new(1.,0.,0.,0.)))
    }
    /// Verified near-foot contact, including upper floors and bridges.
    pub fn contact(&mut self,p:Placement)->bool {
        if !p.valid() || !self.resident(p.cell){return false;}
        let mut ray=LosParameters::new();
        ray.set_from_position_and_offset(&Self::position(p,0.25),Vector::new(0.,0.,-0.55));
        self.terrain.los_check(&mut ray);
        ray.trace_result()==1 && ray.impact_normal().z>=0.57 && (ray.end_point().z-p.coordinates[2]).abs()<=0.3
    }
    /// Authoring guard for terrestrial actors; underwater floors are valid
    /// collision surfaces but must not become land-creature spawn anchors.
    pub fn dry_ground(&self,p:Placement)->bool {
        self.water_height(p.cell).is_ok_and(|water|
            water.is_none_or(|height|p.coordinates[2]>=height+0.15))
    }
    /// Explicit 74A9 shoreline repair, shared with AC2Water.cpp.
    /// A native plane still wins if later DATs restore the missing water cell.
    pub fn water_height(&self,cell:u32)->Result<Option<f32>,ac2_db::ResourceError> {
        let native=self.world.water_surface_height(CellId::new(cell))?;
        if native.is_none() && (cell >> 16)==0x74a9 && (cell & 0xffff)<0x100 {
            return self.world.water_surface_height(CellId::new(0x75a90001));
        }
        Ok(native)
    }
    /// No hit is only clear when the whole segment stayed in resident geometry.
    pub fn clear(&mut self,a:Placement,b:Placement,height:f32)->bool {
        self.traverse(a,b,height).is_some()
    }
    fn traverse(&mut self,a:Placement,b:Placement,height:f32)->Option<Placement> {
        if !a.valid()||!b.valid()||!self.resident(a.cell)||!self.resident(b.cell){return None;}
        let mut ray=LosParameters::new();
        ray.set_from_begin_and_end_position(&Self::position(a,height),&Self::position(b,height));
        self.terrain.los_check(&mut ray);
        if ray.trace_result()!=0{return None;}
        let end=ray.end_point();
        Some(Placement{cell:ray.final_cell_id(),coordinates:[end.x,end.y,end.z-height],heading:b.heading})
    }
    pub fn ground(&mut self,p:Placement,up:f32,down:f32)->Option<Placement>{
        if !p.valid()||!self.resident(p.cell){return None;}
        let mut ray=LosParameters::new();
        ray.set_ray(Ray::from_offset(Self::position(p,up).frame.origin,Vector::new(0.,0.,-up-down)),p.cell);
        self.terrain.los_check(&mut ray);
        if ray.trace_result()!=1||ray.impact_normal().z<0.57{return None;}
        let hit=ray.end_point();Some(Placement{cell:ray.final_cell_id(),coordinates:[hit.x,hit.y,hit.z],heading:p.heading})
    }
    /// Bounded walking step through native cell portals and outdoor boundaries.
    /// Three body rays reject wall edges; floor queries reject ledges and steps.
    /// This is conservative reach checking, not a full capsule movement solver.
    pub fn walk_step(&mut self,from:Placement,to:Placement,radius:f32)->Option<Placement>{
        if !radius.is_finite() || !(0.0..=1.0).contains(&radius) || from.distance(to).is_none_or(|d|d>1.) {return None;}
        let reached=self.traverse(from,to,0.6)?;
        let floor=self.ground(reached,0.45,0.65)?;
        if (floor.coordinates[2]-from.coordinates[2]).abs()>0.65{return None;}
        let [dx,dy,_]=from.offset_to(floor)?;let length=dx.hypot(dy);
        if length<=0.0001{return None;}
        for side in [-radius,0.,radius] {
            let offset=[-dy/length*side,dx/length*side,0.];
            let a=from.translated(offset)?;let b=floor.translated(offset)?;
            if !self.clear(a,b,0.6)||!self.clear(a,b,1.5){return None;}
        }
        Some(floor)
    }
}
