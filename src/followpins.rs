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
    /// The SITE's height, which is what `determinePitch` compares rows by. ⚠️ Not the same as
    /// `bbox.dy()`: a row is selected by its site height and then measured by its bbox, and for a
    /// row spanning several site rows those two differ.
    pub site_height: i32,
    /// ⛔ **`R0` AND `MY` are right side up.** Only `MX` ("FS") and `R180` ("S") invert the
    /// master's y-axis and so swap the rails; `MY` ("FN") mirrors in x and leaves them alone.
    /// ⚠️ An earlier reading here tested `== "R0"` alone and put `MY` with the flipped rows.
    pub orient: String,
    /// Whether the site carries a ROW PATTERN — a hybrid site. Such a row is skipped outright:
    /// *"its row pattern holds the rails"*.
    pub has_row_pattern: bool,
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
    row_height: i32,
) -> Vec<Rail> {
    let mut out = Vec::with_capacity(rows.len() * 2);
    for row in rows {
        let (bx0, by0, bx1, by1) = row.bbox;
        let x0 = if bx0 == core.0 { boundary.0 } else { bx0 };
        let x1 = if bx1 == core.2 { boundary.2 } else { bx1 };

        // ⛔ **A hybrid site's row is SKIPPED, not laid.** *"Skipping row {} of hybrid site {},
        // its row pattern holds the rails"* — the pattern already describes where the rails go,
        // so stepping through the row would lay a second, wrong set over them.
        if row.has_row_pattern {
            continue;
        }
        let site_height = row.site_height;
        // ⛔ **A row whose height is NOT a whole number of standard-cell rows has no interior
        // boundary**, so it steps by its OWN height and therefore emits only its two edges.
        // Upstream's example is the 9-track row of a 9/7-track hybrid pattern: stepping it by the
        // 7-track row height would lay rails across the middle of the cells and never land on the
        // upper edge.
        let spans_whole_rows = row_height > 0 && site_height % row_height == 0;
        let rail_pitch = if spans_whole_rows { row_height } else { site_height };
        // ⛔ **An EVEN-height row always starts with GROUND, whatever its orientation.**
        // *"A row spanning an even number of standard cell rows carries the same net at both of
        // its edges, so its orientation says nothing about which net that is."* Reading the
        // orientation anyway is what put a double-height row's ladder out of phase with the
        // single-height rows it overlaps — measured on `core_grid_multiheight_rows`, where our
        // `FS` double row started with power and fought the single rows at 13600 and 16320.
        let is_right_side_up = row.orient == "R0" || row.orient == "MY";
        let even_height_row = row_height > 0 && site_height % (2 * row_height) == 0;
        let start_with_power = if even_height_row { false } else { !is_right_side_up };
        // ⛔ **A row is not two edges, it is a LADDER of `row_height` steps.**
        //
        // ```text
        // bool do_power = start_with_power;
        // for (int y = bbox.yMin(); y <= bbox.yMax(); y += row_height_) { ...; do_power = !do_power; }
        // ```
        //
        // 🔑 The old rule emitted exactly two straps, power at one edge and ground at the other.
        // That describes a row exactly one site high and nothing else: a row spanning an EVEN
        // number of standard-cell rows carries the SAME net on both its edges, so "power at one
        // edge, ground at the other" puts one of the two straps on the wrong net — and nothing
        // reports it, because `GridComponent::addShape` drops a cross-net overlap with only a
        // debug message. Upstream `2d8976cd6b`, *"pdn: use row height for power / ground followpin
        // straps"*, and its test says the requirement plainly: *"The rails must alternate VSS/VDD
        // on every 2.72um row boundary."*
        //
        // ⚠️ **`<=`, so `yMax` is included** — a single-height row still emits its two edge straps
        // and this is a strict generalisation of the old rule, not a change to it.
        //
        // ⚠️ **The EMISSION ORDER changed and it is load-bearing.** Straps come out bottom-up
        // starting at `yMin`, where before it was always power then ground. `addShape` drops a
        // later cross-net overlap, so whichever strap is added FIRST wins the square — the test's
        // own floorplan lists its double-height rows before the single-height rows they overlap
        // precisely so that a wrong strap would be inserted first and displace a correct rail.
        let mut do_power = start_with_power;
        let mut y = by0;
        while y <= by1 {
            // ⚠️ `- width / 2` then `+ width`, the same asymmetry the straps have: for an odd
            // width the extra unit falls on the high side of the row edge.
            let y_start = y - width / 2;
            out.push(Rail {
                layer: layer.into(),
                net: if do_power { power.into() } else { ground.into() },
                rect: (x0, y_start, x1, y_start + width),
            });
            do_power = !do_power;
            // A zero or negative step cannot terminate. Upstream cannot reach `makeShapes` with
            // one — `FollowPins`' constructor raises PDN-190 *"Unable to determine the pitch of
            // the rows"* when `row_height_ == 0` — so refuse the row rather than invent a step.
            if rail_pitch <= 0 {
                break;
            }
            y += rail_pitch;
        }
    }
    out
}

/// **F1b** — the row height every follow pin set steps by, and half its pitch.
///
/// ```text
/// const auto min_row = std::min_element(rows.begin(), rows.end(),
///     [](dbRow* a, dbRow* b) { return a->getSite()->getHeight() < b->getSite()->getHeight(); });
/// row_height_ = (*min_row)->getBBox().dy();
/// setPitch(2 * row_height_);
/// ```
///
/// ⛔ **Selected by SITE height, measured by BBOX height, and the two are not the same number.**
/// A row is chosen for having the smallest site and then contributes its bounding box's `dy`, so a
/// row built on the smallest site but spanning several rows vertically yields a LARGE step.
/// Reading either field for both halves is a different rule.
///
/// ⚠️ **The FIRST minimum wins**, as `std::min_element` returns the first of equal elements —
/// which is also what Rust's `min_by_key` does, so the transcription is direct.
///
/// ⚠️ This was previously read off `rows.first()`, which agrees only when every row is the same
/// height. That held for every case in the corpus until upstream added the multi-height ones.
pub fn row_height(rows: &[Row]) -> Option<i32> {
    let min_row = rows.iter().min_by_key(|r| r.site_height)?;
    Some(min_row.bbox.3 - min_row.bbox.1)
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
            site_height: y1 - y0,
            has_row_pattern: false,
        }
    }

    /// A row built on a SMALL site but spanning `n` of them — what a multi-height floorplan has.
    fn tall_row(x0: i32, y0: i32, x1: i32, y1: i32, orient: &str, site_height: i32) -> Row {
        Row {
            bbox: (x0, y0, x1, y1),
            site: String::new(),
            orient: orient.into(),
            site_height,
            has_row_pattern: false,
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
    /// ⛔ **The ORDER is bottom-up from `yMin`, and it CHANGED at `2d8976cd6b`.** The old rule
    /// emitted power then ground; the ladder emits `yMin` first and alternates, so an `R0` row
    /// (power on top) now yields ground, then power. The RECTANGLES are unchanged — this is only
    /// the order they are added in, which decides who wins a cross-net overlap in `addShape`.
    fn every_row_gets_a_ground_rail_then_a_power_rail_bottom_up() {
        let rows = [row(0, 0, 1000, 200, "R0")];
        let out = make(
            "m1",
            "VDD",
            "VSS",
            40,
            &rows,
            (0, 0, 1000, 200),
            (0, 0, 1000, 200),
            200,
        );
        assert_eq!(out.len(), 2, "a single-height row still emits exactly two rails");
        assert_eq!(out[0].net, "VSS", "yMin comes first, and an R0 row has ground there");
        assert_eq!(out[1].net, "VDD");
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
            200,
        );
        assert_eq!(out[1].rect, (0, 180, 1000, 220), "power straddles the top edge");
        assert_eq!(out[0].rect, (0, -20, 1000, 20), "ground straddles the bottom");
    }

    /// ⛔ **Only `MX` and `R180` swap the rails; `R0` and `MY` do NOT.**
    ///
    /// `is_right_side_up = orient == R0 || orient == MY` — those two leave the master's y-axis
    /// alone, and `MY` ("FN") mirrors in x. ⚠️ This engine previously tested `== "R0"` alone and
    /// filed `MY` with the flipped rows, which is a rail on the wrong net for every `FN` row.
    #[test]
    fn only_the_y_inverting_orientations_swap_the_rails() {
        for orient in ["R0", "MY"] {
            let rows = [row(0, 0, 1000, 200, orient)];
            let out = make("m1", "VDD", "VSS", 40, &rows, (0, 0, 1000, 200), (0, 0, 1000, 200), 200);
            assert_eq!(out[0].net, "VSS", "{orient} is right side up: ground at yMin");
            assert_eq!(out[1].net, "VDD", "and power on top for {orient}");
        }
        for orient in ["MX", "R180", "MXR90"] {
            let rows = [row(0, 0, 1000, 200, orient)];
            let out = make(
                "m1",
                "VDD",
                "VSS",
                40,
                &rows,
                (0, 0, 1000, 200),
                (0, 0, 1000, 200),
                200,
            );
            assert_eq!(out[0].rect, (0, -20, 1000, 20), "power on the bottom for {orient}");
            assert_eq!(out[0].net, "VDD", "and it IS the power net for {orient}");
            assert_eq!(out[1].net, "VSS", "ground on top for {orient}");
        }
    }

    /// ⛔ **A double-height row carries the SAME net on BOTH its edges**, so the old two-strap
    /// model had to put one of them on the wrong net. Stepping by the row height instead emits
    /// THREE rails — bottom, middle, top — alternating, and the middle one is the rail the old
    /// rule could not express at all.
    ///
    /// Upstream `core_grid_multiheight_rows.tcl` states the requirement: *"The rails must
    /// alternate VSS/VDD on every 2.72um row boundary."*
    #[test]
    fn a_double_height_row_gets_a_rail_at_every_site_boundary_not_just_its_edges() {
        // A row on a 400-unit DOUBLE-height site, in a design whose standard row is 200.
        let rows = [tall_row(0, 0, 1000, 400, "R0", 400)];
        let out = make("m1", "VDD", "VSS", 40, &rows, (0, 0, 1000, 400), (0, 0, 1000, 400), 200);
        assert_eq!(out.len(), 3, "yMin, the interior boundary, and yMax -- `<=` includes yMax");
        let ys: Vec<i32> = out.iter().map(|r| r.rect.1 + 20).collect();
        assert_eq!(ys, vec![0, 200, 400], "one rail per 200-unit row boundary");
        let nets: Vec<&str> = out.iter().map(|r| r.net.as_str()).collect();
        assert_eq!(nets, vec!["VSS", "VDD", "VSS"], "alternating, starting !power_on_top");
        // ⚠️ The probe must be able to FAIL: the OLD rule emitted two rails on opposite nets, so
        // both the count and the repeated VSS at the two edges are new facts.
        assert_eq!(nets[0], nets[2], "both EDGES carry the same net -- the whole defect");
    }

    /// ⚠️ Selected by SITE height, measured by BBOX height. A floorplan listing a double-height
    /// row alongside single-height ones must step by the SINGLE height.
    /// ⛔ **An even-height row starts with GROUND whatever its orientation**, because it carries
    /// the same net at both edges and so *"its orientation says nothing about which net that is"*.
    ///
    /// ⚠️ This is what put us out of phase on `core_grid_multiheight_rows`: its `DROW_2` is an
    /// `FS` double-height row, and reading the orientation started it with POWER, fighting the
    /// single-height rows that share its 13600 and 16320 boundaries.
    #[test]
    fn an_even_height_rows_orientation_does_not_decide_its_first_rail() {
        for orient in ["R0", "MY", "MX", "R180"] {
            let rows = [tall_row(0, 0, 1000, 400, orient, 400)];
            let out = make("m1", "VDD", "VSS", 40, &rows, (0, 0, 1000, 400), (0, 0, 1000, 400), 200);
            let nets: Vec<&str> = out.iter().map(|r| r.net.as_str()).collect();
            assert_eq!(nets, vec!["VSS", "VDD", "VSS"], "ground first for {orient}");
        }
        // ⚠️ The probe must be able to FAIL: an ODD-height row DOES read its orientation, so the
        // two groups must not agree, or this test proves nothing about the even-height rule.
        let odd = [row(0, 0, 1000, 200, "MX")];
        let out = make("m1", "VDD", "VSS", 40, &odd, (0, 0, 1000, 200), (0, 0, 1000, 200), 200);
        assert_eq!(out[0].net, "VDD", "an odd-height MX row still starts with power");
    }

    /// ⛔ A row on a HYBRID site is skipped outright — *"its row pattern holds the rails"*.
    #[test]
    fn a_hybrid_sites_row_is_skipped_entirely() {
        let mut r = row(0, 0, 1000, 200, "R0");
        r.has_row_pattern = true;
        assert!(
            make("m1", "VDD", "VSS", 40, &[r], (0, 0, 1000, 200), (0, 0, 1000, 200), 200).is_empty(),
            "the row pattern already placed these rails"
        );
    }

    /// ⛔ A row that is NOT a whole number of standard rows steps by its OWN height, so it emits
    /// only its two edges — stepping it by the standard row would lay rails across the cells and
    /// never land on the top edge. Upstream's example is the 9-track row of a 9/7-track hybrid.
    #[test]
    fn a_row_that_is_not_a_whole_number_of_rows_emits_only_its_edges() {
        // A 270-tall site in a design whose standard row is 200: 270 % 200 != 0.
        let rows = [tall_row(0, 0, 1000, 270, "R0", 270)];
        let out = make("m1", "VDD", "VSS", 40, &rows, (0, 0, 1000, 270), (0, 0, 1000, 270), 200);
        assert_eq!(out.len(), 2, "two edges, no interior rail");
        let ys: Vec<i32> = out.iter().map(|r| r.rect.1 + 20).collect();
        assert_eq!(ys, vec![0, 270], "stepped by 270, not by 200");
    }

    #[test]
    fn the_step_comes_from_the_smallest_site_not_the_first_row() {
        let rows = [
            // The multi-height floorplan lists its double-height rows FIRST -- reading
            // `rows.first()` would step by 400 and skip every interior boundary. A row on the
            // `unithddbl` site has the TALLER site, so it loses the min_element.
            tall_row(0, 0, 1000, 400, "R0", 400),
            row(0, 0, 1000, 200, "R0"),
        ];
        assert_eq!(row_height(&rows), Some(200), "the min-site row, measured by its bbox");
        // ⚠️ The probe must be able to FAIL: `rows.first()` would answer 400 here, which is the
        // number this engine used to compute and the reason the interior rails went missing.
        assert_ne!(row_height(&rows), Some(400), "reading the FIRST row is the old bug");
    }

    /// ⛔ A row chosen for the smallest SITE still contributes its own BBOX height, so reading one
    /// field for both halves of the rule gives a different number.
    #[test]
    fn the_min_site_row_is_measured_by_its_bbox_not_by_its_site() {
        // Smallest site (100) but spanning four of them, beside a plain 200-tall row.
        let rows = [tall_row(0, 0, 1000, 400, "R0", 100), row(0, 0, 1000, 200, "R0")];
        assert_eq!(row_height(&rows), Some(400), "bbox dy of the min-SITE row, not its site");
    }

    #[test]
    fn with_no_rows_there_is_no_step() {
        assert_eq!(row_height(&[]), None);
    }

    #[test]
    fn a_row_flush_with_the_core_is_stretched_to_the_boundary() {
        let rows = [row(100, 0, 900, 200, "R0")];
        let core = (100, 0, 900, 200);
        let out = make("m1", "VDD", "VSS", 40, &rows, core, (0, 0, 1000, 200), 200);
        assert_eq!((out[0].rect.0, out[0].rect.2), (0, 1000));
    }

    #[test]
    fn a_row_one_unit_inside_the_core_is_not_stretched_at_all() {
        // ⚠️ Equality, not a tolerance. This is the whole rule: a design whose rows are inset by
        // any amount gets no extension, and a comparison written with `<=` would extend every row.
        let rows = [row(101, 0, 899, 200, "R0")];
        let core = (100, 0, 900, 200);
        let out = make("m1", "VDD", "VSS", 40, &rows, core, (0, 0, 1000, 200), 200);
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
        let out = make("m1", "VDD", "VSS", 40, &rows, core, (0, 0, 1000, 200), 200);
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
            200,
        );
        assert_eq!(out[1].rect, (0, 180, 1000, 221));
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
