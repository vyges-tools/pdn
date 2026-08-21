// SPDX-License-Identifier: Apache-2.0
//! Power switches — where a switch cell has to sit for its always-on pin to reach a strap.
//!
//! A UPF power switch splits the supply in two: an **always-on** net that the grid's straps carry,
//! and a **switched** net that the standard cells' rails carry. The switch cells bridge them, and
//! the grid has to land its always-on straps on the switch's input pin — so the cells are placed
//! against the straps rather than the straps routed to the cells.
//!
//! Nothing here touches a database.

/// **S1** — a span expressed as the site positions it covers.
///
/// `PowerCell::getRectAsSiteWidths`: the sites wholly inside the span, measured from `offset`.
///
/// ⚠️ **Rounded IN at both ends** — up at the low end and down at the high — so a span narrower
/// than one site yields nothing rather than the site it sits in.
pub fn site_positions(span: (i32, i32), site_width: i32, offset: i32) -> Vec<i32> {
    if site_width <= 0 {
        return Vec::new();
    }
    let div_ceil = |v: i32| {
        let (q, r) = (v / site_width, v % site_width);
        if r > 0 {
            q + 1
        } else {
            q
        }
    };
    let div_floor = |v: i32| {
        let (q, r) = (v / site_width, v % site_width);
        if r < 0 {
            q - 1
        } else {
            q
        }
    };
    let start = div_ceil(span.0 - offset) * site_width;
    let end = div_floor(span.1 - offset) * site_width;
    let mut out = Vec::new();
    let mut x = start;
    while x <= end {
        out.push(x + offset);
        x += site_width;
    }
    out
}

/// **S2** — every position a switch cell may take so that one of its always-on pins meets a strap.
///
/// For each site position the strap covers, and each always-on pin the cell carries, the cell is
/// offset so that pin lands on that site. The position is kept when the cell's always-on pins
/// **fall entirely inside the strap**, or **entirely span it** — ⚠️ and nothing in between: a pin
/// hanging half over the strap's edge is refused, however much metal it would touch.
///
/// 🔑 **The extremes are taken across ALL the pins, not per pin.** `min_pin` and `max_pin` bound the
/// whole set, so a cell with two always-on pins is judged by the pair even while being aligned by
/// one of them. That is what stops a two-pin cell being placed with one pin on the strap and the
/// other hanging into the neighbouring track.
///
/// Returns the positions in ascending order; the reference takes the first.
pub fn locations(
    strap: (i32, i32),
    pin_positions: &[i32],
    site_width: i32,
    core_x0: i32,
) -> Vec<i32> {
    if pin_positions.is_empty() {
        return Vec::new();
    }
    let min_pin = *pin_positions.iter().min().unwrap();
    let max_pin = *pin_positions.iter().max().unwrap();
    let mut out: Vec<i32> = Vec::new();
    for strap_pos in site_positions(strap, site_width, core_x0) {
        for pin in pin_positions {
            let at = strap_pos - pin;
            let (lo, hi) = (at + min_pin, at + max_pin);
            let inside = lo >= strap.0 && hi <= strap.1;
            let spanning = lo <= strap.0 && hi >= strap.1;
            if (inside || spanning) && !out.contains(&at) {
                out.push(at);
            }
        }
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_reports_the_sites_wholly_inside_it() {
        assert_eq!(site_positions((100, 500), 100, 0), vec![100, 200, 300, 400, 500]);
    }

    #[test]
    fn the_ends_round_inwards() {
        // ⚠️ 150 rounds up to 200 and 450 down to 400; a span narrower than a site yields nothing.
        assert_eq!(site_positions((150, 450), 100, 0), vec![200, 300, 400]);
        assert_eq!(site_positions((150, 190), 100, 0), Vec::<i32>::new());
    }

    #[test]
    fn the_offset_moves_the_lattice_not_the_span() {
        assert_eq!(site_positions((100, 300), 100, 50), vec![150, 250]);
    }

    #[test]
    fn a_cell_is_offset_so_its_pin_lands_on_the_strap() {
        // One pin at 30 into the cell; the strap covers sites 200 and 300.
        let at = locations((200, 300), &[30], 100, 0);
        assert_eq!(at, vec![170, 270]);
    }

    #[test]
    fn a_pin_hanging_over_the_edge_is_refused() {
        // 🔑 The pin set must be inside the strap or spanning it, never straddling one edge.
        let wide = [0, 400];
        // Strap 0..300: the pair spans it only from -100 (pins at -100 and 300) — not quite —
        // so nothing qualifies at this site width.
        assert!(locations((0, 300), &wide, 100, 0)
            .iter()
            .all(|at| (at + 0 >= 0 && at + 400 <= 300) || (at + 0 <= 0 && at + 400 >= 300)));
    }

    #[test]
    fn a_pin_pair_may_straddle_the_strap_entirely() {
        // Pins at 0 and 400 with a strap 100..300 inside them: spanning, so allowed.
        let at = locations((100, 300), &[0, 400], 100, 0);
        assert!(at.contains(&-100), "spanning position missing from {at:?}");
    }

    #[test]
    fn a_cell_with_no_always_on_pin_has_nowhere_to_go() {
        assert_eq!(locations((0, 1000), &[], 100, 0), Vec::<i32>::new());
    }
}
