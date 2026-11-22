---
name: design-for-fdm
description: Rules of thumb for shapes that print well on a filament (FDM) printer: walls, overhangs, holes, fits, orientation. Read before choosing dimensions for anything the user will print.
---
# Designing for FDM

These are rules of thumb for a 0.4 mm nozzle, 0.2 mm layers and PLA or PETG. They are not measurements of the user's printer. When
a fit matters, say that the clearances are defaults and that the first print is the real test.

## Walls and small features

- One printed line is about 0.45 mm wide (1.125 times the nozzle). Lamina has no gap fill yet, so a wall thinner than one line is
  not printed at all, and with the default two walls anything under about 1.35 mm gets only one.
- Aim for walls of at least 1.4 mm where they carry load and 0.9 mm for light covers. Make floors and roofs at least 0.8 mm,
  which is four layers at 0.2 mm.
- A pin or post thinner than 2 mm snaps easily. Thicken it or give it a wider base.

## Overhangs and bridges

- A surface that leans more than about 45 degrees from vertical needs support to print cleanly. Lamina has no supports yet, so
  design without them: keep undersides at 45 degrees or steeper.
- A sphere touches the plate at a point and overhangs badly below its equator. Tell the user a sphere will print poorly without
  support, and suggest another shape when the use allows.
- A flat span over air of up to about 5 mm usually works. At 10 mm it sags, and longer needs support. Lamina has no bridging logic.

## Orientation

- Put the largest flat face on the plate. It sticks best and has the cleanest surface.
- Layers are weakest where they join. Orient the part so loads pull along the layers, not across them. `rotate {id, axis, degrees}`
  sets the part back on the plate afterwards.
- A tall, thin part wobbles as it grows. Widen its base or lay it down.

## Holes and fits

- Printed holes come out a little small. Make a screw hole about 0.2 to 0.4 mm wider than the screw.
- For two parts that must fit, leave about 0.2 mm per side for a snug fit and 0.3 to 0.4 mm per side for a sliding fit.
- Cut holes with `difference`, using a cutting shape that sticks out past the faces it passes through.

## Size limits

- The part must fit the plate (220 x 220 mm and 250 mm tall by default). `slicer_profile` shows the printer's size and `slice_check`
  refuses what does not fit, and says by how much.
- Layer height is at most 80 percent of the nozzle. Thinner is slower and smoother.
