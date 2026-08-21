// SPDX-License-Identifier: Apache-2.0
//! Trimming — pulling each shape back to what actually connects to it, and removing what does not.
//!
//! This runs after the vias, and it must: a shape is trimmed to the extent of the things attached
//! to it, and a shape with nothing attached is deleted. Nothing here can be decided before the
//! connections exist.
//!
//! Nothing here touches a database.

use crate::{Direction, Rect};

/// **T1** — the extent of everything attached to a shape.
///
/// The bounding box of its via areas and its terminal connections. ⚠️ **`None` where nothing is
/// attached** — the reference starts from an *inverted* rectangle and never merges into it, and the
/// inversion is then tested for. A shape with no connections has no minimum rect, which is not the
/// same as having an empty one at the origin.
pub fn minimum_rect(connections: &[Rect]) -> Option<Rect> {
    connections
        .iter()
        .copied()
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// **T5** — a follow pin's minimum rect, which is not the general one.
///
/// 🔑 **A follow pin is held up by the ROWS it serves, not by what connects to it.** Start from the
/// general minimum rect, put the shape's own thickness back, then extend along the running axis to
/// span every row the shape covers.
///
/// ⚠️ That is why a design with no vias at all keeps every one of its follow pins: the rows hold
/// them. Treating a follow pin like any other shape deletes the entire rail network of a grid whose
/// straps were never connected — which is exactly what happens if this override is missed.
///
/// ⚠️ **The thickness is copied back from the shape**, so a follow pin is never thinned even when
/// the things attached to it are narrower. Only its length is ever trimmed.
///
/// ⚠️ **`horizontal` is the SHAPE's orientation and never the layer's.** The reference asks
/// `isHorizontal()`, which is `dx > dy` on the rect itself, and consults the layer only for a
/// square. Passing the layer's preferred direction turns this inside out for any follow pin whose
/// layer runs the other way: the length is copied back as the thickness and the rows are merged
/// across the width, so a 340-wide rail on vertical metal2 comes out 5600 wide.
pub fn followpin_minimum_rect(
    shape: Rect,
    attached: Option<Rect>,
    rows: &[Rect],
    horizontal: bool,
) -> Rect {
    let mut r = attached.unwrap_or(shape);
    // Put the shape's own thickness back on the across axis.
    if horizontal {
        r = (r.0, shape.1, r.2, shape.3);
    } else {
        r = (shape.0, r.1, shape.2, r.3);
    }
    if attached.is_none() {
        // With nothing attached the length starts from the rows alone, not from the whole shape.
        if let Some(first) = rows.first() {
            r = if horizontal {
                (first.0, r.1, first.2, r.3)
            } else {
                (r.0, first.1, r.2, first.3)
            };
        }
    }
    for row in rows {
        r = if horizontal {
            (r.0.min(row.0), r.1, r.2.max(row.2), r.3)
        } else {
            (r.0, r.1.min(row.1), r.2, r.3.max(row.3))
        };
    }
    r
}

/// **T6** — the block terminals a shape gains by reaching the die edge.
///
/// 🔑 **A shape whose edge sits on the die boundary becomes a PIN there**, and the sliver it gains
/// counts as a connection — so the shape is held out to that edge and trimming leaves it alone.
/// This is why a ring extended to the boundary keeps its full length while an unextended one is
/// pulled back to its vias: the extended one is pinned at both ends.
///
/// ⚠️ **Equality with the die edge, not proximity.** A shape ending one unit short of the boundary
/// gains nothing. Each of the four sides is tested on its own, so a shape can be pinned at one end
/// and free at the other.
///
/// Each sliver is `min_width` deep and is **NOT clamped to the shape**.
///
/// 🔑 **That is the point, and it is what removes an unheld offcut.** The reference computes this
/// once, on the shape as first added, and a piece cut from that shape *inherits* the rect without
/// re-clipping — so an offcut against the die edge carries a pin that sticks out of it. Trimming
/// then finds its minimum rect is not contained by the shape, cannot shift it inside, leaves no
/// replacement, and removes it for having one connection where two are needed.
///
/// ⚠️ Clamped to the shape instead, the minimum rect equals the offcut exactly, trimming returns
/// "no change", and a 460-unit sliver of metal survives against the die edge with nothing holding
/// it. Recomputing per piece and not clamping reproduces the inherited value for the offcut and is
/// identical for a shape that was never cut.
pub fn boundary_pins(shape: Rect, die: Rect, min_width: i32) -> Vec<Rect> {
    let mut out = Vec::new();
    if shape.0 == die.0 {
        out.push((shape.0, shape.1, die.0 + min_width, shape.3));
    }
    if shape.2 == die.2 {
        out.push((die.2 - min_width, shape.1, shape.2, shape.3));
    }
    if shape.1 == die.1 {
        out.push((shape.0, shape.1, shape.2, die.1 + min_width));
    }
    if shape.3 == die.3 {
        out.push((shape.0, die.3 - min_width, shape.2, shape.3));
    }
    out
}

/// **T2** — grow a rectangle until it meets the layer's minimum area.
///
/// ⚠️ **The largest of the area rules wins**, not the smallest — the most demanding one governs.
/// And where LEF58 area rules exist the layer's own `AREA` is **ignored entirely** rather than
/// combined with them.
///
/// The growth is along the layer's own direction: a horizontal layer gets wider, a vertical one
/// taller. ⚠️ **Each side grows by `ceil(added / 2)`**, so an odd shortfall over-grows rather than
/// under-grows, and the two edges then snap in **opposite** directions — the low edge down, the
/// high edge up — so the result can only get larger.
pub fn adjust_to_min_area(
    rect: Rect,
    min_area: i64,
    direction: Direction,
    manufacturing_grid: i32,
) -> Rect {
    if min_area == 0 {
        return rect;
    }
    let (w, h) = ((rect.2 - rect.0) as i64, (rect.3 - rect.1) as i64);
    if w * h >= min_area {
        return rect;
    }
    let snap = crate::straps::snap_to_manufacturing_grid;
    if direction == Direction::Horizontal {
        if h == 0 {
            return rect;
        }
        let required = ((min_area + h - 1) / h) as i32;
        // ⚠️ Rounded UP, so an odd shortfall over-grows rather than under-grows.
        let adjust = (required - w as i32 + 1) / 2;
        (
            snap(rect.0 - adjust, manufacturing_grid, false),
            rect.1,
            snap(rect.2 + adjust, manufacturing_grid, true),
            rect.3,
        )
    } else {
        if w == 0 {
            return rect;
        }
        let required = ((min_area + w - 1) / w) as i32;
        let adjust = (required - h as i32 + 1) / 2;
        (
            rect.0,
            snap(rect.1 - adjust, manufacturing_grid, false),
            rect.2,
            snap(rect.3 + adjust, manufacturing_grid, true),
        )
    }
}

/// What trimming decides about one shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Left exactly as it is.
    Keep,
    /// Pulled back to this rectangle.
    Replace(Rect),
    /// Nothing holds it up.
    Remove,
}

/// **T3** — shift a rectangle back inside its shape.
///
/// ⚠️ **By the SMALLER of the two edge deltas**, so the rectangle moves the least distance that
/// puts it back inside — not to whichever edge it overran. Along the shape's own direction: a
/// horizontal shape shifts in x.
pub fn shift_inside(shape: Rect, want: Rect, direction: Direction) -> Rect {
    if direction == Direction::Horizontal {
        let (d0, d1) = (shape.0 - want.0, shape.2 - want.2);
        let d = if d0.abs() < d1.abs() { d0 } else { d1 };
        (want.0 + d, want.1, want.2 + d, want.3)
    } else {
        let (d0, d1) = (shape.1 - want.1, shape.3 - want.3);
        let d = if d0.abs() < d1.abs() { d0 } else { d1 };
        (want.0, want.1 + d, want.2, want.3 + d)
    }
}

/// **T4** — what becomes of one shape.
///
/// ⚠️ **A shape with nothing attached is REMOVED**, not left alone. That is the whole reason an
/// unconnected strap disappears from the output while looking perfectly well-formed in the
/// generator — and why comparing a generated grid against a trimmed run shows straps that were
/// never wrong, only unheld.
///
/// ⚠️ **A shape whose vias all coincide exactly with its minimum rect is also removed.** It exists
/// only to carry a via stack and nothing else holds it up; trimming it to itself would keep metal
/// that serves no purpose. The test is equality with *every* via, not with any — and it is
/// vacuously true for a shape with **no** vias, which is how an offcut held by nothing but its own
/// boundary pin comes to be removed.
///
/// ⚠️ **`via_areas` is VIAS, not everything holding the shape up.** The reference asks
/// `shape->getVias()`; a bterm connection counts toward the connection *count* that decides
/// removability, never toward this test.
///
/// ⚠️ **A pin layer is never modified**, only removed — a pin's shape is its contract with whatever
/// connects from outside.
pub fn decide(
    shape: Rect,
    min_rect: Option<Rect>,
    via_areas: &[Rect],
    min_area: i64,
    direction: Direction,
    manufacturing_grid: i32,
    is_pin_layer: bool,
    removable: bool,
) -> Decision {
    let Some(min) = min_rect else {
        // Nothing is attached, so nothing holds the shape up.
        return if removable {
            Decision::Remove
        } else {
            Decision::Keep
        };
    };

    let mut want = adjust_to_min_area(min, min_area, direction, manufacturing_grid);
    if !contains(shape, want) {
        want = shift_inside(shape, want, direction);
    }
    if want == shape {
        return Decision::Keep;
    }

    // ⚠️ **TRUE when there are no vias at all**, which is the case that matters. The reference
    // opens `bool effectively_vias_stack = true;` and only a via whose area differs from the
    // minimum rect falsifies it — a shape with no vias never enters that loop, so the flag stands
    // and the shape drops to the removal test. Requiring a non-empty list inverts exactly the
    // case that decides whether an unheld offcut survives.
    let only_vias = via_areas.iter().all(|v| *v == min);
    if !only_vias && contains(shape, want) {
        if is_pin_layer {
            return Decision::Keep; // its shape is a contract; leave it
        }
        return Decision::Replace(want);
    }
    if removable {
        Decision::Remove
    } else {
        Decision::Keep
    }
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer.0 <= inner.0 && outer.1 <= inner.1 && outer.2 >= inner.2 && outer.3 >= inner.3
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the minimum rect ─────────────────────────────────────────────────────────────────────

    #[test]
    fn the_minimum_rect_spans_everything_attached() {
        let got = minimum_rect(&[(10, 10, 20, 20), (50, 5, 60, 30)]);
        assert_eq!(got, Some((10, 5, 60, 30)));
    }

    #[test]
    fn nothing_attached_gives_no_rect_rather_than_an_empty_one() {
        // ⚠️ Not `Some((0,0,0,0))`: a rect at the origin would be *inside* most shapes and would
        // trim them to nothing instead of removing them.
        assert_eq!(minimum_rect(&[]), None);
    }

    // ── minimum area ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_shape_already_meeting_the_minimum_is_untouched() {
        let r = (0, 0, 100, 100);
        assert_eq!(adjust_to_min_area(r, 1000, Direction::Horizontal, 1), r);
    }

    #[test]
    fn no_minimum_area_leaves_the_shape_alone() {
        let r = (0, 0, 10, 10);
        assert_eq!(adjust_to_min_area(r, 0, Direction::Horizontal, 1), r);
    }

    #[test]
    fn a_horizontal_shape_grows_wider_and_a_vertical_one_taller() {
        let r = (0, 0, 10, 10); // area 100, wants 200
        let h = adjust_to_min_area(r, 200, Direction::Horizontal, 1);
        assert_eq!((h.1, h.3), (0, 10), "height untouched");
        assert!(h.2 - h.0 >= 20, "wide enough for the area");
        let v = adjust_to_min_area(r, 200, Direction::Vertical, 1);
        assert_eq!((v.0, v.2), (0, 10), "width untouched");
        assert!(v.3 - v.1 >= 20);
    }

    #[test]
    fn the_growth_is_split_between_the_sides_rounding_up() {
        // ⚠️ 10x10 wanting 250 needs width 25, so 15 more, so 8 on each side — not 7.
        let got = adjust_to_min_area((0, 0, 10, 10), 250, Direction::Horizontal, 1);
        assert_eq!(got, (-8, 0, 18, 10));
    }

    #[test]
    fn an_undirected_layer_grows_taller_like_a_vertical_one() {
        let got = adjust_to_min_area((0, 0, 10, 10), 200, Direction::None, 1);
        assert_eq!((got.0, got.2), (0, 10), "the x extent is untouched");
    }

    #[test]
    fn a_degenerate_shape_is_left_alone_rather_than_dividing_by_zero() {
        let flat = (0, 5, 100, 5);
        assert_eq!(
            adjust_to_min_area(flat, 1000, Direction::Horizontal, 1),
            flat
        );
    }

    // ── shifting back inside ─────────────────────────────────────────────────────────────────

    #[test]
    fn a_rectangle_moves_the_shorter_way_to_get_back_inside() {
        // ⚠️ Overhanging the low edge by 5 and inside the high edge by 80: it moves by 5.
        let got = shift_inside((0, 0, 100, 10), (-5, 0, 20, 10), Direction::Horizontal);
        assert_eq!(got, (0, 0, 25, 10));
    }

    #[test]
    fn a_vertical_shape_shifts_in_y() {
        let got = shift_inside((0, 0, 10, 100), (0, -5, 10, 20), Direction::Vertical);
        assert_eq!(got, (0, 0, 10, 25));
    }

    // ── follow pins ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_follow_pin_is_held_up_by_its_rows_with_nothing_attached() {
        // 🔑 The rule that keeps every rail in a design with no vias at all. Missing it deletes
        // the whole rail network of an unconnected grid.
        let got = followpin_minimum_rect(
            (0, 100, 1000, 140),
            None,
            &[(200, 100, 400, 140), (600, 100, 800, 140)],
            true,
        );
        assert_eq!(
            got,
            (200, 100, 800, 140),
            "spans the rows, keeps its own thickness"
        );
    }

    #[test]
    fn a_follow_pin_keeps_its_thickness_however_narrow_its_connections() {
        // ⚠️ Only the length is ever trimmed.
        let got =
            followpin_minimum_rect((0, 100, 1000, 140), Some((300, 118, 320, 122)), &[], true);
        assert_eq!(got, (300, 100, 320, 140));
    }

    #[test]
    fn a_follow_pin_spans_both_its_connections_and_its_rows() {
        let got = followpin_minimum_rect(
            (0, 100, 1000, 140),
            Some((500, 118, 520, 122)),
            &[(200, 100, 400, 140)],
            true,
        );
        assert_eq!(got, (200, 100, 520, 140));
    }

    #[test]
    fn a_vertical_follow_pin_works_the_other_way_round() {
        let got = followpin_minimum_rect((100, 0, 140, 1000), None, &[(100, 200, 140, 400)], false);
        assert_eq!(got, (100, 200, 140, 400));
    }

    // ── boundary pins ────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_shape_reaching_the_die_edge_is_pinned_there() {
        // 🔑 Which is why a ring extended to the boundary keeps its full length: it is pinned at
        // both ends and its minimum rect therefore spans everything.
        let got = boundary_pins((0, 100, 1000, 140), (0, 0, 1000, 1000), 20);
        assert_eq!(got.len(), 2, "pinned at both x edges");
        assert_eq!(got[0], (0, 100, 20, 140));
        assert_eq!(got[1], (980, 100, 1000, 140));
    }

    #[test]
    fn a_shape_one_unit_short_of_the_edge_gains_nothing() {
        // ⚠️ Equality, not proximity — the same identity rule the row extension uses.
        assert!(boundary_pins((1, 100, 999, 140), (0, 0, 1000, 1000), 20).is_empty());
    }

    #[test]
    fn each_side_is_tested_on_its_own() {
        let got = boundary_pins((0, 100, 999, 140), (0, 0, 1000, 1000), 20);
        assert_eq!(got.len(), 1, "pinned at the low edge only");
        assert_eq!(got[0], (0, 100, 20, 140));
    }

    #[test]
    fn a_sliver_on_a_shape_narrower_than_it_overhangs_that_shape() {
        // 🔑 Not clamped, and that is what removes an unheld offcut. The reference computes this
        // on the shape as first added and a cut piece inherits it, so a sliver against the die
        // edge carries a pin wider than itself — the minimum rect is then not contained by the
        // shape, no replacement is made, and the piece is removed for want of a second connection.
        let got = boundary_pins((0, 100, 5, 140), (0, 0, 1000, 1000), 20);
        assert_eq!(got[0], (0, 100, 20, 140), "20 deep, overhanging the shape");
    }

    #[test]
    fn an_offcut_at_the_die_edge_is_removed_rather_than_left_standing() {
        // A metal7 sliver in miniature: 460 wide against
        // a die edge, holding one boundary pin and no via at all.
        let die = (0, 0, 200260, 201600);
        let offcut = (199800, 31300, 200260, 32700);
        let pins = boundary_pins(offcut, die, 800);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].0, 199460, "the pin reaches past the offcut");
        let min = minimum_rect(&pins);
        assert_eq!(
            decide(offcut, min, &[], 0, Direction::Horizontal, 1, false, true),
            Decision::Remove
        );
    }

    // ── the decision ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_shape_with_nothing_attached_is_removed() {
        // 🔑 This is why an unconnected strap disappears from the output while being perfectly
        // well-formed in the generator.
        let d = decide(
            (0, 0, 100, 10),
            None,
            &[],
            0,
            Direction::Horizontal,
            1,
            false,
            true,
        );
        assert_eq!(d, Decision::Remove);
    }

    #[test]
    fn an_unremovable_shape_with_nothing_attached_stays() {
        let d = decide(
            (0, 0, 100, 10),
            None,
            &[],
            0,
            Direction::Horizontal,
            1,
            false,
            false,
        );
        assert_eq!(d, Decision::Keep);
    }

    #[test]
    fn a_shape_is_pulled_back_to_what_holds_it() {
        let d = decide(
            (0, 0, 100, 10),
            Some((20, 0, 40, 10)),
            &[(20, 0, 40, 10), (30, 0, 35, 10)],
            0,
            Direction::Horizontal,
            1,
            false,
            true,
        );
        assert_eq!(d, Decision::Replace((20, 0, 40, 10)));
    }

    #[test]
    fn a_shape_already_at_its_minimum_rect_is_kept_unchanged() {
        let r = (20, 0, 40, 10);
        let d = decide(
            r,
            Some(r),
            &[(20, 0, 40, 10), (25, 0, 30, 10)],
            0,
            Direction::Horizontal,
            1,
            false,
            true,
        );
        assert_eq!(d, Decision::Keep, "no change means no replacement");
    }

    #[test]
    fn a_shape_held_up_only_by_a_via_stack_is_removed() {
        // ⚠️ Every via coincides with the minimum rect, so nothing else holds the shape and the
        // metal serves no purpose. The test is equality with EVERY via, not with any.
        let d = decide(
            (0, 0, 100, 10),
            Some((20, 0, 40, 10)),
            &[(20, 0, 40, 10)],
            0,
            Direction::Horizontal,
            1,
            false,
            true,
        );
        assert_eq!(d, Decision::Remove);
    }

    #[test]
    fn one_via_off_the_minimum_rect_is_enough_to_keep_the_shape() {
        let d = decide(
            (0, 0, 100, 10),
            Some((20, 0, 40, 10)),
            &[(20, 0, 40, 10), (22, 0, 24, 10)],
            0,
            Direction::Horizontal,
            1,
            false,
            true,
        );
        assert_eq!(d, Decision::Replace((20, 0, 40, 10)));
    }

    #[test]
    fn a_pin_layer_is_never_modified_only_removed() {
        // ⚠️ A pin's shape is its contract with whatever connects from outside.
        let d = decide(
            (0, 0, 100, 10),
            Some((20, 0, 40, 10)),
            &[(20, 0, 40, 10), (22, 0, 24, 10)],
            0,
            Direction::Horizontal,
            1,
            true,
            true,
        );
        assert_eq!(d, Decision::Keep);
        let gone = decide(
            (0, 0, 100, 10),
            None,
            &[],
            0,
            Direction::Horizontal,
            1,
            true,
            true,
        );
        assert_eq!(gone, Decision::Remove, "but an unheld one still goes");
    }

    #[test]
    fn minimum_area_can_widen_the_trimmed_shape_beyond_its_connections() {
        // The connections span 2 units; the layer wants 100 of area on a 10-tall shape, so the
        // trimmed shape is widened to 10 rather than left at 2.
        let d = decide(
            (0, 0, 100, 10),
            Some((20, 0, 22, 10)),
            &[(20, 0, 22, 10), (21, 0, 21, 10)],
            100,
            Direction::Horizontal,
            1,
            false,
            true,
        );
        match d {
            Decision::Replace(r) => assert!(r.2 - r.0 >= 10, "widened for the minimum area: {r:?}"),
            other => panic!("expected a replacement, got {other:?}"),
        }
    }
}
