#![forbid(unsafe_code)]

//! Asheron's Call 2 physics: the `CPhysicsObj` movement step, `CMotionInterp` locomotion and
//! `CTransition` collision, ported from the retail client (admin 7148 decompile, checked against
//! the Legions 7160 DVD client instruction by instruction).
//!
//! Positions are retail `Position`s: a cell id and a frame in AC axes (z up) relative to the
//! cell's landblock. Arithmetic follows the retail x87 model (see `fpu`).
mod anim_rep;
mod bsp;
mod cells;
mod constraint;
mod envcell;
mod find_cells;
mod fpu;
mod frame;
mod heading;
mod interp;
pub mod landdefs;
mod loader;
mod los;
mod motion_interp;
mod motion_interp_behaviors;
mod moveto;
mod objects;
mod physics_obj;
mod physics_obj_contact;
mod physics_obj_place;
mod position;
mod shapes;
mod terrain;
mod transition;
mod vector;

pub use anim_rep::BodyAnimation;
pub use bsp::{BspTree, Polygon};
pub use cells::{CellSource, LandCell, TerrainPolygon};
pub use envcell::{CellBsp, CellBspNode, CellPortal, CellStructure, EnvCell};
pub use frame::Frame;
pub use heading::Heading;
pub use loader::{
    load_block_interiors, load_block_objects, load_body_shape, load_object_shape,
    load_terrain_block,
};
pub use los::{LosParameters, Ray, RayIntersection};
pub use motion_interp::MotionInterp;
pub use moveto::{
    MoveToResult, MovementParameters, PackedMovementParameters, MOVE_TO_AWAY, MOVE_TO_MOVE,
    MOVE_TO_OBJECT, MOVE_TO_PERSISTENT, MOVE_TO_TURN, MOVE_TO_USE_CYLINDERS,
};
pub use objects::{BBox, ObjectShape, StaticObject};
pub use physics_obj::{BodyShape, DescSlider};
pub use physics_obj_place::{SetPositionError, FORCE_SPF, SLIDE_SPF};
pub use position::Position;
pub use shapes::cylsphere::CylSphere;
pub use terrain::Terrain;
pub use vector::{Plane, Quaternion, Sphere, Vector};
