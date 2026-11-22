# Research: slicers, G-code, editors and Rust crates

A web survey made on 2026-11-22 to decide what to build first. Crate dates are the crates.io last-publish date. "Unverified" means
the source could not confirm it. This is a starting point, not a legal opinion: check a licence before depending on or copying anything.

**Headline.** Build a CLI-first slicer core on `i_overlay`, add Manifold for mesh booleans, and let the AI drive a scripted CSG
modeller. Skip B-rep and sculpting at first. Almost every major slicer is AGPL, so the code has to be clean-room.

## Slicers

| Slicer | Best at | Licence |
|---|---|---|
| PrusaSlicer | polished all-rounder, Arachne variable-width walls, has a CLI | AGPL-3.0 |
| OrcaSlicer | calibration suite, wide printer support, sends to Klipper, PrusaLink and OctoPrint | AGPL-3.0 (Bambu network plugin is non-free) |
| Bambu Studio | Bambu printers (a PrusaSlicer fork) | AGPL-3.0 |
| SuperSlicer | calibration tests, extra surface and seam options (a PrusaSlicer fork) | AGPL-3.0 |
| UltiMaker Cura | plugins, tree supports | Cura LGPL-3.0, CuraEngine AGPL-3.0 |
| Slic3r | the ancestor of the family | AGPL-3.0 |
| Simplify3D | per-region process settings, support editing | proprietary, subscription |
| ideaMaker, KISSlicer | free tiers, tuned for specific printers | proprietary |
| Kiri:Moto | browser slicer, also CAM and laser | MIT |
| Lychee, ChiTuBox | resin slicing | proprietary (licence text unverified) |

## Slicer features, by when Lamina needs them

- **MVP:** STL and 3MF import with light repair, slicing to layers, constant-width walls with simple seams, rectilinear and grid infill (gyroid
  next), solid top and bottom layers, skirt and brim, retraction and cooling, time and filament estimates, a G-code preview with a layer
  slider, a profile system (printer, filament, process) with inheritance and start/end G-code placeholders.
- **v1:** normal then tree supports, raft, bridging, adaptive layer height, per-object settings and height-range modifiers, pressure advance
  emitted per firmware, arc fitting (G2/G3), variable-width walls (public 2020 paper), calibration prints, ironing, importing Orca and Prusa profiles.
- **Later:** fuzzy skin, multi-material and purge towers, painted supports and seams, modifier meshes, binary G-code (Prusa's `.bgcode` library is AGPL, so skip it).

Profiles: Orca uses JSON presets with an `inherits` chain. Prusa-family 3MF projects embed settings in `Metadata/Slic3r_PE.config`.
Design our own schema and import theirs at runtime. Whether bundling their profile files carries AGPL terms is unverified.

## G-code and connectivity

Marlin is the baseline. Klipper takes standard G-code plus named macros called from start G-code. Pressure advance is `M900` on Marlin,
`M572` on RepRapFirmware, and a macro or `SET_PRESSURE_ADVANCE` on Klipper. PrusaSlicer's placeholders are `[name]` for substitution and
`{expr}` for a small macro language. Lamina supports `[name]` today and refuses unknown names.

| Target | Mechanism | Effort |
|---|---|---|
| Moonraker (Klipper) | `POST /server/files/upload`, multipart, `print=true` | easy, do first |
| OctoPrint | `POST /api/files/local`, multipart, `print=true`, key in `X-Api-Key` | easy, do second |
| PrusaLink | `PUT /api/v1/files/usb/<name>`, digest auth preferred | medium |
| Duet | RRF `rr_upload` or DSF `PUT /machine/file/...` (sources disagree on which works where) | medium |
| Bambu LAN | MQTT over TLS 8883, implicit FTPS 990, upload 3MF then send `project_file`; 2025 authorization changes restrict this | hard, sources conflict |
| Serial | needs streaming, resend and buffer handling | hard |

## Modelling editors

| Tool | Pattern worth taking | Tier |
|---|---|---|
| Blender | orbit/pan/zoom, transform gizmo, snapping, modifier stack | navigation, gizmo, snap: MVP. Modifiers: v1. Sculpt: later |
| Tinkercad | primitives, "hole" shapes that subtract on group, workplane, align, ruler | the MVP interaction model |
| OpenSCAD | CSG as code | the MVP idea for scripts |
| Fusion 360 | sketch, extrude, parametric history | v1 |
| FreeCAD, Shapr3D, Plasticity | parametric or NURBS B-rep on OpenCascade or Parasolid | later |
| Meshmixer | repair, hollow, split, cut (development stopped) | repair: MVP. Hollow, split, cut: v1 |

**Best AI fit: parametric CSG scripts.** Text is easier for a model to write than a binary CAD file, and parameters make edits cheap.
Mesh editing fits poorly because there is nothing for the AI to change. CSG cannot make organic shapes, so keep mesh import. A rival
with this thesis exists (SlicerX, pre-alpha, README claims only).

## Rust crates

| Stage | Pick | Licence | Notes |
|---|---|---|---|
| Polygon clip and offset | **i_overlay** 9.2.0 (in use) | MIT or Apache-2.0 | pure Rust; booleans, outline offset, line clipping |
| | clipper2 0.6.1 | MIT or Apache-2.0 | C++ underneath |
| | clipper2-rust, boostvoronoi | **BSL-1.0** | permissive but its own text; boostvoronoi likely needed for variable-width walls |
| Triangulation | spade 2.15, earcut 0.4 | MIT or Apache-2.0 | for caps and previews |
| Mesh booleans | **manifold3d** 0.4.2 | MIT or Apache-2.0 | robust, but builds C++ with cmake; not tried in Lamina |
| | **csgrs** 0.20.1 (in use) | MIT | pure Rust BSP. Measured in Lamina: right on boxes and upright cylinders, unreliable on curved-on-curved (see ARCHITECTURE.md). Needs a stand-in for the yanked `core2` |
| Math, BVH | glam, nalgebra, bvh, parry3d | permissive | not needed yet |
| Formats | stl_io, tobj (OBJ), threemf 0.8 (0BSD), lib3mf 0.1.7 | permissive | no maintained AMF crate found |
| G-code | gcode 0.7 | MIT or Apache-2.0 | parser only; writing is string formatting |
| STEP and B-rep | truck 0.6, ruststep 0.4 | Apache-2.0 | last published 2024; later |
| Avoid | opencascade (LGPL-2.1), mesh-repair (yanked) | | |

Licence flags among existing Rust slicers: mslicer (GPL-3.0), GladiusSlicer (LGPL-2.1), fable-slicer (AGPL-3.0). Spaghetti Slicer
(Apache-2.0 or MIT) is the one to read for structure; its README calls it an early prototype never run on a printer.

## Existing 3D skills for Claude (surveyed 2026-11-22)

None is about slicing, and almost none is Rust, so they inform Lamina's own skills and are never dependencies.

| Skill | What it is | Licence |
|---|---|---|
| OmniSkill `img2threejs`, `vgpu`, `anatomy`, `motion-ui` | routers to Three.js, WebGPU and Python toolkits for the web | MIT adapters over Apache-2.0 or MIT upstreams, not bundled |
| `andreahaku/openscad_claude_skill` | OpenSCAD design, modify, reconstruct, export modes with FDM rules | none found on GitHub, so do not copy |
| `flowful-ai/cad-skill` | CadQuery parametric printable parts with a render and fix loop | NOASSERTION |
| `earthtojake/text-to-cad` | STEP, STL and 3MF generation, a design-for-additive check, G-code slicing | MIT |
| `ahujasid/blender-mcp` | Blender driven over MCP, with Python execution in Blender | MIT |
| ECC `blender-motion-state-inspection` | inspecting Blender rigs and poses | ECC plugin |

Takeaways that carry over: infer and list assumptions rather than interviewing; run arithmetic checks before a visual one; say when
a clearance is a default and not a measurement; modify an existing mesh instead of rebuilding it. Not yet possible in Lamina: a
render the model can look at (see the roadmap).

## Risks

1. **Real meshes.** Turning messy meshes (non-manifold, self-intersecting, tiny features) into valid layer polygons, then offsetting them
   robustly. Every later feature stands on it, and a failure shows up as silently bad G-code. Lamina warns on an open mesh today.
2. Bambu's authorization lockout for third-party control.
3. An AI assistant reproducing AGPL code. Keep the clean-room rule: implement from papers and documentation, not from slicer source.

## Unverified or conflicting

Lychee and ChiTuBox licences; Bambu Developer Mode behaviour; PrusaLink's exact print-after-upload parameter; Cura's current engine
transport; Orca and Cura placeholder syntax; whether the listed crates are pure Rust (taken from descriptions); truck's real maintenance level.

## Sources

Project pages and licence APIs on GitHub for PrusaSlicer, OrcaSlicer, BambuStudio, Cura, CuraEngine, SuperSlicer, Slic3r and
grid-apps (Kiri:Moto); the Klipper G-code reference; Moonraker and OctoPrint file API docs; PrusaSlicer's macro help; the Orca
profile guide; Zoo's KCL write-up and the Text-to-CadQuery paper (arXiv 2505.06507) on LLM-driven CAD; iOverlay, manifold-csg and
baby_shark on GitHub; crates.io's API for versions and licences; the Spaghetti Slicer, SlicerX, mslicer, GladiusSlicer and
fable-slicer repositories.
