// SPDX-License-Identifier: Apache-2.0
//! Follow pins — the power rails that run along every standard-cell row.
//!
//! Unlike a strap set, these are not laid on a pitch of their own: there is one power rail and one
//! ground rail per row, sitting on the row's own edges. The rows decide where they go.
//!
//! Nothing here touches a database.

use crate::Rect;

/// One standard-cell row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub bbox: Rect,
    /// The site's name — a power switch applies to a row only where the two agree.
    pub site: String,
    /// ⚠️ Only `R0` puts power on top. Every other orientation — including `MY`, which is also
    /// unflipped vertically — puts ground there, because the test is an equality against `R0` and
    /// not a question about vertical mirroring.
    pub orient: String,
}

/// One rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rail {
    pub layer: String,
    pub net: String,
    pub rect: Rect,
}

/// **F1** — the width of a follow pin, taken from the standard cells themselves.
///
/// The narrowest supply pin box on a routing layer across every **core** master. Returns `None`
/// where no such box exists, which the reference treats as a hard error rather than a default.
///
/// ⚠️ **`getDY()`, the box's HEIGHT, whatever the layer's direction.** Follow pins run along a row
/// and a row is horizontal, so the rail's thickness is a y measurement — but the rule is written
/// against the box, not against the layer, so a supply pin on a vertical layer still contributes
/// its height.
///
/// ⚠️ **Minimum across every master, not per layer and not per master.** One unusually thin cell
/// sets the width for the whole design.
pub fn determine_width(boxes: &[(bool, bool, bool, i32)]) -> Option<i32> {
    // `(is_core, is_supply, is_routing, height)`
    boxes
        .iter()
        .filter(|(core, supply, routing, _)| *core && *supply && *routing)
        .map(|(_, _, _, dy)| *dy)
        .min()
}

/// **F2** — the rails for one set of rows.
///
/// Power comes first for each row, then ground. ⚠️ **Not the grid's net order** — follow pins ask
/// the domain for its power and ground nets directly, so the ground-first ordering that governs
/// rings and straps does not apply here. An implementation that routed this through the same net
/// list would put the rails on the wrong nets for every row.
///
/// `boundary` is what a row is stretched to. ⚠️ **A row is extended only when its edge sits exactly
/// on the core's**, tested by equality. A row starting one unit inside the core keeps its own edge,
/// so a design whose rows are inset by any amount gets no extension at all — this is not a
/// tolerance, it is an identity.
pub fn make(
    layer: &str,
    power: &str,
    ground: &str,
    width: i32,
    rows: &[Row],
    core: Rect,
    boundary: Rect,
) -> Vec<Rail> {
    let mut out = Vec::with_capacity(rows.len() * 2);
    for row in rows {
        let (bx0, by0, bx1, by1) = row.bbox;
        let x0 = if bx0 == core.0 { boundary.0 } else { bx0 };
        let x1 = if bx1 == core.2 { boundary.2 } else { bx1 };

        let power_on_top = row.orient == "R0";
        // ⚠️ `- width / 2` then `+ width`, the same asymmetry the straps have: for an odd width the
        // extra unit falls on the high side of the row edge.
        let power_y = (if power_on_top { by1 } else { by0 }) - width / 2;
        let ground_y = (if power_on_top { by0 } else { by1 }) - width / 2;

        out.push(Rail {
            layer: layer.into(),
            net: power.into(),
            rect: (x0, power_y, x1, power_y + width),
        });
        out.push(Rail {
            layer: layer.into(),
            net: ground.into(),
            rect: (x0, ground_y, x1, ground_y + width),
        });
    }
    out
}

/// **F3** — what a follow pin is stretched to, which is not what a strap is stretched to.
///
/// ⚠️ **`Fixed` uses the core**, not the fixed coordinates a strap set would use. The two extend
/// modes share a name and a value and mean different things in the two components, which is only
/// visible by reading both.
pub fn boundary(mode: crate::straps::Extend, core: Rect, rings: Rect, grid: Rect) -> Rect {
    use crate::straps::Extend::*;
    match mode {
        Core | Fixed => core,
        Rings => rings,
        Boundary => grid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(x0: i32, y0: i32, x1: i32, y1: i32, orient: &str) -> Row {
        Row {
            bbox: (x0, y0, x1, y1),
            site: String::new(),
            orient: orient.into(),
        }
    }

    #[test]
    fn the_width_is_the_narrowest_supply_pin_on_a_routing_layer() {
        let boxes = [
            (true, true, true, 340),  // a core cell's supply pin
            (true, true, true, 300),  // a narrower one -- this wins
            (true, false, true, 100), // signal, ignored
            (true, true, false, 50),  // not a routing layer, ignored
            (false, true, true, 20),  // not a core master, ignored
        ];
        assert_eq!(determine_width(&boxes), Some(300));
    }

    #[test]
    fn with_no_supply_pin_to_measure_there_is_no_width() {
        // The reference errors here rather than defaulting, so `None` has to reach the caller.
        assert_eq!(determine_width(&[(true, false, true, 100)]), None);
        assert_eq!(determine_width(&[]), None);
    }

    #[test]
    fn every_row_gets_a_power_rail_and_a_ground_rail_in_that_order() {
        let rows = [row(0, 0, 1000, 200, "R0")];
        let out = make(
            "m1",
            "VDD",
            "VSS",
            40,
            &rows,
            (0, 0, 1000, 200),
            (0, 0, 1000, 200),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].net, "VDD",
            "power first, whatever order the grid uses elsewhere"
        );
        assert_eq!(out[1].net, "VSS");
    }

    #[test]
    fn an_r0_row_puts_power_on_its_top_edge() {
        let rows = [row(0, 0, 1000, 200, "R0")];
        let out = make(
            "m1",
            "VDD",
            "VSS",
            40,
            &rows,
            (0, 0, 1000, 200),
            (0, 0, 1000, 200),
        );
        assert_eq!(
            out[0].rect,
            (0, 180, 1000, 220),
            "power straddles the top edge"
        );
        assert_eq!(
            out[1].rect,
            (0, -20, 1000, 20),
            "ground straddles the bottom"
        );
    }

    #[test]
    fn any_orientation_other_than_r0_swaps_them() {
        // ⚠️ Including MY, which does not mirror vertically. The test is `== R0`, so MY lands with
        // the flipped rows rather than with R0.
        for orient in ["MX", "MY", "R180", "MXR90"] {
            let rows = [row(0, 0, 1000, 200, orient)];
            let out = make(
                "m1",
                "VDD",
                "VSS",
                40,
                &rows,
                (0, 0, 1000, 200),
                (0, 0, 1000, 200),
            );
            assert_eq!(
                out[0].rect,
                (0, -20, 1000, 20),
                "power on the bottom for {orient}"
            );
        }
    }

    #[test]
    fn a_row_flush_with_the_core_is_stretched_to_the_boundary() {
        let rows = [row(100, 0, 900, 200, "R0")];
        let core = (100, 0, 900, 200);
        let out = make("m1", "VDD", "VSS", 40, &rows, core, (0, 0, 1000, 200));
        assert_eq!((out[0].rect.0, out[0].rect.2), (0, 1000));
    }

    #[test]
    fn a_row_one_unit_inside_the_core_is_not_stretched_at_all() {
        // ⚠️ Equality, not a tolerance. This is the whole rule: a design whose rows are inset by
        // any amount gets no extension, and a comparison written with `<=` would extend every row.
        let rows = [row(101, 0, 899, 200, "R0")];
        let core = (100, 0, 900, 200);
        let out = make("m1", "VDD", "VSS", 40, &rows, core, (0, 0, 1000, 200));
        assert_eq!(
            (out[0].rect.0, out[0].rect.2),
            (101, 899),
            "keeps its own edges"
        );
    }

    #[test]
    fn each_end_is_tested_on_its_own() {
        let rows = [row(100, 0, 899, 200, "R0")];
        let core = (100, 0, 900, 200);
        let out = make("m1", "VDD", "VSS", 40, &rows, core, (0, 0, 1000, 200));
        assert_eq!(
            (out[0].rect.0, out[0].rect.2),
            (0, 899),
            "left stretched, right not"
        );
    }

    #[test]
    fn an_odd_width_puts_the_extra_unit_above_the_row_edge() {
        let rows = [row(0, 0, 1000, 200, "R0")];
        let out = make(
            "m1",
            "VDD",
            "VSS",
            41,
            &rows,
            (0, 0, 1000, 200),
            (0, 0, 1000, 200),
        );
        assert_eq!(out[0].rect, (0, 180, 1000, 221));
    }

    #[test]
    fn a_fixed_extend_mode_uses_the_core_here_unlike_a_strap_set() {
        use crate::straps::Extend;
        let core = (10, 10, 90, 90);
        let rings = (5, 5, 95, 95);
        let grid = (0, 0, 100, 100);
        assert_eq!(
            boundary(Extend::Fixed, core, rings, grid),
            core,
            "NOT the fixed coordinates"
        );
        assert_eq!(boundary(Extend::Core, core, rings, grid), core);
        assert_eq!(boundary(Extend::Rings, core, rings, grid), rings);
        assert_eq!(boundary(Extend::Boundary, core, rings, grid), grid);
    }
}
