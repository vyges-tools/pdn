# vyges-pdn

Power distribution network generation over an OpenDB database.

A design needs power everywhere, delivered as a mesh of wide metal laid over the whole die and
stitched together with vias. This engine builds that mesh: the ring around the core, the strap sets
that cross it, and the rails that run along every standard-cell row.

## Status

| | |
| --- | --- |
| Rings | ✅ implemented |
| Straps, track snapping, derived defaults | ✅ implemented |
| Follow pins | ✅ implemented |
| Grid build order, deferral, refinement | ✅ implemented |
| Shape merge and cutting | ✅ implemented |
| Vias, trimming, channel repair | ✅ implemented |
| Pad connections, including over the pads | ✅ implemented |
| Region domains and power switches | ✅ implemented |
| `-existing` grids, `repair_pdn_vias`, `add_sroute_connect` | ⛔ not yet |

Every rule is measured against a real design rather than asserted — every shape and every via
checked, not sampled, on designs from a few dozen shapes to a few thousand.

## Design

The library reads no database: the geometry is pure functions over values, and the binary reads the
design and hands those values in. That split is what lets every rule be tested without a design,
and it is why the test count is high relative to the code.

```text
src/nets.rs        which nets a component builds for, and in what order
src/rings.rs       the loop of wide metal around the core, one ring per net
src/straps.rs      the repeating stripes, track snapping, derived defaults
src/followpins.rs  the rails along every standard-cell row
src/grid.rs        build order, the one retry, the refinement loop, strap boundary
src/shapes.rs      merging shapes and cutting them around obstructions
src/vias.rs        where a via belongs, and which candidates are thrown away
src/viagen.rs      what via is built there: cut arrays, enclosures, array spacing
src/pads.rs        pad connections, over the pad and from its edge
src/trim.rs        what holds a shape up, and what is left when nothing does
src/channels.rs    finding unconnected channels and repairing them
```

## Attribution

OpenROAD's `pdn` module was the **reference for this engine's behaviour**. The algorithms here are
reimplemented from its published behaviour — the same published algorithm and the same parameter
semantics — and are not a transliteration of its source; no code is copied. Written against OpenDB
at pin `b5624809f29048e1f9ce9e83eb562620c652e084`.

Agreeing with `pdn` is not a goal this project is bound to. Where its own requirements point
elsewhere — advanced nodes among them — this engine follows those instead, and the two will
diverge.

The design database is read through `vyges-opendb`, which binds OpenROAD's OpenDB (`libodb`). See
`NOTICE`.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
