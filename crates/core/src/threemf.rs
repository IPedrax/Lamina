//! 3MF, the format other slicers and model sites use, from the 3MF Consortium's public Core Specification (and the path attribute of
//! its Production Extension, which slicers use to keep each object in its own part). A 3MF file is a ZIP package: the relationships
//! part names the model part, an XML file of objects (meshes, or components that place other objects) and build items, each an object
//! with a transform. Each build item becomes one Lamina object. Slicers' own settings in the package are left alone.
//!
//! 3MF writes a transform as four rows of three, applied to a row vector (`p' = p M`, the last row the translation); Lamina's
//! [`Transform`] applies its matrix to a column vector, so each is the other's transpose.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::mesh::{MAX_TRIANGLES, Mesh, Transform};
use crate::zip;

/// The most a part of the package may unpack to, bytes.
pub const MAX_PART: usize = 768 << 20;
/// How deep components may nest.
const MAX_DEPTH: usize = 16;
const MODEL_TYPE: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

/// One build item: a name, its mesh (its components gathered into one) and where the build puts it.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    pub mesh: Mesh,
    pub xf: Transform,
}

#[derive(Default)]
struct Object {
    name: Option<String>,
    kind: String,
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    components: Vec<Placed>,
}

/// An object placed: by a build item or a component. `path` names another model part (the Production Extension).
#[derive(Clone)]
struct Placed {
    object: u32,
    path: Option<String>,
    xf: Transform,
}

struct Model {
    scale: f64,
    objects: HashMap<u32, Object>,
    build: Vec<Placed>,
}

/// The build items of a 3MF file, each as a part.
pub fn read(bytes: &[u8]) -> Result<Vec<Part>, String> {
    let archive = zip::Archive::new(bytes).map_err(|e| format!("not a 3MF file: {e}"))?;
    let root = archive
        .read("_rels/.rels", 1 << 20)
        .ok()
        .and_then(|rels| model_target(&String::from_utf8_lossy(&rels)))
        .unwrap_or_else(|| "3D/3dmodel.model".to_string());
    let mut parts: HashMap<String, Model> = HashMap::new();
    let key = |p: &str| p.trim_start_matches('/').to_ascii_lowercase();
    let load = |path: &str, parts: &mut HashMap<String, Model>| -> Result<(), String> {
        if parts.contains_key(&key(path)) {
            return Ok(());
        }
        let text = archive.read(path, MAX_PART)?;
        let model =
            parse(std::str::from_utf8(&text).map_err(|_| format!("`{path}` is not UTF-8 text"))?).map_err(|e| format!("`{path}`: {e}"))?;
        parts.insert(key(path), model);
        Ok(())
    };
    load(&root, &mut parts)?;
    let build = parts.get(&key(&root)).map(|m| m.build.clone()).unwrap_or_default();
    if build.is_empty() {
        return Err("the 3MF file builds nothing: its model has no build items".into());
    }
    if build.len() > crate::scene::MAX_OBJECTS {
        return Err(format!("{} build items is more than the {} objects a plate holds", build.len(), crate::scene::MAX_OBJECTS));
    }
    // Every part a component names is read once, before the meshes are gathered.
    let mut pending: Vec<String> = vec![root.clone()];
    while let Some(p) = pending.pop() {
        let paths: Vec<String> = parts
            .get(&key(&p))
            .map(|m| m.objects.values().flat_map(|o| o.components.iter().filter_map(|c| c.path.clone())).collect())
            .unwrap_or_default();
        for path in paths {
            if !parts.contains_key(&key(&path)) {
                load(&path, &mut parts)?;
                pending.push(path);
            }
        }
        if parts.len() > 1000 {
            return Err("the 3MF file has more model parts than are read".into());
        }
    }
    let mut out = Vec::new();
    for (n, item) in build.iter().enumerate() {
        let path = item.path.clone().unwrap_or_else(|| root.clone());
        let model = parts.get(&key(&path)).ok_or_else(|| format!("a build item names `{path}`, which the package lacks"))?;
        let object =
            model.objects.get(&item.object).ok_or_else(|| format!("a build item names object {}, which is not there", item.object))?;
        if matches!(object.kind.as_str(), "support" | "solidsupport" | "surface" | "other") {
            continue; // not a solid to print as a part
        }
        let mut gathered = Gathered::default();
        gather(&parts, &key(&path), item.object, &Transform::IDENTITY, 0, &mut gathered)?;
        let scale = model.scale;
        let vertices = gathered.vertices.iter().map(|v| v.map(|c| (c * scale) as f32)).collect();
        let mesh = Mesh::new(vertices, gathered.triangles)?;
        if mesh.triangles.is_empty() {
            continue;
        }
        let xf = Transform { t: item.xf.t.map(|c| c * scale), ..item.xf };
        let name = object.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("part {}", n + 1));
        out.push(Part { name: name.chars().take(200).collect(), mesh, xf });
    }
    if out.is_empty() {
        return Err("the 3MF file holds no solid to print".into());
    }
    Ok(out)
}

#[derive(Default)]
struct Gathered {
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
}

/// Object `id` of the part `path`, with its components, moved by `xf`, added to `into`.
fn gather(parts: &HashMap<String, Model>, path: &str, id: u32, xf: &Transform, depth: usize, into: &mut Gathered) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("components nest more than {MAX_DEPTH} deep, or name each other in a ring"));
    }
    let model = parts.get(path).ok_or_else(|| format!("a component names `{path}`, which the package lacks"))?;
    let o = model.objects.get(&id).ok_or_else(|| format!("a component names object {id}, which is not there"))?;
    if into.triangles.len() + o.triangles.len() > MAX_TRIANGLES {
        return Err(format!("an object has more than the {MAX_TRIANGLES} triangles a mesh may hold"));
    }
    let base = into.vertices.len() as u32;
    // A mirroring transform turns the faces inside out: wind them the other way to keep them facing out.
    let mirror = xf.det() < 0.0;
    into.vertices.extend(o.vertices.iter().map(|&v| xf.apply(v)));
    into.triangles.extend(o.triangles.iter().map(|t| {
        let t = t.map(|i| i + base);
        if mirror { [t[0], t[2], t[1]] } else { t }
    }));
    for c in &o.components {
        let sub = c.path.as_deref().map(|p| p.trim_start_matches('/').to_ascii_lowercase()).unwrap_or_else(|| path.to_string());
        gather(parts, &sub, c.object, &c.xf.then(xf), depth + 1, into)?;
    }
    Ok(())
}

/// The target of the relationship of the 3D model type, from the package's `_rels/.rels`.
fn model_target(rels: &str) -> Option<String> {
    let mut reader = Reader::from_str(rels);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                let a = attributes(&e).ok()?;
                if a.get("Type").is_some_and(|t| t == MODEL_TYPE) {
                    return a.get("Target").cloned();
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// An element's attributes by local name (a prefix such as `p:` is dropped).
fn attributes(e: &BytesStart) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    for a in e.attributes() {
        let a = a.map_err(|e| format!("a bad attribute: {e}"))?;
        let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
        let value = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|e| format!("a bad attribute value: {e}"))?.into_owned();
        out.insert(key, value);
    }
    Ok(out)
}

fn number(a: &HashMap<String, String>, key: &str) -> Result<f64, String> {
    let v: f64 =
        a.get(key).ok_or_else(|| format!("a `{key}` is missing"))?.trim().parse().map_err(|_| format!("`{key}` is not a number"))?;
    if v.is_finite() { Ok(v) } else { Err(format!("`{key}` is not finite")) }
}

fn index(a: &HashMap<String, String>, key: &str) -> Result<u32, String> {
    a.get(key).ok_or_else(|| format!("a `{key}` is missing"))?.trim().parse().map_err(|_| format!("`{key}` is not a whole number"))
}

/// A 3MF transform attribute: twelve numbers, four rows of three.
fn transform(text: &str) -> Result<Transform, String> {
    let a: Vec<f64> = text.split_whitespace().map(str::parse).collect::<Result<_, _>>().map_err(|_| "a transform is not numbers")?;
    if a.len() != 12 || a.iter().any(|c| !c.is_finite()) {
        return Err("a transform is not twelve finite numbers".into());
    }
    Ok(Transform { m: [[a[0], a[3], a[6]], [a[1], a[4], a[7]], [a[2], a[5], a[8]]], t: [a[9], a[10], a[11]] })
}

/// The transform as a 3MF attribute.
fn transform_text(xf: &Transform) -> String {
    let m = &xf.m;
    [m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2], xf.t[0], xf.t[1], xf.t[2]]
        .iter()
        .map(|c| format!("{c}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse(text: &str) -> Result<Model, String> {
    let mut reader = Reader::from_str(text);
    let mut model = Model { scale: 1.0, objects: HashMap::new(), build: Vec::new() };
    let mut current: Option<(u32, Object)> = None;
    let mut total = 0usize;
    loop {
        let event = reader.read_event().map_err(|e| format!("bad XML at byte {}: {e}", reader.buffer_position()))?;
        let (e, empty) = match &event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) if e.local_name().as_ref() == b"object" => {
                if let Some((id, o)) = current.take() {
                    model.objects.insert(id, o);
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let a = attributes(e)?;
        match e.local_name().as_ref() {
            b"model" => {
                model.scale = match a.get("unit").map(String::as_str).unwrap_or("millimeter") {
                    "micron" => 0.001,
                    "millimeter" => 1.0,
                    "centimeter" => 10.0,
                    "inch" => 25.4,
                    "foot" => 304.8,
                    "meter" => 1000.0,
                    other => return Err(format!("unknown unit `{other}`")),
                };
            }
            b"object" => {
                let id = index(&a, "id")?;
                let o = Object {
                    name: a.get("name").cloned(),
                    kind: a.get("type").cloned().unwrap_or_else(|| "model".into()),
                    ..Object::default()
                };
                if model.objects.len() >= 100_000 {
                    return Err("too many objects".into());
                }
                if empty {
                    model.objects.insert(id, o);
                } else {
                    current = Some((id, o));
                }
            }
            b"vertex" => {
                let (_, o) = current.as_mut().ok_or("a vertex outside an object")?;
                total += 1;
                if total > 3 * MAX_TRIANGLES {
                    return Err("more vertices than a mesh may hold".into());
                }
                o.vertices.push([number(&a, "x")?, number(&a, "y")?, number(&a, "z")?]);
            }
            b"triangle" => {
                let (_, o) = current.as_mut().ok_or("a triangle outside an object")?;
                let t = [index(&a, "v1")?, index(&a, "v2")?, index(&a, "v3")?];
                if t.iter().any(|&i| i as usize >= o.vertices.len()) {
                    return Err("a triangle names a vertex the object does not have".into());
                }
                if o.triangles.len() >= MAX_TRIANGLES {
                    return Err(format!("an object has more than the {MAX_TRIANGLES} triangles a mesh may hold"));
                }
                if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                    o.triangles.push(t);
                }
            }
            b"component" => {
                let (_, o) = current.as_mut().ok_or("a component outside an object")?;
                let xf = a.get("transform").map(|t| transform(t)).transpose()?.unwrap_or(Transform::IDENTITY);
                o.components.push(Placed { object: index(&a, "objectid")?, path: a.get("path").cloned(), xf });
            }
            b"item" => {
                let xf = a.get("transform").map(|t| transform(t)).transpose()?.unwrap_or(Transform::IDENTITY);
                model.build.push(Placed { object: index(&a, "objectid")?, path: a.get("path").cloned(), xf });
            }
            _ => {}
        }
    }
    Ok(model)
}

/// A 3MF package of `parts`, each its own object and build item.
pub fn write(parts: &[Part]) -> Vec<u8> {
    use std::fmt::Write;
    let esc = |t: &str| quick_xml::escape::escape(t).into_owned();
    let mut model = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE}\">\n \
         <metadata name=\"Application\">Lamina {}</metadata>\n <resources>\n",
        env!("CARGO_PKG_VERSION")
    );
    for (k, p) in parts.iter().enumerate() {
        let _ = write!(model, "  <object id=\"{}\" name=\"{}\" type=\"model\">\n   <mesh>\n    <vertices>\n", k + 1, esc(&p.name));
        for v in &p.mesh.vertices {
            let _ = writeln!(model, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", v[0], v[1], v[2]);
        }
        model.push_str("    </vertices>\n    <triangles>\n");
        for t in &p.mesh.triangles {
            let _ = writeln!(model, "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2]);
        }
        model.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    model.push_str(" </resources>\n <build>\n");
    for (k, p) in parts.iter().enumerate() {
        let _ = writeln!(model, "  <item objectid=\"{}\" transform=\"{}\"/>", k + 1, transform_text(&p.xf));
    }
    model.push_str(" </build>\n</model>\n");
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
                 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
                 <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>\n";
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"{MODEL_TYPE}\"/></Relationships>\n"
    );
    zip::write(&[("[Content_Types].xml", types.as_bytes()), ("_rels/.rels", rels.as_bytes()), ("3D/3dmodel.model", model.as_bytes())])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(model: &str) -> Vec<u8> {
        zip::write(&[("3D/3dmodel.model", model.as_bytes())])
    }

    #[test]
    fn parts_go_out_and_come_back_with_their_transforms() {
        let a = Part { name: "base & lid".into(), mesh: Mesh::cuboid([10.0, 20.0, 5.0]), xf: Transform::translate([110.0, 100.0, 0.0]) };
        let turned = Transform::rotate(2, 30.0).then(&Transform::translate([80.0, 120.0, 0.0]));
        let b = Part { name: "peg".into(), mesh: Mesh::cylinder(3.0, 8.0, 24), xf: turned };
        let back = read(&write(&[a.clone(), b.clone()])).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!((back[0].name.as_str(), &back[0].mesh, back[0].xf), ("base & lid", &a.mesh, a.xf));
        assert_eq!(back[1].mesh, b.mesh);
        for (p, q) in back[1].xf.m.iter().flatten().zip(b.xf.m.iter().flatten()) {
            assert!((p - q).abs() < 1e-12);
        }
        // The transform is applied to a row vector in the file: the point (1, 0, 0) of the peg lands where Lamina says.
        let p = back[1].xf.apply([1.0, 0.0, 0.0]);
        let q = b.xf.apply([1.0, 0.0, 0.0]);
        assert!((p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9);
    }

    #[test]
    fn components_units_and_other_parts_are_gathered_into_one_object() {
        // An object made of a box placed twice, in centimetres, the second copy in another model part (as slicers write them).
        let main = r#"<?xml version="1.0"?>
<model unit="centimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
 <resources>
  <object id="1" type="model"><mesh><vertices>
   <vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/>
  </vertices><triangles>
   <triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/>
  </triangles></mesh></object>
  <object id="2" name="pair" type="model"><components>
   <component objectid="1"/>
   <component objectid="9" p:path="/3D/Objects/other.model" transform="1 0 0 0 1 0 0 0 1 5 0 0"/>
  </components></object>
 </resources>
 <build><item objectid="2" transform="1 0 0 0 1 0 0 0 1 10 10 0"/></build>
</model>"#;
        let other = main.replace("id=\"1\" type", "id=\"9\" type").replace("<build>", "<!--").replace("</build>", "-->");
        let rels = format!("<Relationships><Relationship Target=\"/3D/3dmodel.model\" Id=\"r\" Type=\"{MODEL_TYPE}\"/></Relationships>");
        let bytes = zip::write(&[
            ("_rels/.rels", rels.as_bytes()),
            ("3D/3dmodel.model", main.as_bytes()),
            ("3D/Objects/other.model", other.as_bytes()),
        ]);
        let parts = read(&bytes).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "pair");
        assert_eq!(parts[0].mesh.triangles.len(), 8);
        let (lo, hi) = parts[0].mesh.bounds().unwrap();
        assert_eq!((lo, hi), ([0.0, 0.0, 0.0], [60.0, 10.0, 10.0]), "centimetres become millimetres");
        assert_eq!(parts[0].xf.t, [100.0, 100.0, 0.0]);
        assert!(parts[0].mesh.is_watertight() && parts[0].mesh.volume() > 0.0);
    }

    #[test]
    fn hostile_packages_are_errors() {
        let tet = |extra: &str| {
            format!(
                r#"<model><resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/>
                <vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/>{extra}</triangles></mesh></object>
                </resources><build><item objectid="1"/></build></model>"#
            )
        };
        assert!(read(&package(&tet(""))).is_ok());
        for bad in [
            tet(r#"<triangle v1="0" v2="1" v3="7"/>"#),
            tet(r#"<triangle v1="0" v2="1" v3="-1"/>"#),
            tet("").replace("x=\"1\"", "x=\"NaN\""),
            tet("").replace("x=\"1\"", "x=\"1e300\""),
            tet("").replace("objectid=\"1\"", "objectid=\"2\""),
            tet("").replace("<item objectid=\"1\"/>", ""),
            tet("").replace("<model>", "<model unit=\"parsec\">"),
            tet("").replace("<item objectid=\"1\"/>", "<item objectid=\"1\" transform=\"1 0 0\"/>"),
            tet("").replace("</vertices>", "</vertices"),
            r#"<model><resources><object id="1"><components><component objectid="1"/></components></object></resources>
               <build><item objectid="1"/></build></model>"#
                .to_string(),
        ] {
            assert!(read(&package(&bad)).is_err(), "{bad}");
        }
        assert!(read(b"not a zip").is_err());
        assert!(read(&zip::write(&[("other.txt", b"x")])).is_err());
    }
}
