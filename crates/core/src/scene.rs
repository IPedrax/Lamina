//! The scene: the objects on the plate. Its origin is the middle of the plate and z = 0 is the plate's surface; a printer profile
//! says how big the plate is and where the machine's own zero lies.

use std::sync::Arc;

use crate::mesh::{MAX_COORD, Mesh, P3, Transform};

pub const MAX_OBJECTS: usize = 1_000;

/// What an object is for when it is not a part to print: a volume that tells the slicer something about the parts it meets. Kept
/// in projects; the slicer prints nothing of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    /// Supports under every overhang of a part inside it, even with supports off.
    SupportEnforcer,
    /// No supports for the overhangs of a part inside it.
    SupportBlocker,
}

#[derive(Clone, Debug)]
pub struct Object {
    pub id: u32,
    pub name: String,
    /// Shared, so an undo step that keeps the scene before an edit costs a pointer, not a copy of the mesh.
    pub mesh: Arc<Mesh>,
    pub xf: Transform,
    /// The commands that made it (`build`), which `edit` changes and runs again; none for an object made any other way.
    pub recipe: Option<Arc<serde_json::Value>>,
    /// Slicer settings of its own (`set_object_settings`): profile fields laid over the plate's for this object alone.
    pub settings: Option<Arc<serde_json::Value>>,
    /// A modifier is not printed (`set_modifier`); none for a part.
    pub modifier: Option<Modifier>,
    /// Settings by height (`set_height_range`): a list of `{from, to, profile}` the slicer reads, kept as it was given.
    pub ranges: Option<Arc<serde_json::Value>>,
    /// Paint on its mesh's triangles (`paint`), one [`Paint`] tag each, as long as the mesh has triangles; none when unpainted.
    pub paint: Option<Arc<Vec<u8>>>,
}

/// What a face is painted: where the seam goes, or where supports must or must not hold it up. The tag kept per triangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    None = 0,
    Seam = 1,
    Support = 2,
    Block = 3,
}

impl Paint {
    pub fn from_tag(tag: u8) -> Paint {
        match tag {
            1 => Paint::Seam,
            2 => Paint::Support,
            3 => Paint::Block,
            _ => Paint::None,
        }
    }
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
        self.objects.push(Object {
            id: self.next_id,
            name: name.to_string(),
            mesh: Arc::new(mesh),
            xf,
            recipe: None,
            settings: None,
            modifier: None,
            ranges: None,
            paint: None,
        });
        Ok(self.next_id)
    }

    /// Adds a mesh that already stands where it should (the result of a boolean), keeping its coordinates, and returns its id.
    pub fn add_placed(&mut self, name: &str, mesh: Mesh) -> Result<u32, String> {
        if self.objects.len() >= MAX_OBJECTS {
            return Err(format!("the plate holds at most {MAX_OBJECTS} objects"));
        }
        if mesh.bounds().is_none() {
            return Err("the mesh is empty".into());
        }
        self.next_id += 1;
        self.objects.push(Object {
            id: self.next_id,
            name: name.to_string(),
            mesh: Arc::new(mesh),
            xf: Transform::IDENTITY,
            recipe: None,
            settings: None,
            modifier: None,
            ranges: None,
            paint: None,
        });
        Ok(self.next_id)
    }

    /// Adds a mesh with the transform that places it, and returns its id.
    pub fn add_transformed(&mut self, name: &str, mesh: Mesh, xf: Transform) -> Result<u32, String> {
        self.add_shared(name, Arc::new(mesh), xf)
    }

    /// Adds an object sharing a mesh another already holds (a copy costs a pointer), and returns its id.
    pub fn add_shared(&mut self, name: &str, mesh: Arc<Mesh>, xf: Transform) -> Result<u32, String> {
        if self.objects.len() >= MAX_OBJECTS {
            return Err(format!("the plate holds at most {MAX_OBJECTS} objects"));
        }
        if mesh.bounds().is_none() {
            return Err("the mesh is empty".into());
        }
        self.next_id += 1;
        self.objects.push(Object {
            id: self.next_id,
            name: name.to_string(),
            mesh,
            xf,
            recipe: None,
            settings: None,
            modifier: None,
            ranges: None,
            paint: None,
        });
        Ok(self.next_id)
    }

    /// A scene of objects read from a project, keeping their ids: each must be unique, and the next new object gets one above them all.
    pub fn restore(objects: Vec<Object>) -> Result<Scene, String> {
        if objects.len() > MAX_OBJECTS {
            return Err(format!("the plate holds at most {MAX_OBJECTS} objects"));
        }
        let mut ids: Vec<u32> = objects.iter().map(|o| o.id).collect();
        ids.sort_unstable();
        if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
            return Err(format!("two objects have the id {}", w[0]));
        }
        let next_id = ids.last().copied().unwrap_or(0);
        Ok(Scene { objects, next_id })
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
