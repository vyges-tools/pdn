// SPDX-License-Identifier: Apache-2.0
//! Straps — the repeating stripes that carry power across the die.
//!
//! A strap set lays one stripe per net at each step of a pitch, snapped to the routing grid where
//! asked. The stripes for one step form a *group*, and the group advances by its own pitch as it
//! is laid, so the nets within a group sit against each other rather than being spread.
//!
//! Nothing here touches a database.

use crate::Rect;

/// How far along its own direction a strap reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extend {
    /// The voltage domain's boundary.
    Core,
    /// Out to the rings.
    Rings,
    /// The grid's boundary.
    Boundary,
    /// A fixed pair of coordinates given by the caller.
    Fixed,
}

/// One strap set's parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub layer: String,
    pub width: i32,
    /// Between the nets of one group.
    pub spacing: i32,
    /// Between one group and the next.
    pub pitch: i32,
    pub offset: i32,
    /// Zero means "as many as fit".
    pub number_of_straps: i32,
    pub snap: bool,
    /// Whether the across extent may run past the core to the die edge.
    pub allow_out_of_core: bool,
}

/// One stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stripe {
    pub layer: String,
    pub net: String,
    pub rect: Rect,
}

/// **S1** — snap a coordinate to the nearest routing track at or above a floor.
///
/// ⚠️ **The scan stops at the first track that is no better than the one before it**, which is only
/// correct because the grid is sorted ascending — the deltas fall until the nearest track is passed
/// and rise thereafter. Scanning the whole grid instead gives the same answer here but a different
/// one on an unsorted grid, so the ordering is part of the contract, not an accident of the data.
///
/// ⚠️ **With no track at or above the floor the position is returned unsnapped**, not clamped to
/// the last track. A strap can therefore sit off-grid, and that is the reference's answer rather
/// than a failure to handle the case.
pub fn snap_to_grid(pos: i32, greater_than: i32, grid: &[i32]) -> i32 {
    if grid.is_empty() {
        return pos;
    }
    let mut best: Option<i32> = None;
    let mut delta = i32::MAX;
    for &g in grid {
        if g < greater_than {
            continue;
        }
        let d = (pos - g).abs();
        if d < delta {
            best = Some(g);
            delta = d;
        } else {
            break;
        }
    }
    best.unwrap_or(pos)
}

/// **S4** — round a value onto the manufacturing grid.
///
/// ⚠️ **Only adjusts a value that is off the grid**, and rounds toward zero rather than down —
/// integer division truncates, so a negative value moves *up* in magnitude terms. `round_up` adds
/// one grid step first, which is not the same as rounding to nearest.
pub fn snap_to_manufacturing_grid(pos: i32, grid: i32, round_up: bool) -> i32 {
    if grid <= 0 || pos % grid == 0 {
        return pos;
    }
    let mut n = pos / grid;
    if round_up {
        n += 1;
    }
    n * grid
}

/// **S5** — the spacing between the nets of a group, where the caller gave none.
///
/// ⚠️ **It divides the pitch by the NUMBER OF NETS, not by two.** With the usual power/ground pair
/// the two coincide, which is exactly what makes this worth pinning: inferring the rule from a
/// two-net design gives `pitch / 2 - width`, and that is wrong for every design with a switched or
/// secondary supply. The nets of a group are meant to share the pitch evenly between them.
///
/// ⚠️ Rounded **down** onto the manufacturing grid on purpose. Rounding up would push the group
/// wider than its pitch and fail the pitch check instead; a spacing below the layer minimum is
/// caught later by its own check, which is the error the author wanted to surface.
pub fn default_spacing(pitch: i32, net_count: i32, width: i32, manufacturing_grid: i32) -> i32 {
    if net_count == 0 {
        return 0;
    }
    snap_to_manufacturing_grid(pitch / net_count - width, manufacturing_grid, false)
}

/// Why a strap set stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Every position that fits was used.
    Exhausted,
    /// The requested number of groups was reached.
    Enough,
    /// A stripe fell past the end of the allotted span.
    PastEnd,
}

/// **S2** — lay the stripes.
///
/// `span` is a plain `(xlo, ylo, xhi, yhi)`. ⚠️ Deliberately not named "along" and "across": which
/// axis is which **inverts** between the two orientations, so a name for one is a lie for the
/// other. Horizontal stripes run along x and step along y; vertical stripes run along y and step
/// along x. `abs` bounds the stepping axis absolutely (the die), and a stripe outside it is
/// dropped.
///
/// ⚠️ **Three details that each change the answer:**
///
/// - **The group position advances before the stripe is tested, not after.** A stripe dropped for
///   hitting an avoidance or falling outside the die still moves the group on, so the nets after it
///   keep their positions rather than closing up. Skipping *and* holding the position back would
///   pack the group differently.
/// - **Running past the end abandons the whole set**, not just that stripe: both checks return.
///   A later group that would have fitted is never tried.
/// - **The count is of GROUPS, not stripes.** With three nets, `number_of_straps = 1` yields three
///   stripes, because the counter increments once per step of the pitch.
///
/// ⚠️ `strap_end` is `strap_start + width`, and `strap_start` is `pos - width / 2`. For an odd
/// width that is **not** symmetric about the position — the extra unit falls on the high side, and
/// writing `pos + width / 2` instead loses it.
pub fn make_straps(
    spec: &Spec,
    nets: &[String],
    span: Rect,
    abs: (i32, i32),
    grid: &[i32],
    avoid: &[Rect],
    horizontal: bool,
) -> (Vec<Stripe>, Stopped) {
    let (x0, y0, x1, y1) = span;
    // ⚠️ **A pitch of zero never advances.** `makeStraps` steps `pos += pitch_` and stops only when
    // `pos` passes the end, so a zero pitch is an infinite loop — in the reference too. `pdn.tcl`
    // requires `-pitch`, so the reference never reaches it; a translation that hands the option
    // through unevaluated does, and the engine spins at full tilt with nothing to show.
    //
    // ⚠️ Unless a count is stated: `number_of_straps_` stops the loop by itself, and one group at
    // a fixed offset is a legitimate thing to ask for — that is exactly what a repair channel is.
    if spec.pitch <= 0 && spec.number_of_straps <= 0 {
        return (Vec::new(), Stopped::Exhausted);
    }
    let half_width = spec.width / 2;
    let group_pitch = spec.spacing + spec.width;
    // `pos` walks the STEPPING axis: y for horizontal stripes, x for vertical ones.
    let (start, end) = if horizontal { (y0, y1) } else { (x0, x1) };

    let mut out = Vec::new();
    let mut groups = 0;
    let mut next_minimum_track = i32::MIN;
    let mut pos = start + spec.offset;

    while pos <= end {
        let mut group_pos = pos;
        for net in nets {
            group_pos = if spec.snap {
                snap_to_grid(group_pos, next_minimum_track, grid)
            } else {
                group_pos
            };
            let strap_start = group_pos - half_width;
            let strap_end = strap_start + spec.width;

            if strap_start >= end || group_pos > end {
                return (out, Stopped::PastEnd);
            }

            let rect = if horizontal {
                (x0, strap_start, x1, strap_end)
            } else {
                (strap_start, y0, strap_end, y1)
            };

            // ⚠️ Before the tests below, deliberately. See the note on the function.
            group_pos += group_pitch;
            next_minimum_track = group_pos;

            if avoid.iter().any(|a| intersects(rect, *a)) {
                continue;
            }
            let (lo, hi) = if horizontal {
                (rect.1, rect.3)
            } else {
                (rect.0, rect.2)
            };
            if lo < abs.0 || hi > abs.1 {
                continue;
            }
            out.push(Stripe {
                layer: spec.layer.clone(),
                net: net.clone(),
                rect,
            });
        }
        groups += 1;
        if spec.number_of_straps != 0 && groups == spec.number_of_straps {
            return (out, Stopped::Enough);
        }
        pos += spec.pitch;
    }
    (out, Stopped::Exhausted)
}

/// **S3** — the span a strap set is laid into.
///
/// ⚠️ **Asymmetric on purpose.** The across axis always *starts* at the core, and only its **end**
/// moves out to the die when the set is allowed out of the core. A set that ran from die edge to
/// die edge would be a different grid.
pub fn span(
    core: Rect,
    die: Rect,
    boundary: Rect,
    horizontal: bool,
    allow_out_of_core: bool,
) -> Rect {
    if horizontal {
        (
            boundary.0,
            core.1,
            boundary.2,
            if allow_out_of_core { die.3 } else { core.3 },
        )
    } else {
        (
            core.0,
            boundary.1,
            if allow_out_of_core { die.2 } else { core.2 },
            boundary.3,
        )
    }
}

/// The absolute bounds a stripe's across extent must lie within: the die, on the across axis.
pub fn absolute(die: Rect, horizontal: bool) -> (i32, i32) {
    if horizontal {
        (die.1, die.3)
    } else {
        (die.0, die.2)
    }
}

/// Closed intersection, as odb's `Rect::intersects` spells it — touching counts.
fn intersects(a: Rect, b: Rect) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pitch_of_zero_builds_nothing_rather_than_looping_for_ever() {
        // ⚠️ `makeStraps` steps `pos += pitch_`, so a zero pitch never advances. The reference is
        // saved from it by `pdn.tcl` requiring `-pitch`; nothing saves a caller that hands the
        // option through unevaluated.
        let spec = Spec {
            layer: "m1".into(),
            width: 100,
            spacing: 100,
            pitch: 0,
            offset: 0,
            number_of_straps: 0,
            snap: false,
            allow_out_of_core: false,
        };
        let (stripes, _) = make_straps(
            &spec,
            &["VDD".into(), "VSS".into()],
            (0, 0, 10000, 10000),
            (0, 10000),
            &[],
            &[],
            true,
        );
        assert!(stripes.is_empty());
    }

    #[test]
    fn a_stated_count_makes_a_zero_pitch_legitimate() {
        // 🔑 One group at a fixed offset is exactly what a repair channel asks for, and the count
        // stops the loop by itself.
        let spec = Spec {
            layer: "m1".into(),
            width: 100,
            spacing: 100,
            pitch: 0,
            offset: 500,
            number_of_straps: 1,
            snap: false,
            allow_out_of_core: false,
        };
        let (stripes, _) = make_straps(
            &spec,
            &["VDD".into(), "VSS".into()],
            (0, 0, 10000, 10000),
            (0, 10000),
            &[],
            &[],
            true,
        );
        assert_eq!(stripes.len(), 2, "one group, one strap per net");
    }

    fn spec(width: i32, spacing: i32, pitch: i32) -> Spec {
        Spec {
            layer: "m5".into(),
            width,
            spacing,
            pitch,
            offset: 0,
            number_of_straps: 0,
            snap: false,
            allow_out_of_core: false,
        }
    }

    fn nets(n: usize) -> Vec<String> {
        ["VSS", "VDD", "VNN"][..n]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    // ── snapping ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn with_no_grid_the_position_is_left_alone() {
        assert_eq!(snap_to_grid(37, i32::MIN, &[]), 37);
    }

    #[test]
    fn a_position_snaps_to_the_nearest_track() {
        assert_eq!(snap_to_grid(37, i32::MIN, &[0, 10, 20, 30, 40, 50]), 40);
        assert_eq!(snap_to_grid(31, i32::MIN, &[0, 10, 20, 30, 40, 50]), 30);
    }

    #[test]
    fn tracks_below_the_floor_are_passed_over() {
        // The floor is what keeps one group's stripes from landing on the previous one's track.
        assert_eq!(snap_to_grid(12, 30, &[0, 10, 20, 30, 40]), 30);
    }

    #[test]
    fn with_every_track_below_the_floor_the_position_is_returned_unsnapped() {
        // ⚠️ Not clamped to the last track. The strap ends up off-grid, and that is the answer.
        assert_eq!(snap_to_grid(12, 100, &[0, 10, 20]), 12);
    }

    #[test]
    fn a_tie_keeps_the_lower_track() {
        // ⚠️ The scan stops at the first delta that is not an improvement, and an equal delta is
        // not an improvement — so the track BELOW wins a midpoint.
        assert_eq!(snap_to_grid(15, i32::MIN, &[0, 10, 20, 30]), 10);
    }

    // ── laying stripes ───────────────────────────────────────────────────────────────────────

    /// A die bound wide enough not to interfere. ⚠️ The obvious `(0, ...)` clips the first stripe,
    /// which is centred ON the start of the span and so reaches half a width below it.
    const WIDE: (i32, i32) = (-1000, 1000);

    #[test]
    fn one_stripe_per_net_at_each_step_of_the_pitch() {
        let s = spec(10, 10, 100);
        let (out, why) = make_straps(&s, &nets(2), (0, 0, 1000, 400), WIDE, &[], &[], true);
        // ⚠️ Groups at 0, 100, 200, 300 give two stripes each; at 400 the first net still fits and
        // the second runs past the end. **A multi-net set almost always ends this way** rather than
        // by the loop condition, so `Exhausted` is the exception and not the rule.
        assert_eq!(why, Stopped::PastEnd);
        assert_eq!(out.len(), 9);
        assert_eq!(out[0].net, "VSS");
        assert_eq!(out[1].net, "VDD");
    }

    #[test]
    fn the_nets_of_a_group_sit_one_group_pitch_apart() {
        let s = spec(10, 10, 100);
        let (out, _) = make_straps(&s, &nets(2), (0, 0, 1000, 100), WIDE, &[], &[], true);
        assert_eq!(
            out[0].rect,
            (0, -5, 1000, 5),
            "first net is centred on the position"
        );
        assert_eq!(
            out[1].rect,
            (0, 15, 1000, 25),
            "second is width + spacing further on"
        );
    }

    #[test]
    fn an_odd_width_puts_the_extra_unit_on_the_high_side() {
        // ⚠️ `strap_end = strap_start + width`, not `pos + width / 2`. For width 11 the stripe runs
        // from pos-5 to pos+6. Writing it symmetrically loses a unit and every stripe is wrong.
        let s = spec(11, 10, 100);
        let (out, _) = make_straps(&s, &nets(1), (0, 100, 1000, 100), WIDE, &[], &[], true);
        assert_eq!(out[0].rect, (0, 95, 1000, 106));
    }

    #[test]
    fn the_count_is_of_groups_not_of_stripes() {
        // ⚠️ With three nets, asking for one strap gives THREE stripes: the counter steps once per
        // pitch, not once per stripe.
        let mut s = spec(10, 10, 100);
        s.number_of_straps = 1;
        let (out, why) = make_straps(&s, &nets(3), (0, 0, 1000, 900), WIDE, &[], &[], true);
        assert_eq!(why, Stopped::Enough);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn a_stripe_past_the_end_abandons_the_whole_set() {
        // ⚠️ `return`, not `continue`. Once a group runs past the end nothing later is attempted,
        // even where a later position would have fitted.
        let s = spec(10, 10, 100);
        let (out, why) = make_straps(&s, &nets(3), (0, 0, 1000, 30), WIDE, &[], &[], true);
        assert_eq!(why, Stopped::PastEnd);
        assert_eq!(
            out.len(),
            2,
            "the third net of the first group is already past 30"
        );
    }

    #[test]
    fn a_stripe_over_an_avoidance_is_dropped_without_closing_the_gap() {
        // 🔑 The group position advances before the test, so dropping the first net's stripe leaves
        // the second exactly where it would have been. A version that skipped and held the position
        // back would put the second net at the first one's place.
        let s = spec(10, 10, 100);
        let avoid = [(0, -100, 1000, 5)];
        let (out, _) = make_straps(&s, &nets(2), (0, 0, 1000, 100), WIDE, &[], &avoid, true);
        assert_eq!(out[0].net, "VDD", "the first net's stripe was dropped");
        assert_eq!(
            out[0].rect,
            (0, 15, 1000, 25),
            "unmoved by its neighbour being dropped"
        );
    }

    #[test]
    fn a_stripe_outside_the_die_is_dropped_rather_than_clipped() {
        let s = spec(10, 10, 100);
        // The die stops at 20, so the second net's stripe (15..25) falls outside it.
        let (out, _) = make_straps(&s, &nets(2), (0, 0, 1000, 100), (-100, 20), &[], &[], true);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].net, "VSS");
        assert_eq!(
            out[0].rect,
            (0, -5, 1000, 5),
            "kept whole, not trimmed to the die"
        );
    }

    #[test]
    fn the_offset_moves_the_first_group_only() {
        let mut s = spec(10, 10, 100);
        s.offset = 50;
        let (out, _) = make_straps(&s, &nets(1), (0, 0, 1000, 200), WIDE, &[], &[], true);
        let centres: Vec<i32> = out.iter().map(|o| o.rect.1 + 5).collect();
        assert_eq!(
            centres,
            vec![50, 150],
            "50, then a pitch on -- not 50 then 200"
        );
    }

    #[test]
    fn vertical_stripes_step_along_x_and_run_along_y() {
        let s = spec(10, 10, 100);
        let (out, _) = make_straps(&s, &nets(1), (0, 40, 200, 900), WIDE, &[], &[], false);
        assert_eq!(
            out[0].rect,
            (-5, 40, 5, 900),
            "runs the full y span, narrow in x"
        );
    }

    #[test]
    fn snapping_pushes_each_net_onto_its_own_track() {
        let mut s = spec(10, 10, 100);
        s.snap = true;
        let grid: Vec<i32> = (0..40).map(|i| i * 25).collect();
        let (out, _) = make_straps(&s, &nets(2), (0, 0, 1000, 100), WIDE, &grid, &[], true);
        let centres: Vec<i32> = out.iter().map(|o| o.rect.1 + 5).collect();
        // ⚠️ Three, not two: the second group at pitch 100 still fits, and its first net snaps to
        // the track at 100. The floor from the previous group is what pushes each net off its
        // neighbour's track.
        assert_eq!(centres, vec![0, 25, 100]);
    }

    // ── defaults ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_default_spacing_divides_the_pitch_between_the_NETS_not_by_two() {
        // ⚠️ The trap: with two nets these agree, so a rule inferred from a power/ground design
        // looks right and is wrong the moment a third net appears.
        assert_eq!(default_spacing(20000, 2, 1860, 5), 8140);
        assert_eq!(
            default_spacing(20000, 3, 1860, 5),
            4805,
            "pitch/3, not pitch/2"
        );
        assert_ne!(
            default_spacing(20000, 3, 1860, 5),
            default_spacing(20000, 2, 1860, 5)
        );
    }

    #[test]
    fn the_default_spacing_rounds_down_onto_the_manufacturing_grid() {
        // 20000/3 - 1860 = 4806.67 -> 4806 by integer division, then down to 4805 on a grid of 5.
        assert_eq!(default_spacing(20000, 3, 1860, 5), 4805);
    }

    #[test]
    fn no_nets_asks_for_no_spacing_rather_than_dividing_by_zero() {
        assert_eq!(default_spacing(20000, 0, 1860, 5), 0);
    }

    #[test]
    fn a_value_already_on_the_manufacturing_grid_is_left_alone() {
        assert_eq!(snap_to_manufacturing_grid(100, 5, false), 100);
        assert_eq!(
            snap_to_manufacturing_grid(100, 5, true),
            100,
            "even when rounding up"
        );
    }

    #[test]
    fn rounding_up_adds_a_whole_step_rather_than_going_to_nearest() {
        // ⚠️ 101 is nearer 100 than 105, and rounding up still gives 105.
        assert_eq!(snap_to_manufacturing_grid(101, 5, true), 105);
        assert_eq!(snap_to_manufacturing_grid(101, 5, false), 100);
    }

    #[test]
    fn a_negative_value_truncates_toward_zero() {
        // ⚠️ Integer division, not a floor. -101 goes to -100, which is UP in value.
        assert_eq!(snap_to_manufacturing_grid(-101, 5, false), -100);
    }

    #[test]
    fn with_no_manufacturing_grid_nothing_is_snapped() {
        assert_eq!(snap_to_manufacturing_grid(101, 0, false), 101);
    }

    // ── the span ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_strap_set_starts_at_the_core_even_when_allowed_out_of_it() {
        // ⚠️ Only the END moves to the die. A set spanning die edge to die edge is a different grid.
        let core = (100, 100, 900, 900);
        let die = (0, 0, 1000, 1000);
        let boundary = (50, 50, 950, 950);
        assert_eq!(span(core, die, boundary, true, false), (50, 100, 950, 900));
        assert_eq!(span(core, die, boundary, true, true), (50, 100, 950, 1000));
        assert_eq!(span(core, die, boundary, false, true), (100, 50, 1000, 950));
    }

    #[test]
    fn the_absolute_bound_is_the_die_on_the_across_axis() {
        assert_eq!(absolute((0, 10, 1000, 990), true), (10, 990));
        assert_eq!(absolute((0, 10, 1000, 990), false), (0, 1000));
    }
}
