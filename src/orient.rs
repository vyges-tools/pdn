// SPDX-License-Identifier: Apache-2.0
//! Placing a master's geometry where its instance sits — `odb::dbTransform`.
//!
//! A master's obstructions and pins are stated in the master's own coordinates, with its lower-left
//! at the origin. An instance names an orientation and an origin, and everything the master
//! declares has to be rotated or mirrored about that origin and then moved.
//!
//! Nothing here touches a database.

use crate::Rect;

/// **O1** — a master rectangle placed at its instance.
///
/// 🔑 **Rotate about the ORIGIN first, then translate.** `dbTransform::apply` does exactly that, and
/// the two do not commute: translating first rotates the placement as well as the shape, which puts
/// every mirrored macro somewhere else entirely.
///
/// ⚠️ **The corners swap under rotation, so the result must be re-ordered.** Negating a coordinate
/// turns a `min` into a `max`; a rect built from the transformed corners without sorting them comes
/// out inverted, and an inverted rect intersects nothing — which reads as "the obstruction is not
/// there" rather than as a geometry fault.
///
/// Both spellings are accepted: odb's own `R0`/`R90`/`MX`/`MY`… and DEF's `N`/`W`/`FS`/`FN`…, which
/// name the same eight orientations. ⚠️ An unrecognised name is treated as `R0` rather than being
/// dropped — an obstruction in the wrong place is a defect, but an obstruction that vanishes is a
/// short.
pub fn transform_rect(r: Rect, orient: &str, offset: (i32, i32)) -> Rect {
    // Each arm maps the two opposite corners; the sort below puts them back in order.
    let (ax, ay, bx, by) = match orient {
        "R90" | "W" => (-r.3, r.0, -r.1, r.2),
        "R180" | "S" => (-r.2, -r.3, -r.0, -r.1),
        "R270" | "E" => (r.1, -r.2, r.3, -r.0),
        "MY" | "FN" => (-r.2, r.1, -r.0, r.3),
        "MX" | "FS" => (r.0, -r.3, r.2, -r.1),
        // 🔑 **A mirror followed by a quarter turn, and odb composes them rather than stating a
        // matrix.** `dbTransform::apply` is `p.setY(-p.y()); p.rotate90();` for MXR90 and
        // `p.setX(-p.x()); p.rotate90();` for MYR90, with `rotate90` the CCW turn
        // `x' = -y; y' = x`. Composing:
        //
        // - MXR90: (x, y) -> (x, -y) -> ( y,  x)   reflection through y = x
        // - MYR90: (x, y) -> (-x, y) -> (-y, -x)   reflection through y = -x
        //
        // ⚠️ **These two were swapped**, and nothing caught it: the rect stays well formed either
        // way and only one design in the suite places a cell at MXR90 — the west pads, whose pins
        // then landed mirrored inside their own pad, never reached the core-facing edge, and left
        // that entire edge unconnected.
        "MXR90" | "FW" => (r.1, r.0, r.3, r.2),
        "MYR90" | "FE" => (-r.3, -r.2, -r.1, -r.0),
        // "R0" | "N" and anything unknown
        _ => (r.0, r.1, r.2, r.3),
    };
    (
        ax.min(bx) + offset.0,
        ay.min(by) + offset.1,
        ax.max(bx) + offset.0,
        ay.max(by) + offset.1,
    )
}

/// **O2** — a master rectangle placed at an instance whose BOUNDING BOX starts at `at`.
///
/// 🔑 **`dbInst::setLocationOrient` keeps the bounding box where the location put it.** Rotating a
/// master about its own origin moves its content off the origin — a mirror in y sends everything
/// negative — so the transformed geometry has to be shifted back until the transformed master
/// outline starts at the placement point again.
///
/// ⚠️ **Without that shift, every flipped row places its cell in the row BELOW.** Rows alternate
/// orientation, so half the cells land on top of the other half: identical shapes, identical vias,
/// and the overlap check then discards both — each being "no larger than" the other. The symptom is
/// exactly half the expected vias, which reads like a filter rejecting every other candidate.
///
/// `master` is the master's own width and height.
pub fn place_in_bbox(r: Rect, orient: &str, master: (i32, i32), at: (i32, i32)) -> Rect {
    let outline = transform_rect((0, 0, master.0, master.1), orient, (0, 0));
    let shift = (at.0 - outline.0, at.1 - outline.1);
    transform_rect(r, orient, shift)
}

/// **O3** — an orientation under odb's own spelling.
///
/// `get_orientations` accepts either naming and rewrites DEF's to odb's before comparing, and the
/// comparison that follows is an **exact** string match against `dbInst::getOrient()`. ⚠️ So the two
/// spellings are not interchangeable at the point of use — only here.
pub fn canonical(orient: &str) -> &str {
    match orient {
        "N" => "R0",
        "W" => "R90",
        "S" => "R180",
        "E" => "R270",
        "FN" => "MY",
        "FS" => "MX",
        "FE" => "MYR90",
        "FW" => "MXR90",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quarter_turn_mirrors_compose_as_odb_composes_them() {
        // Derived from dbTransform::apply, not from a matrix: mirror, then the CCW rotate90
        // `x' = -y; y' = x`.
        let r: Rect = (10, 20, 30, 50);
        // MXR90 = mirror in y, then turn = reflection through y = x.
        assert_eq!(transform_rect(r, "MXR90", (0, 0)), (20, 10, 50, 30));
        // MYR90 = mirror in x, then turn = reflection through y = -x.
        assert_eq!(transform_rect(r, "MYR90", (0, 0)), (-50, -30, -20, -10));
    }

    #[test]
    fn the_two_quarter_turn_mirrors_are_not_the_same_map() {
        // ⚠️ Swapping them keeps every rectangle well formed, which is why it went unnoticed.
        let r: Rect = (10, 20, 30, 50);
        assert_ne!(
            transform_rect(r, "MXR90", (0, 0)),
            transform_rect(r, "MYR90", (0, 0))
        );
    }

    #[test]
    fn the_def_spellings_canonicalise_to_odbs() {
        for (def, odb) in [
            ("N", "R0"),
            ("W", "R90"),
            ("S", "R180"),
            ("E", "R270"),
            ("FN", "MY"),
            ("FS", "MX"),
            ("FE", "MYR90"),
            ("FW", "MXR90"),
        ] {
            assert_eq!(canonical(def), odb);
            assert_eq!(canonical(odb), odb, "already canonical");
        }
    }

    #[test]
    fn a_mirrored_master_is_shifted_back_into_its_own_outline() {
        // 🔑 A cell 100 x 200 with a pin at y 20..40. Mirrored in y the pin would sit at -40..-20;
        // placed at y = 1000 its bbox must run 1000..1200 and the pin 1160..1180.
        let pin = (10, 20, 90, 40);
        assert_eq!(
            place_in_bbox(pin, "MX", (100, 200), (0, 1000)),
            (10, 1160, 90, 1180)
        );
    }

    #[test]
    fn an_unrotated_master_is_placed_exactly_where_it_is_put() {
        let pin = (10, 20, 90, 40);
        assert_eq!(
            place_in_bbox(pin, "R0", (100, 200), (500, 1000)),
            (510, 1020, 590, 1040)
        );
    }

    #[test]
    fn every_orientation_keeps_the_cell_inside_its_own_outline() {
        let pin = (10, 20, 90, 40);
        for o in ["R0", "R90", "R180", "R270", "MY", "MX", "MXR90", "MYR90"] {
            let outline = place_in_bbox((0, 0, 100, 200), o, (100, 200), (300, 700));
            let placed = place_in_bbox(pin, o, (100, 200), (300, 700));
            assert_eq!((outline.0, outline.1), (300, 700), "{o} outline moved");
            assert!(
                placed.0 >= outline.0
                    && placed.1 >= outline.1
                    && placed.2 <= outline.2
                    && placed.3 <= outline.3,
                "{o}: pin {placed:?} escaped outline {outline:?}"
            );
        }
    }

    const R: Rect = (10, 20, 40, 60);

    #[test]
    fn an_unrotated_master_is_only_moved() {
        assert_eq!(transform_rect(R, "R0", (1000, 2000)), (1010, 2020, 1040, 2060));
        assert_eq!(transform_rect(R, "N", (0, 0)), R);
    }

    #[test]
    fn a_quarter_turn_swaps_the_extents() {
        // 30 x 40 becomes 40 x 30.
        let t = transform_rect(R, "R90", (0, 0));
        assert_eq!(t, (-60, 10, -20, 40));
        assert_eq!((t.2 - t.0, t.3 - t.1), (40, 30));
    }

    #[test]
    fn every_orientation_keeps_the_rectangle_the_right_way_round() {
        // ⚠️ The guard against an inverted rect: negation alone would produce several.
        for o in [
            "R0", "R90", "R180", "R270", "MY", "MX", "MXR90", "MYR90",
        ] {
            let t = transform_rect(R, o, (500, 500));
            assert!(t.0 < t.2 && t.1 < t.3, "{o} came out inverted: {t:?}");
            let (w, h) = (t.2 - t.0, t.3 - t.1);
            assert!(
                (w, h) == (30, 40) || (w, h) == (40, 30),
                "{o} changed the size to {w}x{h}"
            );
        }
    }

    #[test]
    fn the_def_spellings_name_the_same_transforms() {
        for (odb, def) in [
            ("R0", "N"),
            ("R90", "W"),
            ("R180", "S"),
            ("R270", "E"),
            ("MY", "FN"),
            ("MX", "FS"),
            ("MYR90", "FE"),
            ("MXR90", "FW"),
        ] {
            assert_eq!(
                transform_rect(R, odb, (7, 9)),
                transform_rect(R, def, (7, 9)),
                "{odb} and {def} disagree"
            );
        }
    }

    #[test]
    fn rotation_happens_before_the_move() {
        // 🔑 The order that matters. Rotating first puts the shape at -60..-20 and then moves it to
        // 940..980; moving first would rotate the offset too and land somewhere else.
        assert_eq!(transform_rect(R, "R90", (1000, 0)), (940, 10, 980, 40));
    }
}
