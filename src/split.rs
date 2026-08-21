// SPDX-License-Identifier: Apache-2.0
//! Split-cut arrays — many single-cut vias where an ordinary connect makes one arrayed via.
//!
//! `add_pdn_connect -split_cuts {M2 1.0}` asks that the cuts crossing a layer be spread out rather
//! than packed into one array. The reference does that by building a **1x1 via** and then placing
//! it repeatedly, which is a different shape of computation from `determineRowsAndColumns` and
//! produces a different result even where the counts agree.
//!
//! Nothing here touches a database.

/// **W1** — how many positions a split array has on each axis.
///
/// ⚠️ **Integer division of the available width by the cut pitch, floored at one.** Not a fit
/// calculation: no enclosure is consulted and no cut size is subtracted, so this gives one more
/// position than a `cuts_across`-style computation wherever the last cut would not have fitted.
///
/// A limit of zero means unlimited, which is how `getMaxColumns` reports "not set".
pub fn counts(
    extent: (i32, i32),
    pitch: (i32, i32),
    max_columns: i32,
    max_rows: i32,
) -> (i32, i32) {
    let one = |span: i32, p: i32, limit: i32| {
        if p <= 0 {
            return 1;
        }
        let n = (span / p).max(1);
        if limit > 0 {
            n.min(limit)
        } else {
            n
        }
    };
    (
        one(extent.0, pitch.0, max_columns),
        one(extent.1, pitch.1, max_rows),
    )
}

/// **W2** — where each of a split array's vias goes.
///
/// The array spans `(cols - 1) * pitch` by `(rows - 1) * pitch` and is centred on the placement
/// point; each position is then snapped to a routing grid.
///
/// 🔑 **The next position is measured from the SNAPPED one**, never from an ideal lattice:
///
/// ```text
/// col = via_rect.xMin() + offset
/// for each column:
///     col_pos = snap(col)
///     place at col_pos
///     col = col_pos + pitch      <- from the snapped value
/// ```
///
/// ⚠️ So snapping **accumulates** along the array. Computing `xMin + c * pitch` and snapping each
/// result independently agrees only where the grid divides the pitch, and drifts everywhere else —
/// by a whole grid step by the end of a long row.
///
/// ⚠️ **An offset applies only where that axis has more than one position.** A single column is
/// placed on the centre, not offset away from it.
///
/// `snap_x` and `snap_y` are the routing grids to snap to. ⚠️ They are chosen by the **bottom
/// layer's** direction, not by the axis: y snaps to the horizontal layer's grid and x to the
/// vertical layer's.
///
/// ℹ️ **Both snap flags are passed `false` at the only call site**, so `populateGrid` never runs,
/// the grids are empty, and `TechLayer::snapToGrid` returns its argument. In every case this suite
/// contains the array is therefore a plain lattice. The snapping is modelled anyway because it is
/// what the code says, and because the accumulation above is not something to rediscover if a
/// caller ever passes `true` — but no current behaviour depends on it.
pub fn positions(
    centre: (i32, i32),
    counts: (i32, i32),
    pitch: (i32, i32),
    offset: (i32, i32),
    snap_x: &dyn Fn(i32) -> i32,
    snap_y: &dyn Fn(i32) -> i32,
) -> Vec<(i32, i32)> {
    let (cols, rows) = counts;
    if cols <= 0 || rows <= 0 {
        return Vec::new();
    }
    let span = ((cols - 1) * pitch.0, (rows - 1) * pitch.1);
    let origin = (centre.0 - span.0 / 2, centre.1 - span.1 / 2);
    let col_offset = if cols > 1 { offset.0 } else { 0 };
    let row_offset = if rows > 1 { offset.1 } else { 0 };

    let mut out = Vec::with_capacity((cols * rows) as usize);
    let mut row = origin.1 + row_offset;
    for _ in 0..rows {
        let row_pos = snap_y(row);
        let mut col = origin.0 + col_offset;
        for _ in 0..cols {
            let col_pos = snap_x(col);
            out.push((col_pos, row_pos));
            col = col_pos + pitch.0;
        }
        row = row_pos + pitch.1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(v: i32) -> i32 {
        v
    }

    #[test]
    fn the_count_is_the_span_divided_by_the_pitch() {
        assert_eq!(counts((1000, 500), (300, 200), 0, 0), (3, 2));
    }

    #[test]
    fn a_span_shorter_than_one_pitch_still_gets_one_position() {
        // ⚠️ Floored at one, so a via is always placed — the array is never empty.
        assert_eq!(counts((100, 100), (300, 300), 0, 0), (1, 1));
    }

    #[test]
    fn a_limit_caps_the_count_and_zero_means_unlimited() {
        assert_eq!(counts((1000, 1000), (100, 100), 3, 4), (3, 4));
        assert_eq!(counts((1000, 1000), (100, 100), 0, 0), (10, 10));
    }

    #[test]
    fn the_array_is_centred_on_the_placement_point() {
        let p = positions((1000, 500), (3, 1), (200, 0), (0, 0), &none, &none);
        assert_eq!(p, vec![(800, 500), (1000, 500), (1200, 500)]);
    }

    #[test]
    fn an_offset_applies_only_where_the_axis_has_more_than_one_position() {
        // One row: the row offset is dropped and the via sits on the centre line.
        let p = positions((1000, 500), (2, 1), (200, 200), (10, 70), &none, &none);
        assert_eq!(p, vec![(910, 500), (1110, 500)]);
    }

    #[test]
    fn snapping_accumulates_along_the_array() {
        // 🔑 The distinguishing behaviour. A grid of 30 against a pitch of 100, starting at 0:
        // each position is snapped and the NEXT is measured from that snapped value, so the array
        // walks 0, 120, 240 rather than the 0, 90, 210 an independently-snapped lattice gives.
        let up30 = |v: i32| ((v + 29) / 30) * 30;
        let p = positions((100, 0), (3, 1), (100, 0), (0, 0), &up30, &none);
        assert_eq!(p, vec![(0, 0), (120, 0), (240, 0)]);

        let independent: Vec<i32> = (0..3).map(|c| up30(0 + c * 100)).collect();
        assert_eq!(independent, vec![0, 120, 210], "an ideal lattice drifts by 30");
    }

    #[test]
    fn a_grid_that_divides_the_pitch_makes_the_two_agree() {
        // ⚠️ Which is exactly why this is easy to get wrong: the common case hides it.
        let up20 = |v: i32| ((v + 19) / 20) * 20;
        let p = positions((200, 0), (3, 1), (100, 0), (0, 0), &up20, &none);
        assert_eq!(p, vec![(100, 0), (200, 0), (300, 0)]);
    }

    #[test]
    fn a_two_dimensional_array_snaps_each_axis_with_its_own_grid() {
        // The unsnapped lattice would be (-50, -50) to (50, 50). x snaps on 10 and y on 25, and
        // both the first position and the step from it move — which is the whole point.
        let x = |v: i32| ((v + 9) / 10) * 10;
        let y = |v: i32| ((v + 24) / 25) * 25;
        let p = positions((0, 0), (2, 2), (100, 100), (0, 0), &x, &y);
        assert_eq!(p, vec![(-40, -25), (60, -25), (-40, 75), (60, 75)]);
    }
}
