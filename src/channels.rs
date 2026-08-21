// SPDX-License-Identifier: Apache-2.0
//! Repair channels — straps inserted where the grid left something unconnected.
//!
//! `RepairChannelStraps::repairGridChannels` is stage 6f. After the vias are made, any strap or
//! follow pin with nothing above it is a hole in the grid; the reference finds the region such
//! shapes occupy and drops a narrow strap set into it, on the lowest layer that the orphaned
//! layer's connect statements can reach.
//!
//! 🔑 **It is a search, not a construction.** Width, spacing and offset are all tried and retried
//! until something fits between the obstructions — which is why the straps it produces are usually
//! narrower than any the user declared, and why two channels in one design can come out at
//! different widths.
//!
//! Nothing here touches a database.

use crate::Rect;

/// **K1** — the regions a set of bloated shapes occupies.
///
/// Each unconnected shape is bloated by its own strap set's pitch **across** its direction, and the
/// overlapping results are merged. Shapes on a regular pitch therefore merge into one region, which
/// is the point: a channel is a run of neighbouring orphans, not one orphan.
///
/// ⚠️ **Touching counts as overlapping.** Two rails bloated by exactly their pitch meet edge to
/// edge, and treating that as separate splits every channel in two.
///
/// 🔑 **The union is DECOMPOSED into rectangles, not reduced to a bounding box.**
///
/// Boost slices the union into horizontal slabs: within one band of y the region's x extent is one
/// interval (or several, where the band is disconnected), and vertically adjacent slabs sharing an
/// x interval are merged back together.
///
/// ⚠️ **A rectangular region is unaffected**, which is why a bounding box passed for so long. It
/// diverges exactly where the region JOGS: an L comes back as two slabs — a wide one under a narrow
/// one — and each is a channel of its own, repaired separately and at its own width. Reduced to its
/// bounding box the L becomes one channel too wide for anything to fit, and the repair is refused
/// outright rather than done in two pieces.
///
/// ℹ️ Each slab is afterwards grown by every orphan overlapping it, which is what makes the wide
/// slab reach back over the narrow one. That growth is the caller's, not this function's.
pub fn merge_channels(bloated: &[Rect]) -> Vec<Rect> {
    let mut ys: Vec<i32> = bloated.iter().flat_map(|r| [r.1, r.3]).collect();
    ys.sort_unstable();
    ys.dedup();

    let mut slabs: Vec<Rect> = Vec::new();
    for band in ys.windows(2) {
        let (y0, y1) = (band[0], band[1]);
        // The x intervals covering this band, merged. ⚠️ Touching counts as covered: two rails
        // bloated until they meet edge to edge are one region, not two.
        let mut xs: Vec<(i32, i32)> = bloated
            .iter()
            .filter(|r| r.1 <= y0 && y1 <= r.3)
            .map(|r| (r.0, r.2))
            .collect();
        if xs.is_empty() {
            continue;
        }
        xs.sort_unstable();
        let mut spans: Vec<(i32, i32)> = Vec::new();
        for (a, b) in xs {
            match spans.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => spans.push((a, b)),
            }
        }
        for (a, b) in spans {
            slabs.push((a, y0, b, y1));
        }
    }

    // Put a slab back together with the one above it where they share an x interval, so a plain
    // rectangle comes back as one rect rather than one per distinct edge.
    slabs.sort_unstable_by_key(|r| (r.0, r.2, r.1));
    let mut out: Vec<Rect> = Vec::new();
    for s in slabs {
        match out.last_mut() {
            Some(l) if l.0 == s.0 && l.2 == s.2 && l.3 == s.1 => l.3 = s.3,
            _ => out.push(s),
        }
    }
    out.sort_unstable();
    out
}

/// **K2** — the part of a channel still free on the layer the repair straps will use.
///
/// A channel is measured on the layer that was left unconnected, and the straps go on another. If
/// something is already standing on the target layer inside the channel, the straps have less room
/// than the channel suggests.
///
/// ⚠️ **Only an obstruction straddling an EDGE trims, and it trims that edge to itself.** The test
/// is `available.max > obs.min && available.max <= obs.max` — an obstruction wholly inside the
/// channel fails it on both sides and is ignored, leaving the straps to be placed across it and
/// rejected later by the offset search. Trimming to the nearer side of an interior obstruction
/// would look tidier and is not what the reference does.
///
/// `blocking` are the obstruction rects of shapes already on the target layer.
pub fn available_area(area: Rect, blocking: &[Rect], vertical: bool) -> Rect {
    let mut out = area;
    for o in blocking {
        if vertical {
            if out.2 > o.0 && out.2 <= o.2 {
                out.2 = o.0;
            }
            if out.0 < o.2 && out.0 >= o.0 {
                out.0 = o.2;
            }
        } else {
            if out.3 > o.1 && out.3 <= o.3 {
                out.3 = o.1;
            }
            if out.1 < o.3 && out.1 >= o.1 {
                out.1 = o.3;
            }
        }
    }
    out
}

/// **K3** — the width one group of straps occupies: one per net, with spacing between.
pub fn group_width(nets: usize, width: i32, spacing: i32) -> i32 {
    if nets == 0 {
        return 0;
    }
    nets as i32 * width + (nets as i32 - 1) * spacing
}

/// **K4** — the next narrower width to try when a group will not fit.
///
/// Halved and snapped **down** to twice the manufacturing grid, then floored at the layer's minimum
/// width. ⚠️ **Twice the grid, not once**: the width is halved again to place the strap, so an odd
/// multiple of the grid would put its edges off it.
///
/// ℹ️ LEF58 `WIDTHTABLE` rules further restrict this to a listed width; not modelled, and no
/// technology in the suite states one.
pub fn next_width(width: i32, min_width: i32, manufacturing_grid: i32) -> i32 {
    let halved = crate::straps::snap_to_manufacturing_grid(
        width / 2,
        manufacturing_grid.saturating_mul(2),
        false,
    );
    if halved <= min_width {
        min_width
    } else {
        halved
    }
}

/// **K5** — where to put the group, searching around whatever is in the way.
///
/// The group is centred in the available area and snapped to the manufacturing grid. If it does not
/// fit, or if anything obstructs it, the search **bisects**: first by a quarter of the channel's
/// width, then by half of that each time, trying the LOW side before the high one at every level.
///
/// 🔑 **The bisection is not a scan.** It is a depth-first walk of offsets `±w/4`, `±w/8`, … from
/// the centre, and the first that clears wins — so the result is the one nearest the centre only in
/// the loose sense that closer offsets are tried at shallower levels. Replacing it with a sweep
/// finds a different offset in any channel where more than one position is free.
///
/// ⚠️ **It stops when the bisection distance snaps to zero**, not at a level count.
///
/// `clear` is asked whether the group placed at a given rect is free of obstructions; it receives
/// the group's whole extent, across and along.
///
/// Returns the group's centre-line position for the first strap, in absolute coordinates.
pub fn determine_offset(
    available: Rect,
    vertical: bool,
    width: i32,
    group: i32,
    manufacturing_grid: i32,
    clear: &dyn Fn(Rect) -> bool,
) -> Option<i32> {
    fn search(
        available: Rect,
        vertical: bool,
        width: i32,
        group: i32,
        mfg: i32,
        clear: &dyn Fn(Rect) -> bool,
        extra: i32,
        bisect: i32,
    ) -> Option<i32> {
        let (lo, hi) = if vertical {
            (available.0, available.2)
        } else {
            (available.1, available.3)
        };
        // ⚠️ The centre is computed from the SUM, matching `0.5 * (min + max)` — halving each and
        // adding drops a unit on an odd span.
        let mut offset = -group / 2 + extra + (lo + hi) / 2;
        let half_width = width / 2;
        offset += half_width;
        offset = crate::straps::snap_to_manufacturing_grid(offset, mfg, false);

        let start = offset - half_width;
        let straps = if vertical {
            (start, available.1, start + group, available.3)
        } else {
            (available.0, start, available.2, start + group)
        };

        let fits = if vertical {
            straps.2 - straps.0 <= hi - lo && straps.0 >= lo && straps.2 <= hi
        } else {
            straps.3 - straps.1 <= hi - lo && straps.1 >= lo && straps.3 <= hi
        };
        if !fits {
            return None;
        }
        if clear(straps) {
            return Some(offset);
        }

        let next = if bisect == 0 {
            (hi - lo) / 4
        } else {
            bisect / 2
        };
        let next = crate::straps::snap_to_manufacturing_grid(next, mfg, false);
        if next == 0 {
            return None;
        }
        // Low side first, then high — the order decides which of two free offsets is taken.
        search(
            available, vertical, width, group, mfg, clear,
            extra - next, next,
        )
        .or_else(|| {
            search(
                available, vertical, width, group, mfg, clear,
                extra + next, next,
            )
        })
    }
    search(
        available, vertical, width, group, manufacturing_grid, clear, 0, 0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_on_a_regular_pitch_merge_into_one_channel() {
        // Three rails, each bloated until it meets the next.
        let rails = [(0, 0, 100, 20), (0, 20, 100, 40), (0, 40, 100, 60)];
        assert_eq!(merge_channels(&rails), vec![(0, 0, 100, 60)]);
    }

    #[test]
    fn shapes_out_of_reach_stay_separate() {
        let rails = [(0, 0, 100, 20), (0, 50, 100, 70)];
        assert_eq!(
            merge_channels(&rails),
            vec![(0, 0, 100, 20), (0, 50, 100, 70)]
        );
    }

    #[test]
    fn a_late_arrival_can_join_two_groups_that_were_apart() {
        // 🔑 The reason the absorb loop repeats: the third rect bridges the first two.
        let rails = [(0, 0, 10, 10), (30, 0, 40, 10), (5, 0, 35, 10)];
        assert_eq!(merge_channels(&rails), vec![(0, 0, 40, 10)]);
    }

    #[test]
    fn an_l_shaped_region_comes_back_as_two_slabs() {
        // 🔑 The jog. A wide band with a narrow one sitting on its left half: horizontal slicing
        // gives the wide slab and the narrow slab, and NOT the bounding box that covers both.
        let region = [(0, 0, 100, 20), (0, 20, 40, 50)];
        assert_eq!(
            merge_channels(&region),
            vec![(0, 0, 100, 20), (0, 20, 40, 50)]
        );
    }

    #[test]
    fn a_band_split_in_two_gives_one_rect_per_piece() {
        let region = [(0, 0, 20, 10), (50, 0, 70, 10)];
        assert_eq!(merge_channels(&region), vec![(0, 0, 20, 10), (50, 0, 70, 10)]);
    }

    #[test]
    fn a_step_on_both_sides_gives_three_slabs() {
        let region = [(0, 10, 100, 20), (30, 0, 60, 30)];
        assert_eq!(
            merge_channels(&region),
            vec![(0, 10, 100, 20), (30, 0, 60, 10), (30, 20, 60, 30)]
        );
    }

    #[test]
    fn overlapping_rects_that_form_a_rectangle_come_back_as_one() {
        // ⚠️ The vertical re-merge: without it a plain rectangle returns one slab per edge.
        let region = [(0, 0, 100, 30), (0, 10, 100, 60)];
        assert_eq!(merge_channels(&region), vec![(0, 0, 100, 60)]);
    }

    #[test]
    fn an_obstruction_over_an_edge_pulls_that_edge_back() {
        let area = (0, 0, 100, 50);
        assert_eq!(available_area(area, &[(80, 0, 120, 50)], true), (0, 0, 80, 50));
        assert_eq!(available_area(area, &[(-20, 0, 30, 50)], true), (30, 0, 100, 50));
    }

    #[test]
    fn an_obstruction_wholly_inside_the_channel_does_not_trim_it() {
        // ⚠️ Deliberate: `max > obs.min && max <= obs.max` fails on both sides.
        let area = (0, 0, 100, 50);
        assert_eq!(available_area(area, &[(40, 0, 60, 50)], true), area);
    }

    #[test]
    fn a_group_is_one_strap_per_net_with_spacing_between() {
        assert_eq!(group_width(2, 20000, 8000), 48000);
        assert_eq!(group_width(1, 20000, 8000), 20000);
        assert_eq!(group_width(0, 20000, 8000), 0);
    }

    #[test]
    fn the_next_width_is_half_snapped_down_to_twice_the_grid() {
        assert_eq!(next_width(20000, 1400, 10), 10000);
        // 9000 / 2 = 4500, snapped down to a multiple of 20.
        assert_eq!(next_width(9000, 1400, 10), 4500 - 4500 % 20);
    }

    #[test]
    fn the_next_width_never_goes_below_the_minimum() {
        assert_eq!(next_width(2000, 1400, 10), 1400);
        assert_eq!(next_width(1400, 1400, 10), 1400);
    }

    #[test]
    fn a_group_that_fits_and_is_clear_sits_on_the_centre_line() {
        // Channel 0..100 across, group 20 wide, strap width 20: centred at 50, so the strap runs
        // 40..60 and its centre line is 50.
        let off = determine_offset((0, 0, 100, 10), true, 20, 20, 1, &|_| true);
        assert_eq!(off, Some(50));
    }

    #[test]
    fn a_group_wider_than_the_channel_is_refused_outright() {
        assert_eq!(
            determine_offset((0, 0, 30, 10), true, 40, 40, 1, &|_| true),
            None
        );
    }

    #[test]
    fn an_obstruction_at_the_centre_pushes_the_search_low_first() {
        // 🔑 Anything overlapping x 45..55 is blocked, so the centre fails and the search tries
        // -25 before +25.
        let clear = |r: Rect| r.2 <= 45 || r.0 >= 55;
        let off = determine_offset((0, 0, 100, 10), true, 20, 20, 1, &clear);
        assert_eq!(off, Some(25), "the low side is tried first");
    }

    #[test]
    fn the_search_gives_up_when_the_bisection_reaches_zero() {
        assert_eq!(
            determine_offset((0, 0, 100, 10), true, 20, 20, 1, &|_| false),
            None
        );
    }
}
