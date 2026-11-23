//! Project files (`.lamina`): the plate and the print settings, so closing the window loses nothing.
//!
//! A project is JSON: `{"format": 1, "generator", "objects": [...], "print": {...}}`. Each object is its id, name, transform
//! (`{"m": rows of a 3x3 matrix, "t": translation}`) and local mesh, as two base64 strings of little-endian arrays: `vertices`
//! (three `f32` each) and `triangles` (three `u32` each), which is lossless and about a third of the size of numbers in text.
//! `print` is the front end's print settings and preset labels, which the core keeps without reading (the slicer lays its
//! `profile` under what a slice is given).
//!
//! Read kindly and hostilely: a key this version does not know is left out and listed, a newer `format` is refused, and every
//! mesh, transform and id is checked as a file is checked.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::base64;
use crate::mesh::{Mesh, Transform};
use crate::scene::{Object, Scene};

/// The format this version writes, and the newest it reads.
pub const FORMAT: u64 = 1;
/// The largest project file read, bytes.
pub const MAX_FILE: u64 = 512 << 20;
/// The extension, without its dot.
pub const EXTENSION: &str = "lamina";

/// A project read from a file.
#[derive(Debug)]
pub struct Loaded {
    pub scene: Scene,
    pub print: Value,
    /// Keys left out because this version does not know them, as `objects[2].colour`.
    pub ignored: Vec<String>,
}

/// The project text for `scene` with the front end's `print` settings.
pub fn write(scene: &Scene, print: &Value) -> String {
    let objects: Vec<Value> = scene.objects.iter().map(object_json).collect();
    let mut v = json!({
        "format": FORMAT,
        "generator": format!("Lamina {}", env!("CARGO_PKG_VERSION")),
        "objects": objects,
    });
    if !print.is_null() {
        v["print"] = print.clone();
    }
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

fn object_json(o: &Object) -> Value {
    let vertices: Vec<u8> = o.mesh.vertices.iter().flatten().flat_map(|c| c.to_le_bytes()).collect();
    let triangles: Vec<u8> = o.mesh.triangles.iter().flatten().flat_map(|i| i.to_le_bytes()).collect();
    json!({
        "id": o.id,
        "name": o.name,
        "transform": {"m": o.xf.m, "t": o.xf.t},
        "vertices": base64::encode(&vertices),
        "triangles": base64::encode(&triangles),
    })
}

/// A project from its text.
pub fn read(text: &str) -> Result<Loaded, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a Lamina project: {e}"))?;
    let Value::Object(top) = v else { return Err("not a Lamina project: the file is not a JSON object".into()) };
    let format = top.get("format").and_then(Value::as_u64).ok_or("not a Lamina project: it has no `format` number")?;
    if format > FORMAT {
        return Err(format!(
            "this project is format {format}, made by a newer Lamina; this version reads format {FORMAT} and older: update Lamina to open it"
        ));
    }
    if format == 0 {
        return Err("not a Lamina project: `format` 0".into());
    }
    let mut ignored = Vec::new();
    for key in top.keys() {
        if !matches!(key.as_str(), "format" | "generator" | "objects" | "print") {
            ignored.push(key.clone());
        }
    }
    let list = match top.get("objects") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err("`objects` is a list".into()),
    };
    if list.len() > crate::scene::MAX_OBJECTS {
        return Err(format!("{} objects is more than the {} a plate holds", list.len(), crate::scene::MAX_OBJECTS));
    }
    let mut objects = Vec::with_capacity(list.len());
    for (k, o) in list.iter().enumerate() {
        let Value::Object(o) = o else { return Err(format!("objects[{k}] is not an object")) };
        objects.push(read_object(k, o, &mut ignored).map_err(|e| format!("objects[{k}]: {e}"))?);
    }
    let print = match top.get("print") {
        None | Some(Value::Null) => Value::Null,
        Some(p @ Value::Object(_)) => p.clone(),
        Some(_) => {
            ignored.push("print".into());
            Value::Null
        }
    };
    let scene = Scene::restore(objects)?;
    scene.check()?;
    Ok(Loaded { scene, print, ignored })
}

fn read_object(k: usize, o: &Map<String, Value>, ignored: &mut Vec<String>) -> Result<Object, String> {
    for key in o.keys() {
        if !matches!(key.as_str(), "id" | "name" | "transform" | "vertices" | "triangles") {
            ignored.push(format!("objects[{k}].{key}"));
        }
    }
    let id =
        o.get("id").and_then(Value::as_u64).filter(|&id| id > 0 && id <= u64::from(u32::MAX)).ok_or("`id` is a whole number above 0")?
            as u32;
    let name: String = o.get("name").and_then(Value::as_str).unwrap_or("object").chars().take(200).collect();
    let xf = transform(o.get("transform").ok_or("it has no `transform`")?)?;
    let bytes = |key: &str| -> Result<Vec<u8>, String> {
        base64::decode(o.get(key).and_then(Value::as_str).ok_or_else(|| format!("`{key}` is base64 text"))?)
            .map_err(|e| format!("`{key}`: {e}"))
    };
    let (vb, tb) = (bytes("vertices")?, bytes("triangles")?);
    if !vb.len().is_multiple_of(12) || !tb.len().is_multiple_of(12) {
        return Err("`vertices` and `triangles` hold three 4-byte numbers each".into());
    }
    let word = |b: &[u8]| [b[0], b[1], b[2], b[3]];
    let vertices: Vec<[f32; 3]> = vb.as_chunks::<12>().0.iter().map(|c| [0, 4, 8].map(|i| f32::from_le_bytes(word(&c[i..])))).collect();
    let triangles: Vec<[u32; 3]> = tb.as_chunks::<12>().0.iter().map(|c| [0, 4, 8].map(|i| u32::from_le_bytes(word(&c[i..])))).collect();
    let mesh = Mesh::new(vertices, triangles)?;
    if mesh.triangles.is_empty() {
        return Err("its mesh has no triangles".into());
    }
    Ok(Object { id, name, mesh: Arc::new(mesh), xf })
}

fn transform(v: &Value) -> Result<Transform, String> {
    let nums = |v: Option<&Value>, n: usize| -> Option<Vec<f64>> {
        let a = v?.as_array()?;
        (a.len() == n).then(|| a.iter().map(Value::as_f64).collect::<Option<Vec<f64>>>())?
    };
    let rows = v.get("m").and_then(Value::as_array).filter(|r| r.len() == 3).ok_or("`transform.m` is three rows of three numbers")?;
    let mut m = [[0.0; 3]; 3];
    for (r, row) in rows.iter().enumerate() {
        let row = nums(Some(row), 3).ok_or("`transform.m` is three rows of three numbers")?;
        m[r] = [row[0], row[1], row[2]];
    }
    let t = nums(v.get("t"), 3).ok_or("`transform.t` is three numbers")?;
    let xf = Transform { m, t: [t[0], t[1], t[2]] };
    if !xf.is_finite() {
        return Err("its transform is not finite".into());
    }
    // A transform that flattens the object is not one any command makes (a mirror, `mirror` makes, with a negative determinant).
    if xf.det().abs() <= 1e-12 {
        return Err("its transform flattens the object".into());
    }
    Ok(xf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    fn plate() -> Session {
        let mut s = Session::new();
        s.execute("add_primitive", &json!({"kind": "box", "size": [20, 10, 5], "name": "base"})).unwrap();
        s.execute("add_primitive", &json!({"kind": "sphere", "radius": 4, "at": [30, 0]})).unwrap();
        s.execute("rotate", &json!({"id": 2, "axis": "x", "degrees": 30})).unwrap();
        s
    }

    #[test]
    fn a_plate_goes_out_and_comes_back_the_same() {
        let s = plate();
        let print = json!({"profile": {"layer_height": 0.12}, "printer": "Generic 220 mm, Marlin"});
        let back = read(&write(&s.scene, &print)).unwrap();
        assert!(back.ignored.is_empty(), "{:?}", back.ignored);
        assert_eq!(back.print, print);
        assert_eq!(back.scene.objects.len(), 2);
        for (a, b) in s.scene.objects.iter().zip(&back.scene.objects) {
            assert_eq!((a.id, &a.name, a.xf), (b.id, &b.name, b.xf));
            assert_eq!(*a.mesh, *b.mesh);
        }
        // The next object made after a reload gets a new id, never one in use.
        let mut scene = back.scene;
        assert_eq!(scene.add("third", Mesh::cuboid([1.0, 1.0, 1.0])).unwrap(), 3);
    }

    #[test]
    fn unknown_keys_are_left_out_and_listed_and_a_newer_format_is_refused() {
        let s = plate();
        let mut v: Value = serde_json::from_str(&write(&s.scene, &Value::Null)).unwrap();
        v["colour"] = json!("red");
        v["objects"][1]["material"] = json!("PLA");
        let back = read(&v.to_string()).unwrap();
        assert_eq!(back.ignored, ["colour", "objects[1].material"]);
        v["format"] = json!(FORMAT + 1);
        assert!(read(&v.to_string()).unwrap_err().contains("newer Lamina"));
    }

    #[test]
    fn hostile_projects_are_errors() {
        let s = plate();
        let good: Value = serde_json::from_str(&write(&s.scene, &Value::Null)).unwrap();
        let with = |f: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            f(&mut v);
            v.to_string()
        };
        let bad = [
            "not json".to_string(),
            "[]".into(),
            "{}".into(),
            r#"{"format": 0}"#.into(),
            with(&|v| v["objects"] = json!(5)),
            with(&|v| v["objects"][0]["id"] = json!(0)),
            with(&|v| v["objects"][1]["id"] = json!(1)),
            with(&|v| v["objects"][0]["vertices"] = json!("AAAA")),
            with(&|v| v["objects"][0]["vertices"] = json!("not base64!")),
            with(&|v| v["objects"][0]["triangles"] = json!(base64::encode(&[255u8; 12]))),
            with(&|v| v["objects"][0]["triangles"] = json!("")),
            with(&|v| v["objects"][0]["vertices"] = json!(base64::encode(&f32::NAN.to_le_bytes().repeat(3)))),
            with(&|v| v["objects"][0]["transform"]["t"] = json!([1e300, 0, 0])),
            with(&|v| v["objects"][0]["transform"]["t"] = json!([2e6, 0, 0])),
            with(&|v| v["objects"][0]["transform"]["m"] = json!([[0, 0, 0], [0, 1, 0], [0, 0, 1]])),
            with(&|v| v["objects"][0]["transform"]["m"] = json!([[1e-9, 0, 0], [0, 1e-9, 0], [0, 0, 1]])),
            with(&|v| v["objects"][0]["transform"]["m"] = json!([[1, 0], [0, 1, 0], [0, 0, 1]])),
            with(&|v| v["objects"][0].as_object_mut().unwrap().remove("transform").map(|_| ()).unwrap_or(())),
        ];
        for text in bad {
            assert!(read(&text).is_err(), "{}", &text[..text.len().min(200)]);
        }
    }
}
