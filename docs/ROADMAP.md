# Roadmap

Ordered by what unblocks the next thing. Each step ends with a test that looks at output (volumes, layer counts, G-code lines).

## Done

- Command engine with rollback, undo, scripts, a catalog agents read, and read-only access.
- Meshes, primitives (box, cylinder, sphere), transforms, STL in and out.
- Slicer: layers, walls, solid and sparse infill, skirt, retraction, flow, time and filament estimates, Marlin-style G-code.
- CLI and MCP server. AI chat engine ported from BIMCraft (providers, sealed key, attachments, tool layer).
- The desktop app: a 3D view with orbit and picking, the objects' tools, print settings, slicing with a layer preview, G-code
  export, and the assistant panel.
- Printer and filament presets (built in, by name from any front end, and the user's own in the app), and a settings file that
  keeps the print settings, the presets and, when asked, the assistant's key, sealed.
- Booleans: `union`, `difference` and `intersect`, with crack repair and a gate that refuses broken results.
- The AI's skills (`read_skill`: modelling, design for FDM, slicer settings, working from a picture) and `slice_check`, a slice that
  writes no file, so the model can verify a part while `slice` stays refused to it.

## Next

0. **Tilted cylinders in booleans.** Upright shapes and spheres now give a clean solid in 199 of 200 random cases; tilted shapes in 180 of
   200, failing mostly on tilted cylinders crossing each other or a box. A tilted cap could be merged if cylinders were built with an
   exactly flat cap in their own frame and moved afterwards in f64; or run booleans in a child process so no input can crash the app.
   Re-measure with `success_matrix` and `curved_score`.

1. **Real prints.** Run G-code on two printers (one Marlin, one Klipper) before trusting any number. Nothing here has touched hardware.
2. **Mesh repair and import.** Weld, fix winding, report non-manifold edges; 3MF and OBJ readers (lift `mesh_in.rs` from BIMCraft).
3. **Slicer quality.** Connect infill lines instead of retracting per line, seam placement, gyroid and grid infill, brim, cooling per layer time.
4. **Profiles.** Printer, filament and process presets with inheritance; Prusa-style `{expr}` placeholders; import Orca and Prusa profiles.
5. **Send to printer.** Moonraker, then OctoPrint (one multipart POST each), as a separate crate that only the desktop and CLI use.
6. **Desktop, next round.** A move/rotate/scale gizmo in the 3D view; the GPU viewport for big meshes; a 3D G-code preview with
   colours by feature; saving and opening a project file; process presets (fine, standard, draft) and importing Orca or Prusa profiles.
7. **A render the model can see.** A `preview` command that draws the scene to a PNG and returns it to the model, so it can compare
   with a picture the way the OpenSCAD and CadQuery skills do. Anthropic takes images in tool results; the other providers need a
   follow-up message.
8. **AI modelling loop.** A text part format the assistant edits (parametric CSG), then slice and read back checks (watertight, bounds, time).
   Needs the boolean decision (Manifold or `csgrs`). Make a chat turn one undo step.

## Later

Supports (normal, then tree), bridging, raft, adaptive layers, arc fitting, pressure advance per firmware, variable-width walls,
per-object settings, sketch and extrude, fillets and chamfers, text and emboss, multi-material, PrusaLink, Duet and Bambu, STEP.

## Known limits today

- Walls are constant width, so thin features vanish instead of getting gap fill. No supports, so overhangs print in the air. No bridging.
- Infill lines are separate strokes. The skirt is an outline of the first layer, not a hull. Time is distance over speed, without acceleration.
- A mesh that is not closed prints with parts missing, with a warning. STL only.
