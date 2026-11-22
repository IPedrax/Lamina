//! The scene: the objects on the plate. Its origin is the middle of the plate and z = 0 is the plate's surface; a printer profile
//! says how big the plate is and where the machine's own zero lies.

use std::sync::Arc;

use crate::mesh::{MAX_COORD, Mesh, P3, Transform};

pub const MAX_OBJECTS: usize = 1_000;

#[derive(Clone, Debug)]
pub struct Object {
    pub id: u32,
    pub name: String,
    /// Shared, so an undo step that keeps the scene before an edit costs a pointer, not a copy of the mesh.
    pub mesh: Arc<Mesh>,
    pub xf: Transform,
}

impl Object {
    /// The mesh where it stands on the plate.
    pub fn world(&self) -> Mesh {
        self.mesh.transformed(&self.xf)
    }

    pub fn bounds(&self) -> Option<(P3, P3)> {
        self.world().bounds()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub objects: Vec<Object>,
    next_id: u32,
}

impl Scene {
    pub fn get(&self, id: u32) -> Result<&Object, String> {
        self.objects.iter().find(|o| o.id == id).ok_or_else(|| format!("no object with id {id}; `info` lists them"))
    }

    pub fn get_mut(&mut self, id: u32) -> Result<&mut Object, String> {
        self.objects.iter_mut().find(|o| o.id == id).ok_or_else(|| format!("no object with id {id}; `info` lists them"))
    }

    /// Puts `mesh` on the plate, centred on x and y with its lowest point at z = 0, and returns its id.
    pub fn add(&mut self, name: &str, mesh: Mesh) -> Result<u32, String> {
        if self.objects.len() >= MAX_OBJECTS {
            return Err(format!("the plate holds at most {MAX_OBJECTS} objects"));
        }
        let (lo, hi) = mesh.bounds().ok_or("the mesh is empty")?;
        let xf = Transform::translate([-(lo[0] + hi[0]) / 2.0, -(lo[1] + hi[1]) / 2.0, -lo[2]]);
        self.next_id += 1;
        self.objects.push(Object { id: self.next_id, name: name.to_string(), mesh: Arc::new(mesh), xf });
        Ok(self.next_id)
    }

    /// What every command must leave true: a transform that stays finite and an object that stays within reach.
    pub fn check(&self) -> Result<(), String> {
        for o in &self.objects {
            if !o.xf.is_finite() {
                return Err(format!("object {} has a transform that is not finite", o.id));
            }
            let out = o.bounds().is_none_or(|(lo, hi)| lo.iter().chain(hi.iter()).any(|c| !c.is_finite() || c.abs() > MAX_COORD));
            if out {
                return Err(format!("object {} lies beyond {MAX_COORD} mm of the origin", o.id));
            }
        }
        Ok(())
    }
}
