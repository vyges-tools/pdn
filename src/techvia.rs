// SPDX-License-Identifier: Apache-2.0
//! Vias the technology already declares, as opposed to vias built from a rule.
//!
//! A `-fixed_vias` connect names vias by name — `VIA12`, `VIA23` — and the technology supplies
//! their geometry outright: cut boxes on a cut layer, metal boxes on the two routing layers. There
//! is nothing to choose and no enclosure to select. What remains is to work out how many of them
//! fit and where, and that is the **same** machinery a generated via uses: `TechViaGenerator`
//! hands rows, columns and pitch to `DbTechVia` through the shared `makeBaseVia`, so everything in
//! [`crate::viagen`] applies unchanged.
//!
//! 🔑 **What is different is only the geometry, and it comes from the boxes.** This module derives
//! it, and nothing here touches a database.
//!
//! ⚠️ **A named via that the technology has as a generate RULE is a different thing entirely.**
//! `pdn.tcl` looks a fixed-via name up twice — `findVia` and `findViaGenerateRule` — and either
//! may answer. Only the first kind is this module's business.

use crate::Rect;

/// A tech via's geometry, derived from its boxes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TechVia {
    /// The cut layer's name, taken from whichever box sits on a cut layer.
    pub cut_layer: String,
    /// One cut, which is what a whole array is built from.
    pub single_cut: Rect,
    /// Every cut merged — the extent an enclosure is measured against.
    pub cut_extent: Rect,
    /// The centre of each cut, in the order the boxes were given.
    pub cut_centres: Vec<(i32, i32)>,
    /// The metal on the bottom layer, merged.
    pub bottom_metal: Rect,
    /// The metal on the top layer, merged.
    pub top_metal: Rect,
}

/// **X1** — a tech via's geometry from its boxes.
///
/// Mirrors `DbTechVia`'s constructor exactly:
///
/// - a box on a **cut** layer names the cut layer, and `single_cut` is the **last** such box —
///   not the first, and not their union;
/// - `cut_extent` is every cut box merged, and each contributes its centre;
/// - a non-cut box is bottom metal if it sits on the via's bottom layer and top metal otherwise —
///   ⚠️ **the test is against the bottom layer alone**, so a via with a box on some third layer
///   files it under top rather than discarding it.
///
/// `boxes` is `(layer, rect)`; `cut_layers` says which layer names are cut layers.
pub fn geometry(
    boxes: &[(String, Rect)],
    bottom_layer: &str,
    is_cut: &dyn Fn(&str) -> bool,
) -> Option<TechVia> {
    let mut cut_layer = String::new();
    let mut single_cut = None;
    let mut cut_extent: Option<Rect> = None;
    let mut cut_centres = Vec::new();
    let mut bottom_metal: Option<Rect> = None;
    let mut top_metal: Option<Rect> = None;

    for (layer, rect) in boxes {
        if is_cut(layer) {
            cut_layer = layer.clone();
            single_cut = Some(*rect);
            cut_extent = Some(match cut_extent {
                Some(e) => merge(e, *rect),
                None => *rect,
            });
            cut_centres.push(((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2));
        } else if layer == bottom_layer {
            bottom_metal = Some(match bottom_metal {
                Some(e) => merge(e, *rect),
                None => *rect,
            });
        } else {
            top_metal = Some(match top_metal {
                Some(e) => merge(e, *rect),
                None => *rect,
            });
        }
    }

    Some(TechVia {
        cut_layer,
        single_cut: single_cut?,
        cut_extent: cut_extent?,
        cut_centres,
        bottom_metal: bottom_metal?,
        top_metal: top_metal?,
    })
}

/// **X2** — the enclosure a tech via's own metal offers on one of its faces.
///
/// ⚠️ **The LARGER of the two sides on each axis.** `TechViaGenerator::getMinimumEnclosures` takes
/// `std::max` of the two margins, and it is not the question "what does this metal enclose on
/// every side" — it is the floor a rule-derived candidate is raised to, so the generous side
/// governs. Taking the smaller reads as the safer choice and builds vias narrower than the
/// reference's on every face whose metal is offset.
///
/// ⓘ This is a **floor, not the answer**: see [`reconcile_enclosures`].
pub fn enclosure(cut_extent: Rect, metal: Rect) -> (i32, i32) {
    (
        (cut_extent.0 - metal.0).max(metal.2 - cut_extent.2),
        (cut_extent.1 - metal.1).max(metal.3 - cut_extent.3),
    )
}

/// **X8** — reconcile the layer's enclosure rules with what the tech via's own metal offers.
///
/// 🔑 **A tech via is not built with its own metal margins.** It is built with an enclosure chosen
/// from the layer's rules and then *raised* to the via's own, per axis:
///
/// - a candidate whose X **and** Y are both below the floor is **erased** — ⚠️ both, so one short
///   on a single axis survives and is raised;
/// - every survivor is raised to the floor on each axis independently;
/// - if nothing survives, the floor itself is the only candidate.
///
/// This is what makes ASAP7's M5 patches 916 wide where the tech via's own metal encloses 0 in x:
/// V4 states a `DEFAULT 11 0` enclosure, which survives (11 >= 0) and has its y raised to the
/// via's 11, giving (11, 11) and `894 + 22`.
pub fn reconcile_enclosures(rules: &[(i32, i32)], floor: (i32, i32)) -> Vec<(i32, i32)> {
    let kept: Vec<(i32, i32)> = rules
        .iter()
        .filter(|(x, y)| !(*x < floor.0 && *y < floor.1))
        .map(|(x, y)| ((*x).max(floor.0), (*y).max(floor.1)))
        .collect();
    if kept.is_empty() {
        vec![floor]
    } else {
        kept
    }
}

/// **X3** — the centre of a tech via's cut extent, which is what a placement is offset by.
///
/// ⚠️ Not the midpoint of the two edges: the reference computes `xMin + dx / 2`, which for an odd
/// width lands one unit lower than `(xMin + xMax) / 2` would.
pub fn centre(cut_extent: Rect) -> (i32, i32) {
    (
        cut_extent.0 + (cut_extent.2 - cut_extent.0) / 2,
        cut_extent.1 + (cut_extent.3 - cut_extent.1) / 2,
    )
}

/// **X4** — the name the reference gives an arrayed tech via.
///
/// `{via}_{rows}_{columns}_{row pitch}_{column pitch}`, then one suffix per on-grid layer. The
/// name is worth reproducing exactly: it encodes the array this module derived, so a DEF diff
/// checks the derivation on every via without inspecting any geometry.
pub fn array_name(via: &str, rows: i32, columns: i32, row_pitch: i32, col_pitch: i32) -> String {
    format!("{via}_{rows}_{columns}_{row_pitch}_{col_pitch}")
}

/// **X5** — the cut spacing a via array needs, given the pitch it is placed on.
///
/// ⚠️ **Spacing is pitch MINUS the cut, per axis** — the parameters a via carries are spacings,
/// not pitches, and handing a pitch straight through spaces every cut by a whole cut too far.
///
/// ⚠️ **And the cut here IS a single one**, unlike [`base_cut_pitch`]: `DbTechVia::generate` takes
/// `cut_width` from `single_via_rect_` while the pitch it subtracts from came from the outline.
pub fn cut_spacing(single_cut: Rect, row_pitch: i32, col_pitch: i32) -> (i32, i32) {
    (
        col_pitch - (single_cut.2 - single_cut.0),
        row_pitch - (single_cut.3 - single_cut.1),
    )
}

/// **X6** — the pitch a via's cuts sit on before any adjacency rule adjusts it.
///
/// `ViaGenerator::determineCutSpacing`, in order:
///
/// 1. the **cut layer's own** `getSpacing()`, applied to both axes when it is non-zero;
/// 2. then, if the via has a cut class, the LEF58 cut-spacing table for that class — a rule
///    marked same-net wins **immediately**, and otherwise the **largest** spacing found wins;
///    ⚠️ and that only applies when **both** axes came back non-zero.
///
/// Pitch is always `cut + spacing`, per axis. Returns `None` when the technology states neither,
/// which is not an error: a connect may state `-cut_pitch` itself and that overrides all of this.
///
/// ⚠️ **The cut here is the merged OUTLINE, not one cut.** `TechViaGenerator::getCut()` returns
/// `cut_outline_`, so both this and the row/column count reason about all the via's cuts together;
/// only the built via's own parameters use a single cut's size. A tech via with one cut box makes
/// the two identical, which is why the distinction survives being got wrong.
pub fn base_cut_pitch(
    cut_extent: Rect,
    layer_spacing: i32,
    class_spacing: Option<(i32, i32)>,
) -> Option<(i32, i32)> {
    let (w, h) = (cut_extent.2 - cut_extent.0, cut_extent.3 - cut_extent.1);
    let mut pitch = if layer_spacing != 0 {
        Some((w + layer_spacing, h + layer_spacing))
    } else {
        None
    };
    if let Some((sx, sy)) = class_spacing {
        // ⚠️ Both, not either: one axis alone leaves the layer's answer standing.
        if sx != 0 && sy != 0 {
            pitch = Some((w + sx, h + sy));
        }
    }
    pitch
}

/// One LEF58 cut-spacing-table rule, reduced to what the pitch derivation reads from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpacingTableRule {
    /// A rule naming a second layer is skipped outright.
    pub has_second_layer: bool,
    pub same_net: bool,
    pub centre_and_edge: bool,
    pub centre_to_centre: bool,
    /// `getMaxSpacing(class, class, MIN)` — used when the cut is square.
    pub max_spacing: i32,
    /// `getSpacing(class, side, class, side)` for the two axes, already crossed: `.0` is what the
    /// **x** pitch asks for and `.1` what the **y** pitch asks for.
    pub sided: (i32, i32),
}

/// **X7** — the cut spacing a class-bearing cut takes from its layer's spacing tables.
///
/// 🔑 **This is where the pitch comes from when the cut layer states no `SPACING` of its own** —
/// which is every ASAP7 cut layer, and why a three-level `M3 M6` stack built nothing at all until
/// it was implemented.
///
/// `ViaGenerator::determineCutSpacing`, second branch, in order:
///
/// - a rule naming a **second layer** is skipped;
/// - a **square** cut takes `getMaxSpacing(class, class, MIN)` on both axes; a non-square one
///   takes `getSpacing` per axis — ⚠️ **with the side flags CROSSED**, because a cut's `dx` is its
///   *side* when `dx > dy`, so the x pitch is asked with the y flag;
/// - **centre-to-centre** values have a cut subtracted; centre-and-edge and plain ones are already
///   spacings;
/// - a **same-net** rule wins immediately and stops the scan; otherwise the **largest** wins;
/// - ⚠️ and the result is taken only when **both** axes came back non-zero.
pub fn class_cut_spacing(cut: Rect, rules: &[SpacingTableRule]) -> Option<(i32, i32)> {
    let (w, h) = (cut.2 - cut.0, cut.3 - cut.1);
    let (mut max_x, mut max_y) = (0, 0);
    for r in rules {
        if r.has_second_layer {
            continue;
        }
        let (mut sx, mut sy) = if w == h {
            (r.max_spacing, r.max_spacing)
        } else {
            r.sided
        };
        if r.centre_to_centre && !r.centre_and_edge {
            sx -= w;
            sy -= h;
        }
        if r.same_net {
            return (sx != 0 && sy != 0).then_some((sx, sy));
        }
        max_x = max_x.max(sx);
        max_y = max_y.max(sy);
    }
    (max_x != 0 && max_y != 0).then_some((max_x, max_y))
}

fn merge(a: Rect, b: Rect) -> Rect {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cutty(l: &str) -> bool {
        l.starts_with('V')
    }

    /// A via with three cuts in a row: metal on M1 and M2, cuts on V1.
    fn three_cuts() -> Vec<(String, Rect)> {
        vec![
            ("M1".into(), (-100, -50, 100, 50)),
            ("M2".into(), (-110, -40, 110, 40)),
            ("V1".into(), (-45, -9, -27, 9)),
            ("V1".into(), (-9, -9, 9, 9)),
            ("V1".into(), (27, -9, 45, 9)),
        ]
    }

    #[test]
    fn the_cut_layer_and_a_single_cut_come_from_the_cut_boxes() {
        let g = geometry(&three_cuts(), "M1", &cutty).unwrap();
        assert_eq!(g.cut_layer, "V1");
        // ⚠️ The LAST cut box, not the first and not their union.
        assert_eq!(g.single_cut, (27, -9, 45, 9));
        assert_eq!(g.cut_extent, (-45, -9, 45, 9));
        assert_eq!(g.cut_centres, vec![(-36, 0), (0, 0), (36, 0)]);
    }

    #[test]
    fn metal_is_split_by_the_bottom_layer_alone() {
        let g = geometry(&three_cuts(), "M1", &cutty).unwrap();
        assert_eq!(g.bottom_metal, (-100, -50, 100, 50));
        assert_eq!(g.top_metal, (-110, -40, 110, 40));
        // Naming the other layer as the bottom swaps them, and nothing is lost either way.
        let g2 = geometry(&three_cuts(), "M2", &cutty).unwrap();
        assert_eq!(g2.bottom_metal, (-110, -40, 110, 40));
        assert_eq!(g2.top_metal, (-100, -50, 100, 50));
    }

    #[test]
    fn a_via_with_no_cut_has_no_geometry() {
        let boxes = vec![("M1".into(), (0, 0, 10, 10)), ("M2".into(), (0, 0, 10, 10))];
        assert_eq!(geometry(&boxes, "M1", &cutty), None);
    }

    #[test]
    fn an_offset_enclosure_takes_its_longer_side() {
        // ⚠️ Metal reaches 100 on the right of the cuts but only 60 on the left, and the floor
        // takes 55 — the generous side — because this is a minimum for rules to be raised to,
        // not a guarantee about every side.
        let metal = (-60, -50, 100, 50);
        assert_eq!(enclosure((-45, -9, 45, 9), metal), (55, 41));
    }

    #[test]
    fn asap7s_via45_top_face_reconciles_to_the_reference_enclosure() {
        // 🔑 The case that settles the M5 DRCFILL width. VIA45's M5 metal offers 0 in x and 11 in
        // y; V4's rules offer (11, 0). The rule survives on x and is raised on y.
        let floor = enclosure((-12, -12, 12, 12), (-12, -23, 12, 23));
        assert_eq!(floor, (0, 11));
        assert_eq!(reconcile_enclosures(&[(11, 0)], floor), vec![(11, 11)]);
    }

    #[test]
    fn a_candidate_short_on_both_axes_is_erased_but_one_short_axis_survives() {
        let floor = (10, 10);
        assert_eq!(reconcile_enclosures(&[(5, 5)], floor), vec![(10, 10)]);
        assert_eq!(reconcile_enclosures(&[(20, 5)], floor), vec![(20, 10)]);
        assert_eq!(reconcile_enclosures(&[(5, 20)], floor), vec![(10, 20)]);
    }

    #[test]
    fn with_no_rules_at_all_the_floor_is_the_only_candidate() {
        assert_eq!(reconcile_enclosures(&[], (3, 4)), vec![(3, 4)]);
    }

    #[test]
    fn a_symmetric_enclosure_is_the_margin_on_either_side() {
        assert_eq!(enclosure((-45, -9, 45, 9), (-100, -50, 100, 50)), (55, 41));
    }

    #[test]
    fn the_centre_is_the_low_edge_plus_half_the_span() {
        assert_eq!(centre((-45, -9, 45, 9)), (0, 0));
        // ⚠️ An odd span lands low, as `xMin + dx / 2` does and `(xMin + xMax) / 2` would not.
        assert_eq!(centre((0, 0, 3, 3)), (1, 1));
    }

    #[test]
    fn the_array_name_carries_the_array() {
        // The reference's own name for the ASAP7 M1-M2 via.
        assert_eq!(array_name("VIA12", 1, 49, 288, 288), "VIA12_1_49_288_288");
    }

    #[test]
    fn asap7s_via12_derives_the_array_the_reference_names() {
        // 🔑 ASAP7's own VIA12, read from the technology: one 18x18 cut on V1, M1 metal taller
        // than the cut and M2 metal wider than it.
        let boxes: Vec<(String, Rect)> = vec![
            ("M1".into(), (-9, -11, 9, 11)),
            ("M2".into(), (-14, -9, 14, 9)),
            ("V1".into(), (-9, -9, 9, 9)),
        ];
        let g = geometry(&boxes, "M1", &cutty).unwrap();
        assert_eq!(g.cut_layer, "V1");
        assert_eq!(g.single_cut, (-9, -9, 9, 9));
        assert_eq!(g.cut_extent, (-9, -9, 9, 9));
        assert_eq!(g.cut_centres, vec![(0, 0)]);
        assert_eq!(centre(g.cut_extent), (0, 0));

        // The metal encloses in one direction only on each layer, which is the point of X2:
        // M1 adds 2 in y and nothing in x, M2 adds 5 in x and nothing in y.
        assert_eq!(enclosure(g.cut_extent, g.bottom_metal), (0, 2));
        assert_eq!(enclosure(g.cut_extent, g.top_metal), (5, 0));

        // The case states `-cut_pitch 0.288`, and the reference named the result
        // `VIA12_1_49_288_288`. Both fall out of the geometry above.
        assert_eq!(cut_spacing(g.single_cut, 288, 288), (270, 270));
        assert_eq!(array_name("VIA12", 1, 49, 288, 288), "VIA12_1_49_288_288");
    }

    #[test]
    fn the_layers_own_spacing_sets_the_pitch() {
        // ASAP7 V2: an 18x18 cut at spacing 18 gives the 36 the reference's VIA23 name carries.
        assert_eq!(base_cut_pitch((0, 0, 18, 18), 18, None), Some((36, 36)));
    }

    fn plain(max: i32, sided: (i32, i32)) -> SpacingTableRule {
        SpacingTableRule {
            has_second_layer: false,
            same_net: false,
            centre_and_edge: false,
            centre_to_centre: false,
            max_spacing: max,
            sided,
        }
    }

    #[test]
    fn asap7s_v3_table_gives_the_pitch_the_reference_named() {
        // 🔑 V3 states no SPACING of its own; its single table answers 34 for every class, and
        // VIA34's cut is 18x24 — so pitch (52, 58), which is exactly `VIA34_20_18_58_52`.
        let cut = (-9, -12, 9, 12);
        let s = class_cut_spacing(cut, &[plain(34, (34, 34))]).unwrap();
        assert_eq!(s, (34, 34));
        assert_eq!(base_cut_pitch(cut, 0, Some(s)), Some((52, 58)));
    }

    #[test]
    fn a_rule_naming_a_second_layer_is_skipped() {
        let mut r = plain(99, (99, 99));
        r.has_second_layer = true;
        assert_eq!(class_cut_spacing((0, 0, 18, 24), &[r]), None);
    }

    #[test]
    fn a_same_net_rule_wins_immediately_and_stops_the_scan() {
        let mut first = plain(10, (10, 10));
        first.same_net = true;
        // ⚠️ The larger rule after it never gets a look in.
        let s = class_cut_spacing((0, 0, 18, 24), &[first, plain(99, (99, 99))]);
        assert_eq!(s, Some((10, 10)));
    }

    #[test]
    fn without_a_same_net_rule_the_largest_wins() {
        let s = class_cut_spacing((0, 0, 18, 24), &[plain(10, (10, 12)), plain(30, (30, 8))]);
        assert_eq!(s, Some((30, 12)), "per axis, not per rule");
    }

    #[test]
    fn a_square_cut_uses_the_max_spacing_and_a_rectangular_one_the_sided_values() {
        let r = plain(50, (7, 9));
        assert_eq!(class_cut_spacing((0, 0, 18, 18), &[r]), Some((50, 50)));
        assert_eq!(class_cut_spacing((0, 0, 18, 24), &[r]), Some((7, 9)));
    }

    #[test]
    fn a_centre_to_centre_rule_has_the_cut_taken_off_it() {
        let mut r = plain(0, (60, 70));
        r.centre_to_centre = true;
        // ⚠️ Centre-to-centre is a pitch, not a spacing: 60 - 18 and 70 - 24.
        assert_eq!(class_cut_spacing((0, 0, 18, 24), &[r]), Some((42, 46)));
        // Centre-and-edge is already a spacing even when centre-to-centre is also set.
        r.centre_and_edge = true;
        assert_eq!(class_cut_spacing((0, 0, 18, 24), &[r]), Some((60, 70)));
    }

    #[test]
    fn the_pitch_comes_from_the_whole_cut_outline_not_one_cut() {
        // 🔑 A three-cut via: each cut is 18 wide but they span 90 together. The generator asks
        // how many of THAT fit, so the pitch is 90 + spacing, not 18 + spacing.
        let g = geometry(&three_cuts(), "M1", &cutty).unwrap();
        assert_eq!(g.cut_extent, (-45, -9, 45, 9));
        assert_eq!(base_cut_pitch(g.cut_extent, 18, None), Some((108, 36)));
        // ⚠️ And the spacing the built via carries subtracts a SINGLE cut from that pitch.
        assert_eq!(cut_spacing(g.single_cut, 36, 108), (90, 18));
    }

    #[test]
    fn a_cut_class_rule_overrides_the_layer_but_only_with_both_axes() {
        let cut = (0, 0, 18, 18);
        assert_eq!(base_cut_pitch(cut, 18, Some((40, 50))), Some((58, 68)));
        // ⚠️ One axis zero leaves the layer's answer standing rather than half-applying.
        assert_eq!(base_cut_pitch(cut, 18, Some((40, 0))), Some((36, 36)));
        assert_eq!(base_cut_pitch(cut, 18, Some((0, 50))), Some((36, 36)));
    }

    #[test]
    fn a_technology_stating_no_spacing_states_no_pitch() {
        // Not an error: the connect may carry `-cut_pitch`, which overrides all of this.
        assert_eq!(base_cut_pitch((0, 0, 18, 18), 0, None), None);
        assert_eq!(
            base_cut_pitch((0, 0, 18, 18), 0, Some((40, 50))),
            Some((58, 68))
        );
    }

    #[test]
    fn spacing_is_the_pitch_less_the_cut() {
        // ⚠️ A via carries spacings, not pitches.
        assert_eq!(cut_spacing((0, 0, 18, 18), 288, 288), (270, 270));
    }
}
