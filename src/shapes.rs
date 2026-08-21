// SPDX-License-Identifier: Apache-2.0
//! Shapes — merging them, and cutting them around obstructions.
//!
//! Nothing here touches a database.

use crate::Rect;

/// **H1** — merge one shape into another.
///
/// The union of the two bounding boxes. ⚠️ Not a geometric union: two shapes that do not touch
/// merge into the rectangle spanning both, so the caller decides *what* may merge and this only
/// says what the result is.
pub fn merge(a: Rect, b: Rect) -> Rect {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// **H2** — collapse a set of shapes that share a net and a layer.
///
/// Two rails on the shared edge of adjacent rows are the same wire written twice; the reference
/// emits one per row and merges them afterwards. ⚠️ **Identical rectangles collapse to one**, which
/// is why a design of 56 flipped rows yields 57 rails rather than 112: every interior edge is
/// written by both of the rows that meet on it.
///
/// Only exact duplicates are collapsed here. Merging *overlapping* shapes is a different question
/// and belongs with the caller that knows whether they may legally join.
pub fn dedupe(shapes: &[Rect]) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::with_capacity(shapes.len());
    for s in shapes {
        if !out.contains(s) {
            out.push(*s);
        }
    }
    out
}

/// **H11** — accumulate one component's shapes the way `GridComponent::addShape` does.
///
/// 🔑 **`is_x_overlap` does not mean the two overlap in x — it means their x extents are EQUAL.**
///
/// So two shapes merge only when they lie exactly on top of one another along one whole axis, and
/// ⚠️ **which axis does not matter** — no direction is consulted.
///
/// ⚠️ **The overlap is INTERIOR.** The reference queries with `intersects` and then discards
/// anything that only touches, so shapes meeting end to end are left as two.
///
/// Three outcomes, and two of them lose the shape:
///
/// - it interior-overlaps a shape of another net → a short, and the shape is **dropped**;
/// - it interior-overlaps a same-net shape aligned on neither axis → **dropped**;
/// - otherwise every same-net shape it overlaps is absorbed into it.
///
/// 🔑 **This is what makes a follow pin span a macro.** Rows are split around macros, but the row
/// below the macro's bottom edge is not — so the rail on their shared edge is written once at full
/// width and once at the split width, on the same y, and the two merge. Keeping only exact
/// duplicates leaves the short one, which then has nothing above it and reads as a channel to
/// repair — spurious channels, from a rail that was only ever half of one.
///
/// ⚠️ **Matches are found against the INCOMING rect, not the growing one.** The reference collects
/// its intersections from one query and then merges them all; a merge that brings a further shape
/// within reach does not pull it in until something else overlaps it.
pub fn add_shapes<T: PartialEq + Clone>(shapes: &[(T, Rect)]) -> Vec<(T, Rect)> {
    let mut kept: Vec<(T, Rect)> = Vec::with_capacity(shapes.len());
    'next: for (net, rect) in shapes {
        let mut absorb: Vec<usize> = Vec::new();
        for (i, (other_net, other)) in kept.iter().enumerate() {
            if !overlaps(*rect, *other) {
                continue;
            }
            if other_net != net {
                continue 'next; // a short: the shape is not added at all
            }
            let same_x = rect.0 == other.0 && rect.2 == other.2;
            let same_y = rect.1 == other.1 && rect.3 == other.3;
            if !same_x && !same_y {
                continue 'next; // cannot be merged, so it is discarded
            }
            absorb.push(i);
        }
        let mut merged = *rect;
        for i in absorb.iter().rev() {
            merged = merge(merged, kept.remove(*i).1);
        }
        kept.push((net.clone(), merged));
    }
    kept
}

/// **H5** — widen a shape by the via metal that lands on it.
///
/// 🔑 **A via's metal patch is sized by the SHAPES IT JOINS, not by their intersection.** A via
/// between a follow pin and a strap that overhangs the core puts metal on the follow-pin layer
/// spanning the strap's whole width — past the follow pin's own end — and that patch is added as a
/// shape and merged in, widening the follow pin.
///
/// ⚠️ **Merging requires the two to align EXACTLY on one axis, and it does not matter WHICH.**
/// `GridComponent::addShape` tests `is_x_overlap || is_y_overlap` and consults no direction at
/// all. Deciding the axis from the layer's preferred direction is wrong for any shape that does
/// not run along it — a follow pin on a vertical layer runs horizontally like every other follow
/// pin, and testing it against the layer merged the two rails of a row into one 2970-wide shape.
///
/// ⚠️ **The overlap is INTERIOR**, as `Rect::overlaps` is: the reference queries an R-tree with
/// `intersects` and then discards anything that only touches. A patch abutting the end of a shape
/// is not merged into it.
pub fn widen_by_via_metal(shape: Rect, patches: &[Rect]) -> Rect {
    let mut out = shape;
    for p in patches {
        let aligned = (p.1 == shape.1 && p.3 == shape.3) || (p.0 == shape.0 && p.2 == shape.2);
        if aligned && overlaps(out, *p) {
            out = merge(out, *p);
        }
    }
    out
}

/// Interior overlap — a shared edge does NOT count, as odb's `Rect::overlaps` has it.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// Closed intersection — a shared edge counts, as odb's `Rect::intersects` does.
fn intersects(a: Rect, b: Rect) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// **H3** — cut a shape around the obstructions crossing it.
///
/// 🔑 **This is a one-dimensional subtraction, and that is not a shortcut.** The reference builds a
/// Boost.Polygon 90-degree set and subtracts polygons from it — but before doing so it *extends
/// every violation to span the shape's whole across extent*, with the comment "ensure the violation
/// overlap fully with the shape to make cut correctly". Every cut is therefore full-width, the
/// pieces are full-width, and the width filter that follows (`accept` only where the piece is as
/// wide as the original) passes for all of them. What is left is interval subtraction along the
/// length.
///
/// ⚠️ **A piece narrower than the original is discarded, not kept.** That is what the width filter
/// does, and it is the reason a partial-height overlap removes a stretch of strap outright rather
/// than thinning it.
///
/// `blocked` are the obstruction extents **along the shape's length**, in any order.
/// Returns `None` when nothing crosses the shape, matching the reference's "no replacements".
pub fn cut(length: (i32, i32), blocked: &[(i32, i32)]) -> Option<Vec<(i32, i32)>> {
    let crossing: Vec<(i32, i32)> = blocked
        .iter()
        .filter(|(lo, hi)| *hi > length.0 && *lo < length.1)
        .copied()
        .collect();
    if crossing.is_empty() {
        return None;
    }

    let mut cuts = crossing;
    cuts.sort_unstable();

    let mut out = Vec::new();
    let mut at = length.0;
    for (lo, hi) in cuts {
        if lo > at {
            out.push((at, lo.min(length.1)));
        }
        at = at.max(hi);
        if at >= length.1 {
            break;
        }
    }
    if at < length.1 {
        out.push((at, length.1));
    }
    out.retain(|(a, b)| b > a);
    Some(out)
}

/// The four per-side distances between a shape and the obstruction it generates.
///
/// ⚠️ **Per side, not one number.** `generateObstruction` merges three rules — plain spacing, the
/// spacing tables and the end-of-line rules — and the last of those grows only the two ends, so a
/// shape's halo is routinely wider along its length than across it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Halo {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// **H9** — the halo a shape's obstruction stands off by, side by side.
///
/// `Shape::getObstructionHalo`, read straight off the two rects.
pub fn obstruction_halo(rect: Rect, obstruction: Rect) -> Halo {
    Halo {
        left: rect.0 - obstruction.0,
        top: obstruction.3 - rect.3,
        right: obstruction.2 - rect.2,
        bottom: rect.1 - obstruction.1,
    }
}

/// **H10** — how far a cut must clear an obstruction: the LARGER of the two shapes' halos.
///
/// 🔑 **`Shape::getRectWithLargestObstructionHalo`** — `std::max(obs.left, halo.left)` and its
/// three siblings, applied to the obstruction's **own rect**, not to its stored bloated one.
///
/// ⚠️ **Neither shape's spacing governs alone.** Two pieces of metal must be separated by whichever
/// of them asks for more, so a narrow pin crossed by a wide strap is cleared by the *strap's*
/// spacing. Using the obstruction's stored rect — which carries only its own halo — leaves the cut
/// short by exactly the difference, and a parallel-run-length table makes that difference a whole
/// table row rather than a rounding error.
pub fn rect_with_largest_halo(rect: Rect, own: Halo, other: Halo) -> Rect {
    (
        rect.0 - own.left.max(other.left),
        rect.1 - own.bottom.max(other.bottom),
        rect.2 + own.right.max(other.right),
        rect.3 + own.top.max(other.top),
    )
}

/// **H11** — the span one obstruction blocks out of a shape, or `None` where it does not reach it.
///
/// 🔑 **The two rects are used for DIFFERENT questions, and swapping them is silent.**
/// `Shape::cut` queries the tree by the obstruction's **stored** rect — bloated when it was made,
/// each by its own spacing — and then measures the cut extent from the obstruction's **raw** rect
/// grown by the larger of the two halos.
///
/// ⚠️ **Reach is asked ACROSS, extent is taken ALONG.** An obstruction only cuts a shape it
/// actually crosses; filtering by layer alone cuts every shape on the layer at the obstruction's
/// along-extent, wherever it sits.
pub fn blocked_span(
    rect: Rect,
    halo: Halo,
    obstruction_stored: Rect,
    obstruction_raw: Rect,
    horizontal: bool,
) -> Option<(i32, i32)> {
    // The shape's own reach, across its direction, is what decides whether the two meet at all.
    let across = if horizontal {
        (rect.1 - halo.bottom, rect.3 + halo.top)
    } else {
        (rect.0 - halo.left, rect.2 + halo.right)
    };
    let (lo, hi) = if horizontal {
        (obstruction_stored.1, obstruction_stored.3)
    } else {
        (obstruction_stored.0, obstruction_stored.2)
    };
    if hi < across.0 || across.1 < lo {
        return None;
    }
    let cleared = rect_with_largest_halo(
        obstruction_raw,
        obstruction_halo(obstruction_raw, obstruction_stored),
        halo,
    );
    Some(if horizontal {
        (cleared.0, cleared.2)
    } else {
        (cleared.1, cleared.3)
    })
}

/// **H4** — whether an obstruction on the same net may be ignored.
///
/// ⚠️ **Only when the shape completely covers it across its width.** A same-net obstruction wholly
/// inside the strap is not a violation — the strap already carries that net there. One that pokes
/// out either side is, because the part sticking out is metal the strap does not cover.
///
/// ⚠️ This exemption does **not** apply to a plain shape: the reference tests
/// `shapeType() != kShape`, so an ordinary same-net *shape* still cuts. Only derived obstruction
/// types are forgiven.
pub fn same_net_covered(
    shape_across: (i32, i32),
    other_across: (i32, i32),
    is_plain_shape: bool,
) -> bool {
    !is_plain_shape && shape_across.0 <= other_across.0 && shape_across.1 >= other_across.1
}

/// **H6** — grow a shape until it reaches something its via already connects to.
///
/// 🔑 **This is what `Grid::repairVias` does to a strap whose via only partly overlaps it.** A via
/// joins two shapes; if one of them is shorter than the other along its own length, the shorter is
/// extended until the two meet, and every via is then rebuilt because the shapes moved.
///
/// The rule, from `Shape::extendTo`:
///
/// - the growth is along the shape's **own direction**, taken from its aspect ratio, and a square
///   shape cannot grow at all — it returns nothing rather than picking an axis;
/// - ⚠️ **both ends move**: the new span is the union of the two rects on that axis, so a shape
///   short at either end reaches out at that end;
/// - the shape stays exactly as it was across;
/// - ⚠️ the growth is **refused outright** — not clipped — if any obstruction meets the new rect,
///   or if any other shape meets the new rect grown by its own halo less one.
///
/// ⚠️ **Both checks ignore the shape's own contribution.** The reference passes the original shape
/// down and skips it by identity; here the caller must leave it out of `obstructions` and
/// `others`, or a shape will always block its own growth.
///
/// `halo` is what `generateObstruction` would add. The reference shrinks that by one before
/// testing, so a neighbour that merely touches the grown shape is allowed and one that overlaps is
/// not.
pub fn extend_to(
    shape: Rect,
    toward: Rect,
    obstructions: &[Rect],
    others: &[Rect],
    halo: i32,
) -> Option<Rect> {
    let grown = match crate::viagen::rect_direction(shape) {
        crate::Direction::Horizontal => (
            shape.0.min(toward.0),
            shape.1,
            shape.2.max(toward.2),
            shape.3,
        ),
        crate::Direction::Vertical => (
            shape.0,
            shape.1.min(toward.1),
            shape.2,
            shape.3.max(toward.3),
        ),
        crate::Direction::None => return None,
    };
    if grown == shape {
        return None;
    }
    if obstructions.iter().any(|o| intersects(grown, *o)) {
        return None;
    }
    let reach = halo - 1;
    let checked = (
        grown.0 - reach,
        grown.1 - reach,
        grown.2 + reach,
        grown.3 + reach,
    );
    if others.iter().any(|o| intersects(checked, *o)) {
        return None;
    }
    Some(grown)
}

/// What becomes of a shape and the via metal landing on it — see [`check_via_shapes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViaCheck {
    /// The metal is already inside the shape; nothing to do.
    Fits,
    /// The shape grows to cover the metal.
    Extend(Rect),
    /// These via-metal entries, by index, are ripped up.
    Ripup(Vec<usize>),
}

/// **H7** — `Via::writeToDb`'s containment check: extend the shape, or rip the via out.
///
/// 🔑 **The last stage of the pipeline mutates shapes and deletes vias**, which is easy to miss
/// when a stage called "write to db" is assumed to write. Both halves come from one function:
///
/// - take the shape's rect **merged with every via shape on its layer**;
/// - if that is no bigger, the metal already fits and nothing happens;
/// - otherwise the change is allowed only when the shape is **modifiable**, no via shape hits an
///   **obstruction**, and the growth is along the layer's **preferred direction only** — a
///   horizontal shape's y may not move, a vertical shape's x may not;
/// - allowed, **the shape is extended**. Refused, **every via shape not contained by the shape is
///   ripped up** — together with any that hit an obstruction.
///
/// 🔑 This is what puts a metal4 strap 170 units past the die: a via at the bottom rail carries
/// metal below the strap's end, the strap is vertical, so growing in y is its preferred direction
/// and the strap simply takes the metal in.
///
/// ⚠️ **The direction guard always applies here.** `allowsNonPreferredDirectionChange` is enabled
/// only by pad direct connections, and `FollowPinShape` overrides the setter to do nothing — so no
/// shape this engine builds is exempt.
///
/// ⚠️ **This is not the merge rule in [`widen_by_via_metal`].** That one is
/// `GridComponent::addShape`'s and demands an exact match on one axis. This one merges
/// unconditionally and then judges the *direction of the growth*.
pub fn check_via_shapes(
    shape: Rect,
    via_metal: &[Rect],
    layer: crate::Direction,
    modifiable: bool,
    obstructions: &[Rect],
) -> ViaCheck {
    let mut grown = shape;
    for m in via_metal {
        grown = merge(grown, *m);
    }
    if grown == shape {
        return ViaCheck::Fits;
    }
    let hits_obstruction: Vec<usize> = via_metal
        .iter()
        .enumerate()
        .filter(|(_, m)| obstructions.iter().any(|o| intersects(**m, *o)))
        .map(|(i, _)| i)
        .collect();
    let straight = match layer {
        crate::Direction::Horizontal => grown.1 == shape.1 && grown.3 == shape.3,
        crate::Direction::Vertical => grown.0 == shape.0 && grown.2 == shape.2,
        crate::Direction::None => true,
    };
    if modifiable && straight && hits_obstruction.is_empty() {
        return ViaCheck::Extend(grown);
    }
    // ⚠️ A UNION with the obstruction hits, not a replacement — a via can be ripped up for
    // either reason and the reference inserts into the same set.
    let mut ripup = hits_obstruction;
    for (i, m) in via_metal.iter().enumerate() {
        if !contains(shape, *m) && !ripup.contains(&i) {
            ripup.push(i);
        }
    }
    ripup.sort_unstable();
    ViaCheck::Ripup(ripup)
}

/// **H8** — whether ripping vias out has broken a stack, in which case all of it goes.
///
/// ⚠️ **The test is over the connect's layers, not the ripped-up ones.** What matters is whether
/// the shapes that SURVIVE still cover every layer the connect spans; a stack missing one level
/// connects nothing, so the reference takes the rest of it out too rather than leaving metal that
/// implies a connection it does not make.
pub fn stack_broken(surviving_layers: &[String], connect_layers: &[String]) -> bool {
    connect_layers
        .iter()
        .any(|l| !surviving_layers.contains(l))
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer.0 <= inner.0 && outer.1 <= inner.1 && outer.2 >= inner.2 && outer.3 >= inner.3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shape_can_absorb_several_at_once() {
        // Two stubs at the same x, bridged by a third that overlaps both.
        let in_order = [("VDD", (100, 0, 120, 10)), ("VDD", (100, 90, 120, 100)),
                        ("VDD", (100, 5, 120, 95))];
        assert_eq!(add_shapes(&in_order), vec![("VDD", (100, 0, 120, 100))]);
    }

    #[test]
    fn rails_on_one_row_edge_merge_into_the_wider_one() {
        // 🔑 The full-width row below a macro and the split row above it write the same rail; the
        // y extents are equal, so they merge and the rail spans the macro.
        let rails = [
            ("VDD", (40280, 95030, 359860, 95370)),
            ("VDD", (40280, 95030, 78280, 95370)),
        ];
        assert_eq!(
            add_shapes(&rails),
            vec![("VDD", (40280, 95030, 359860, 95370))]
        );
    }

    #[test]
    fn shapes_aligned_on_neither_axis_are_dropped_not_merged() {
        // ⚠️ `addShape` returns nullptr — the shape does not survive in a reduced form.
        let s = [("VDD", (0, 0, 100, 100)), ("VDD", (50, 50, 200, 200))];
        assert_eq!(add_shapes(&s), vec![("VDD", (0, 0, 100, 100))]);
    }

    #[test]
    fn a_shape_shorting_another_net_is_dropped() {
        let s = [("VDD", (0, 0, 100, 20)), ("VSS", (0, 0, 100, 20))];
        assert_eq!(add_shapes(&s), vec![("VDD", (0, 0, 100, 20))]);
    }

    #[test]
    fn shapes_that_only_touch_are_left_alone() {
        // ⚠️ Interior overlap only: two rails meeting end to end stay two.
        let s = [("VDD", (0, 0, 100, 20)), ("VDD", (100, 0, 200, 20))];
        assert_eq!(add_shapes(&s).len(), 2);
    }

    #[test]
    fn an_exact_duplicate_collapses_into_one() {
        let s = [("VDD", (0, 0, 100, 20)), ("VDD", (0, 0, 100, 20))];
        assert_eq!(add_shapes(&s), vec![("VDD", (0, 0, 100, 20))]);
    }

    #[test]
    fn a_halo_is_the_gap_between_a_shape_and_its_obstruction() {
        let h = obstruction_halo((100, 200, 300, 400), (90, 180, 340, 460));
        assert_eq!(
            h,
            Halo {
                left: 10,
                top: 60,
                right: 40,
                bottom: 20
            }
        );
    }

    #[test]
    fn the_larger_halo_wins_on_each_side_independently() {
        // 🔑 Side by side, so a shape wider along its length and narrower across it contributes
        // to some sides and not others.
        let own = Halo { left: 10, top: 60, right: 40, bottom: 20 };
        let other = Halo { left: 50, top: 5, right: 40, bottom: 30 };
        assert_eq!(
            rect_with_largest_halo((100, 200, 300, 400), own, other),
            (50, 170, 340, 460)
        );
    }

    #[test]
    fn an_obstruction_with_no_halo_of_its_own_is_cleared_by_the_other_shapes() {
        // A plain block obstruction is stored raw; the crossing strap's spacing is what clears it.
        let strap = Halo { left: 540, top: 540, right: 540, bottom: 540 };
        assert_eq!(
            rect_with_largest_halo((0, 0, 100, 100), Halo::default(), strap),
            (-540, -540, 640, 640)
        );
    }
    use crate::Direction;

    #[test]
    fn merging_spans_both_rectangles() {
        assert_eq!(merge((0, 0, 10, 10), (20, 20, 30, 30)), (0, 0, 30, 30));
    }

    #[test]
    fn merging_two_rails_on_a_shared_edge_gives_back_the_same_rail() {
        let rail = (0, 180, 1000, 220);
        assert_eq!(merge(rail, rail), rail);
    }

    #[test]
    fn duplicates_collapse_but_distinct_shapes_do_not() {
        // ⚠️ The 56-rows-to-57-rails case: every interior row edge is written twice.
        let rails = [(0, 0, 10, 4), (0, 0, 10, 4), (0, 8, 10, 12)];
        assert_eq!(dedupe(&rails), vec![(0, 0, 10, 4), (0, 8, 10, 12)]);
    }

    #[test]
    fn dedupe_keeps_the_order_the_shapes_were_made_in() {
        let rails = [(0, 8, 10, 12), (0, 0, 10, 4), (0, 8, 10, 12)];
        assert_eq!(dedupe(&rails), vec![(0, 8, 10, 12), (0, 0, 10, 4)]);
    }

    #[test]
    fn via_metal_overhanging_a_shape_widens_it() {
        // 🔑 A via joining a follow pin to a strap that overhangs the core puts metal spanning the
        // STRAP's width, past the follow pin's end, and merging it widens the follow pin.
        let followpin = (20140, 22230, 180500, 22570);
        let patch = (19140, 22230, 21140, 22570);
        assert_eq!(
            widen_by_via_metal(followpin, &[patch]),
            (19140, 22230, 180500, 22570)
        );
    }

    #[test]
    fn a_patch_that_does_not_align_exactly_does_not_merge() {
        // ⚠️ `addShape` merges only when both edges of one axis coincide. Widening on mere overlap
        // grows shapes the reference leaves alone.
        let followpin = (20140, 22230, 180500, 22570);
        let off_by_one = (19140, 22231, 21140, 22570);
        assert_eq!(widen_by_via_metal(followpin, &[off_by_one]), followpin);
    }

    #[test]
    fn a_patch_inside_the_shape_changes_nothing() {
        let followpin = (20140, 22230, 180500, 22570);
        let inside = (50000, 22230, 52000, 22570);
        assert_eq!(widen_by_via_metal(followpin, &[inside]), followpin);
    }

    #[test]
    fn alignment_on_either_axis_merges_and_the_layer_has_no_say() {
        // 🔑 A follow pin on a VERTICAL layer still runs horizontally. Choosing the axis from the
        // layer merged the two rails of one row into a single 2970-wide shape.
        let shape = (100, 200, 900, 540);
        let along_x = (100, 540, 900, 800); // shares both x edges, extends in y
        assert_eq!(
            widen_by_via_metal(shape, &[along_x]),
            shape,
            "touching only"
        );
        let overlapping = (100, 400, 900, 800);
        assert_eq!(
            widen_by_via_metal(shape, &[overlapping]),
            (100, 200, 900, 800)
        );
    }

    #[test]
    fn a_patch_that_only_touches_is_not_merged() {
        // ⚠️ `Rect::overlaps` is interior-only; the reference discards a hit that merely abuts.
        let shape = (100, 200, 900, 540);
        let abutting = (900, 200, 1200, 540);
        assert_eq!(widen_by_via_metal(shape, &[abutting]), shape);
    }

    // ── H6, extending a strap to meet its via's other shape ──────────────────────────────────

    /// The edge-pin case in miniature: a vertical metal4 strap stopping
    /// at the die edge, and a metal8 strap that runs 170 past it at both ends.
    const STRAP: Rect = (3520, 0, 4480, 201600);
    const REACH: Rect = (2600, -170, 5400, 201770);

    #[test]
    fn a_strap_grows_along_its_own_length_to_meet_the_other_shape() {
        let got = extend_to(STRAP, REACH, &[], &[], 1).expect("it can grow");
        assert_eq!(got, (3520, -170, 4480, 201770));
        // ⚠️ Across, it does not move at all.
        assert_eq!((got.0, got.2), (STRAP.0, STRAP.2));
    }

    #[test]
    fn the_union_is_unconditional_so_a_trimmed_end_grows_back() {
        // ⚠️ **Not "grow only where short".** The span is `min`/`max` against the other rect on
        // both ends, so a strap already trimmed at one end is pulled back out there too. Whether
        // an extension looks one-sided is decided by what it reaches TOWARD, never by what
        // trimming did to it earlier.
        let trimmed_top: Rect = (3520, 0, 4480, 196170);
        assert_eq!(
            extend_to(trimmed_top, REACH, &[], &[], 1).unwrap(),
            (3520, -170, 4480, 201770)
        );
        // Reaching toward something that stops short leaves that end alone.
        let short_partner: Rect = (2600, -170, 5400, 100000);
        assert_eq!(
            extend_to(trimmed_top, short_partner, &[], &[], 1).unwrap(),
            (3520, -170, 4480, 196170)
        );
    }

    // ── H7, what `Via::writeToDb` does with the metal a via leaves on a shape ────────────────

    /// An edge-pin strap with a via at the bottom rail: the metal reaches
    /// 170 below the strap, which starts at the die edge.
    const VSTRAP: Rect = (3520, 0, 4480, 201600);
    const BELOW: Rect = (3520, -170, 4480, 170);

    #[test]
    fn a_vertical_strap_takes_in_metal_past_its_end() {
        // 🔑 Growing in y is a vertical shape's preferred direction, so the strap simply extends —
        // and that is where the reference's -170 comes from.
        assert_eq!(
            check_via_shapes(VSTRAP, &[BELOW], Direction::Vertical, true, &[]),
            ViaCheck::Extend((3520, -170, 4480, 201600))
        );
    }

    #[test]
    fn metal_already_inside_the_shape_changes_nothing() {
        let inside = (3600, 5000, 4400, 5400);
        assert_eq!(
            check_via_shapes(VSTRAP, &[inside], Direction::Vertical, true, &[]),
            ViaCheck::Fits
        );
    }

    #[test]
    fn growth_across_the_preferred_direction_is_refused_and_the_via_ripped_up() {
        // ⚠️ The same metal on a HORIZONTAL layer would widen the shape across its own direction,
        // which is not allowed — so the via goes instead of the shape growing.
        let wide = (3000, 0, 5000, 201600);
        assert_eq!(
            check_via_shapes(VSTRAP, &[wide], Direction::Vertical, true, &[]),
            ViaCheck::Ripup(vec![0])
        );
    }

    #[test]
    fn an_unmodifiable_shape_never_grows() {
        assert_eq!(
            check_via_shapes(VSTRAP, &[BELOW], Direction::Vertical, false, &[]),
            ViaCheck::Ripup(vec![0])
        );
    }

    #[test]
    fn one_obstructed_via_costs_every_overhanging_via_on_that_shape() {
        // 🔑 **Not just the offender.** The reference refuses the extension when `ripup` is
        // non-empty, and then rips up EVERY via shape the rect does not contain — which is how a
        // single blocked via takes nine out at once, as `PDN-0195` reports.
        let overhangs_below = (3520, -170, 4480, 170);
        let blocked = (3520, 201600, 4480, 201900);
        let obs = [(3400, 201700, 4600, 202000)];
        assert_eq!(
            check_via_shapes(
                VSTRAP,
                &[overhangs_below, blocked],
                Direction::Vertical,
                true,
                &obs
            ),
            ViaCheck::Ripup(vec![0, 1]),
            "both overhang, so both go once the extension is refused"
        );
        // ⚠️ And a via wholly inside survives the same refusal.
        let inside = (3600, 5000, 4400, 5400);
        assert_eq!(
            check_via_shapes(VSTRAP, &[inside, blocked], Direction::Vertical, true, &obs),
            ViaCheck::Ripup(vec![1])
        );
    }

    #[test]
    fn a_broken_stack_is_recognised_by_a_layer_no_shape_covers() {
        let connect = ["metal1".to_string(), "metal2".to_string(), "metal3".to_string()];
        let all = ["metal1".to_string(), "metal2".to_string(), "metal3".to_string()];
        assert!(!stack_broken(&all, &connect));
        let missing_middle = ["metal1".to_string(), "metal3".to_string()];
        assert!(stack_broken(&missing_middle, &connect));
    }

    #[test]
    fn a_square_shape_cannot_grow_because_it_has_no_length() {
        assert_eq!(extend_to((0, 0, 100, 100), (0, -50, 100, 150), &[], &[], 1), None);
    }

    #[test]
    fn reaching_nothing_new_is_not_a_change() {
        // The other shape is already inside this one's span.
        assert_eq!(extend_to(STRAP, (3520, 500, 4480, 1000), &[], &[], 1), None);
    }

    #[test]
    fn an_obstruction_in_the_way_refuses_the_growth_rather_than_shortening_it() {
        // ⚠️ All or nothing: the reference returns nullptr, it does not extend part way.
        let blocker = (3520, -200, 4480, -100);
        assert_eq!(extend_to(STRAP, REACH, &[blocker], &[], 1), None);
    }

    #[test]
    fn a_neighbouring_shape_within_the_halo_refuses_it_but_one_a_hair_further_does_not() {
        // The grown strap reaches y = -170; with a halo of 100 the tested rect reaches -269.
        let close = (3520, -300, 4480, -269);
        assert_eq!(extend_to(STRAP, REACH, &[], &[close], 100), None);
        let clear = (3520, -400, 4480, -370);
        assert!(extend_to(STRAP, REACH, &[], &[clear], 100).is_some());
    }

    #[test]
    fn both_ends_widen_independently() {
        let followpin = (20140, 22230, 180500, 22570);
        let left = (19140, 22230, 21140, 22570);
        let right = (179140, 22230, 181140, 22570);
        assert_eq!(
            widen_by_via_metal(followpin, &[left, right]),
            (19140, 22230, 181140, 22570)
        );
    }

    #[test]
    fn a_shape_nothing_crosses_is_left_whole() {
        // ⚠️ `None`, not `Some(vec![the original])`. The reference returns false and keeps the
        // shape as it stands; producing a replacement identical to the original would churn.
        assert_eq!(cut((0, 100), &[]), None);
        assert_eq!(cut((0, 100), &[(200, 300)]), None);
    }

    #[test]
    fn an_obstruction_in_the_middle_leaves_a_piece_on_each_side() {
        assert_eq!(cut((0, 100), &[(40, 60)]), Some(vec![(0, 40), (60, 100)]));
    }

    #[test]
    fn an_obstruction_over_an_end_leaves_one_piece() {
        assert_eq!(cut((0, 100), &[(-50, 40)]), Some(vec![(40, 100)]));
        assert_eq!(cut((0, 100), &[(60, 150)]), Some(vec![(0, 60)]));
    }

    #[test]
    fn an_obstruction_over_the_whole_shape_leaves_nothing() {
        // ⚠️ `Some(empty)`, not `None`: the shape WAS cut, and cut away entirely. A caller that
        // treats an empty list as "unchanged" keeps a strap that should have gone.
        assert_eq!(cut((0, 100), &[(-10, 110)]), Some(vec![]));
    }

    #[test]
    fn overlapping_obstructions_merge_into_one_gap() {
        assert_eq!(
            cut((0, 100), &[(40, 60), (50, 70)]),
            Some(vec![(0, 40), (70, 100)])
        );
    }

    #[test]
    fn obstructions_are_taken_in_position_order_however_they_are_given() {
        assert_eq!(
            cut((0, 100), &[(70, 80), (20, 30)]),
            Some(vec![(0, 20), (30, 70), (80, 100)])
        );
    }

    #[test]
    fn an_obstruction_that_merely_touches_an_end_does_not_cut() {
        // The crossing test is strict on both sides, so an obstruction ending exactly where the
        // shape begins is not crossing it.
        assert_eq!(cut((0, 100), &[(-50, 0)]), None);
        assert_eq!(cut((0, 100), &[(100, 150)]), None);
    }

    #[test]
    fn a_same_net_obstruction_wholly_inside_the_shape_is_forgiven() {
        assert!(same_net_covered((0, 40), (10, 30), false));
    }

    #[test]
    fn a_same_net_obstruction_poking_out_is_not() {
        assert!(!same_net_covered((0, 40), (10, 50), false));
        assert!(!same_net_covered((0, 40), (-10, 30), false));
    }

    #[test]
    fn a_plain_shape_on_the_same_net_still_cuts() {
        // ⚠️ The exemption is for derived obstruction types only — `shapeType() != kShape`.
        assert!(!same_net_covered((0, 40), (10, 30), true));
    }
}

#[cfg(test)]
mod cut_sequence_tests {
    use super::*;

    // ⚠️ **These assert the RULE, not the reference.** Whether the rule is OpenROAD's is checked
    // by an actual reference run — a case that isolates the decision, whose golden is generated
    // rather than transcribed. A constant typed into a unit test cannot notice that it has gone
    // stale; a generated golden can, and does.
    //
    // The numbers are a worked example taken from a real cut, kept because a rule stated in the
    // abstract is hard to debug against. A block obstruction: ⚠️ **its raw rect and its stored
    // rect are the SAME** — the bloat is applied when the obstruction is made and both fields
    // carry the result — so its own halo is ZERO and the shape being cut supplies the only halo
    // there is.
    //
    // ℹ️ Writing this fixture with an invented narrower raw rect made the test fail, and the
    // fixture was wrong rather than the rule: with a halo of its own, `max` would have swallowed
    // the shape's and the mechanism under test would never have applied.
    const RAW: Rect = (2394870, 2398470, 2405130, 2401530);
    const STORED: Rect = RAW;

    fn span(halo: i32) -> Option<(i32, i32)> {
        let h = Halo { left: halo, top: halo, right: halo, bottom: halo };
        blocked_span((359160, 2399430, 5637240, 2399770), h, STORED, RAW, true)
    }

    #[test]
    fn the_cut_shapes_own_halo_is_what_clears_a_block_obstruction() {
        // 🔑 A `kBlockObs` has no halo of its own — its obstruction IS its rect — so the shape
        // being cut supplies the only halo there is.
        assert_eq!(span(130), Some((2394740, 2405260)), "cleared by the crossing shape");
    }

    #[test]
    fn reading_zero_for_that_halo_cuts_short_by_exactly_it() {
        // ⚠️ What a parallel-run-length-table-only lookup answers on a layer that declares no
        // table: nothing. The stored rect's own bloat still applies; the crossing shape's does
        // not, and the cut lands short by exactly the halo that was not read.
        assert_eq!(span(0), Some((2394870, 2405130)), "130 short at each end");
    }

    #[test]
    fn an_obstruction_the_shape_does_not_reach_across_blocks_nothing() {
        // ⚠️ Reach is asked ACROSS. Filtering by layer alone would cut this shape at the
        // obstruction's along-extent even though the two never meet.
        let h = Halo { left: 130, top: 130, right: 130, bottom: 130 };
        assert_eq!(
            blocked_span((359160, 100, 5637240, 440), h, STORED, RAW, true),
            None
        );
    }

    #[test]
    fn reach_uses_the_stored_rect_and_extent_uses_the_raw_one() {
        // 🔑 The load-bearing pair, on an obstruction where the two genuinely differ — a block
        // PIN, which is stored bloated by its own spacing and keeps its raw rect alongside.
        let raw = (1000, 1000, 2000, 2000);
        let stored = (800, 800, 2200, 2200); // its own halo, 200 a side
        let h = Halo { left: 500, top: 500, right: 500, bottom: 500 };
        // Reaches the STORED rect but not the raw one, and is cut all the same.
        let s = blocked_span((0, 2150, 9000, 2190), h, stored, raw, true);
        assert_eq!(s, Some((500, 2500)), "the RAW rect grown by the larger halo, not the stored one");
        // ⚠️ Had the extent come from the stored rect it would read 300..2700 — wider by the
        // obstruction's own halo counted twice.
        assert_ne!(s, Some((300, 2700)));
    }
}
