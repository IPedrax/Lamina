---
name: materials
description: Common filaments (PLA, PETG, ABS, ASA, TPU, nylon, PC): what each suits, how to print it with the presets, and its limits. Read when the user chooses a material.
---
# Materials

Recommend by what the part must do: carry load, bend, stay stiff when warm, sit in the sun, touch food. Then name the preset:
`slice_check {filament: "PETG"}`. The filament's own label wins over any range here. Never promise a part is safe for food, for a
child or for a load that could hurt someone: say what to check.

## The built-in presets

`slicer_presets` lists them; each sets the temperatures and the fan.

- **PLA**: nozzle 210, bed 60, fan full. The easy one: stiff, crisp detail, little warping. Softens at about 55 C (a hot car,
  a dishwasher), brittle when it fails, not for outdoor parts in the sun. Good for models, prototypes, jigs, enclosures that stay cool.
- **PETG**: nozzle 240, bed 80, fan half. Tougher and more heat tolerant (about 75 C) than PLA, bends before it breaks, fine
  outdoors. Strings more; sticks hard to a smooth PEI plate (use glue or a textured plate). Good for functional parts, brackets,
  clips, outdoor fittings.
- **ABS**: nozzle 250, bed 100, fan off. Heat tolerant (about 95 C), tough, sands and glues well. Warps and cracks without an
  enclosure, and gives off fumes: needs ventilation. Use a brim (`brim_width` 5 to 8).
- **ASA**: nozzle 255, bed 100, fan off. Like ABS but stands up to sunlight: the choice for outdoor parts that must stay stiff when
  warm. Enclosure, ventilation and a brim, as ABS.
- **TPU 95A**: nozzle 225, bed 50, fan half. Flexible and rubbery: gaskets, bumpers, phone cases, feet. Print slowly (`perimeter_speed`
  and `infill_speed` 20 to 25), little or no retraction (`retract_length` 0.5 to 1 on direct drive), and a Bowden extruder may
  not push it at all.

## Without a preset

Set the fields yourself, with the user's label as the authority.

- **Nylon (PA)**: nozzle 250 to 270, bed 70 to 90, fan low. Very tough and wear resistant, for gears and hinges. Takes up water
  from the air within hours: dry it before printing. Warps; a brim and an enclosure help.
- **Polycarbonate (PC)**: nozzle 260 to 300, bed 100 to 120. Strong and heat tolerant past 110 C, but needs a hot nozzle, an
  enclosure and a heated chamber to avoid cracking. Check the printer reaches the temperatures (an all-metal hot end).
- **Filled filaments** (carbon fibre, glow, wood, metal): wear a brass nozzle quickly; suggest a hardened one. Settings as the base
  material.

Example: `slice_check {profile: {nozzle_temp: 260, first_layer_nozzle_temp: 265, bed_temp: 80, first_layer_bed_temp: 80, fan_speed: 40}}`.

## Strength and the print

The way a part is printed matters as much as the material. Layers are weakest where they join: turn the part so the load runs
along the layers, not across them. More walls (`perimeters` 3 to 4) add more strength than more infill; "Strong" (`process:
"Strong"`) sets four walls and 40 % gyroid infill.

## Food, skin and heat

FDM parts have fine grooves that hold bacteria, and pigments and the nozzle's metal are not tested for food. Say a part is not
food safe unless the user uses a filament certified for it and seals the surface, and even then only for brief contact. Parts near
a heat source should be ABS, ASA, PC or nylon, never PLA.

## Storage

Most filaments take up water from the air: PETG, TPU and nylon most. Wet filament pops, strings and prints weak. Suggest drying
(the label says how) and storing with desiccant.
