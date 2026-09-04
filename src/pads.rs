// SPDX-License-Identifier: Apache-2.0
//! Connecting a ring out to the pad cells — `PadDirectConnectionStraps`.
//!
//! `add_pdn_ring -connect_to_pads` asks for a strap from every pad cell's power pin to whatever
//! ring or stripe of the same net is nearest it. The pads sit outside the core on all four sides,
//! so each one reaches inward along its own edge.
//!
//! 🔑 **A pad strap is a grid COMPONENT like any other**, made after the rings and stripes because
//! it needs somewhere to reach *to*.
//!
//! Nothing here touches a database.

use crate::{Direction, Rect};

/// Which side of the core a pad sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    North,
    South,
    East,
    West,
}

impl Edge {
    /// A pad on the east or west reaches inward horizontally — `isConnectHorizontal`.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Edge::East | Edge::West)
    }
}

/// **P1** — which edge a pad is on, if exactly one.
///
/// ⚠️ **Strictly outside the core, and on ONE side only.** A pad overlapping the core on no side
/// has no edge, and a corner pad qualifies on two — both give `None`, and `canConnect` then refuses
/// the pad rather than guessing which way it faces.
///
pub fn pad_edge(inst: Rect, core: Rect) -> Option<Edge> {
    let mut found = None;
    let mut n = 0;
    for (hit, edge) in [
        (inst.1 > core.3, Edge::North),
        (inst.3 < core.1, Edge::South),
        (inst.2 < core.0, Edge::West),
        (inst.0 > core.2, Edge::East),
    ] {
        if hit {
            n += 1;
            found = Some(edge);
        }
    }
    if n == 1 {
        found
    } else {
        None
    }
}

/// One pin box of a pad, already placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub layer: String,
    pub rect: Rect,
    /// The LAYER's direction, which is what decides whether a pin is usable on this edge.
    pub direction: Direction,
}

/// **P3** — the pins that form a RING around the pad, and the layer a strap over them would use.
///
/// 🔑 **A ring pin spans the whole pad.** `getPinsFormingRing` keeps only boxes whose width or
/// height equals the master's width or height, so a row of such pads carries continuous metal.
///
/// The pins are then taken from the TOPMOST layer that still has any, and the strap runs on the
/// next routing layer above that — over the pad rather than inward from its edge.
///
/// `layers` is every routing layer in order, lowest first; `master` is the master's own size.
/// Returns the surviving pins and the index into `layers` of the layer a strap would use.
///
/// ⚠️ **The refusals live in the caller**, not here: this answers "which pins could form a ring",
/// and whether one may actually be used depends on obstructions and other pins, which this does
/// not see. Building a strap on this answer alone emits straps the reference refuses.
pub fn pins_forming_ring(
    pins: &[Pin],
    master: (i32, i32),
    layers: &[String],
) -> Option<(Vec<Pin>, usize)> {
    let spans = |r: Rect| {
        let (dx, dy) = (r.2 - r.0, r.3 - r.1);
        dx == master.0 || dx == master.1 || dy == master.0 || dy == master.1
    };
    let ring: Vec<&Pin> = pins.iter().filter(|p| spans(p.rect)).collect();
    if ring.is_empty() {
        return None;
    }
    // The topmost layer carrying one, by position in the routing order.
    let top = ring
        .iter()
        .filter_map(|p| layers.iter().position(|l| *l == p.layer))
        .max()?;
    // ⚠️ **The next ROUTING layer above**, which is the one the strap goes on. `layers` holds
    // routing layers only, so that is simply the next index — but it may not exist, and a pad
    // whose ring pin is already on the top routing layer forms no over-pad connection.
    let routing = top + 1;
    if routing >= layers.len() {
        return None;
    }
    let kept: Vec<Pin> = ring
        .into_iter()
        .filter(|p| layers.iter().position(|l| *l == p.layer) == Some(top))
        .cloned()
        .collect();
    Some((kept, routing))
}

/// **P3b** — may a strap actually run over this pad, on `routing`?
///
/// 🔑 **Most of `getPinsFormingRing` is refusals**, and they are why a pad that LOOKS connectable
/// often is not.
///
/// ⟹ Nothing of the master may sit ABOVE the layer the strap would use, and nothing of it may sit
/// ON that layer in the strap's way.
///
/// `obstructions` and `geometry` are `(layer index, rect)` over the same routing order as
/// `pins_forming_ring`; anything on a non-routing layer is not passed here.
///
/// ⚠️ **A refusal is not a failure to build** — it means this pad forms no over-pad connection at
/// all, and the reference then makes none for it. Skipping these emits straps it does not.
pub fn may_run_over_pad(
    routing: usize,
    pins: &[Pin],
    obstructions: &[(usize, Rect)],
    geometry: &[(usize, Rect)],
) -> bool {
    if obstructions.iter().any(|(l, _)| *l > routing) {
        return false;
    }
    for (l, r) in geometry {
        if *l > routing {
            return false;
        }
        if *l == routing && pins.iter().any(|p| overlaps(p.rect, *r)) {
            return false;
        }
    }
    true
}

/// Interior overlap, as the reference's `Rect::intersects` has it.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// **P3c** — where an over-pad strap sits, and how wide.
///
/// 🔑 **The straps on one pad SHARE its width.** `makeShapesOverPads` divides the instance's
/// extent across the connect direction between every connection on that pad.
///
/// ⟹ Each strap is one slot along the pad, `index` places it, and the pin shape is then narrowed
/// onto that slot — `set_ylo(offset - width/2)`, `set_yhi(yMin + width)` for a horizontal connect.
///
/// ⚠️ **A pad carrying many connections builds NONE of them.** The width shrinks with the count,
/// and below the layer's minimum the reference returns without building — so a count that is too
/// high does not merely narrow the straps, it removes them.
///
/// `pin_shape` is the union of the connection's ring pins, already placed. Returns the strap's
/// rect, or `None` where the width does not qualify.
#[allow(clippy::too_many_arguments)]
pub fn over_pad_strap(
    pin_shape: Rect,
    inst: Rect,
    horizontal: bool,
    index: usize,
    count: usize,
    layer_min_width: i32,
    layer_max_width: i32,
    layer_spacing: i32,
    manufacturing_grid: i32,
) -> Option<Rect> {
    // 🔑 **The WIDTH snaps to twice the manufacturing grid; the offset to once.** The third
    // argument of `snapToManufacturingGrid` is a multiplier, not a flag.
    //
    // and `makeShapesOverPads` calls it as `(max_width, false, 2)` for the width and with the
    // default multiplier for the offset. ⚠️ Using one grid for both leaves the width off by up to
    // a grid step and the offset with it — small, and never the same amount twice.
    let snap_by = |v: i32, mult: i32| {
        let grid = manufacturing_grid * mult;
        if grid <= 0 {
            v
        } else {
            (v / grid) * grid
        }
    };
    let _ = snap_by;
    // 🔑 **One rule, in one place.** The width/spacing and the default lane are now
    // `over_pad_lanes` and `default_lane_offset`, because `buildOverPad` needs them separately in
    // order to try other lanes; this keeps the historical single-shot form on top of them rather
    // than restating the arithmetic and letting the two drift.
    let (width, spacing) = over_pad_lanes(
        count, inst, horizontal, layer_min_width, layer_max_width, layer_spacing,
        manufacturing_grid,
    )?;
    let offset =
        default_lane_offset(index, width, spacing, inst, horizontal, manufacturing_grid);
    Some(slot_at_offset(pin_shape, horizontal, offset, width))
}

/// `TechLayer::snapToManufacturingGrid(pos, round_up, multiplier)`.
///
/// ```text
/// const int grid = grid_multiplier * tech->getManufacturingGrid();
/// if (pos % grid != 0) { int round_pos = pos / grid; if (round_up) round_pos += 1; ... }
/// ```
///
/// ⚠️ **Already on the grid is left alone**, whichever way `round_up` points — the adjustment is
/// inside the `pos % grid != 0` test, so this is not `ceil`/`floor` of a division.
/// ⚠️ Integer division truncates toward ZERO, so for a negative `pos` the untaken branch rounds
/// the other way. Pad coordinates here are die coordinates and positive.
pub fn snap_grid(pos: i32, round_up: bool, grid: i32) -> i32 {
    if grid <= 0 || pos % grid == 0 {
        return pos;
    }
    let mut r = pos / grid;
    if round_up {
        r += 1;
    }
    r * grid
}

/// **P1g** — `makeShapeOverPad`: the pin rectangle narrowed onto one lane.
///
/// ```text
/// pin_shape.set_ylo(offset - getWidth() / 2);
/// pin_shape.set_yhi(pin_shape.yMin() + getWidth());
/// ```
///
/// ⚠️ `- width / 2` then `+ width`, the same asymmetry the straps have: for an odd width the extra
/// unit falls on the high side of the lane. ⚠️ And the second edge is set from the FIRST, not from
/// `offset + width / 2`, which differs by one for an odd width.
pub fn slot_at_offset(pin_shape: Rect, horizontal: bool, offset: i32, width: i32) -> Rect {
    let lo = offset - width / 2;
    if horizontal {
        (pin_shape.0, lo, pin_shape.2, lo + width)
    } else {
        (lo, pin_shape.1, lo + width, pin_shape.3)
    }
}

/// **P1b** — `computeOverPadLanes`: the width and spacing every strap on one pad shares.
///
/// ```text
/// const int max_width = inst_width / (2 * (group_size + 1));
/// const int target_width = layer.snapToManufacturingGrid(max_width, false, 2);
/// if (target_width < layer.getMinWidth()) return false;      // build NOTHING
/// width   = std::min(target_width, layer.getMaxWidth());
/// spacing = std::max(width, layer.getSpacing(width));
/// ```
///
/// ⚠️ **`2 * (n + 1)`, not `n`** — the pad keeps room either side of every strap.
/// ⚠️ **The WIDTH snaps to TWICE the manufacturing grid**; the offset snaps to once. The third
/// argument is a multiplier, not a flag.
/// 🔑 **A pad carrying too many connections builds NONE of them**: below the layer minimum the
/// reference returns false, so a high count does not merely narrow the straps, it removes them.
#[allow(clippy::too_many_arguments)]
pub fn over_pad_lanes(
    group_size: usize,
    inst: Rect,
    horizontal: bool,
    layer_min_width: i32,
    layer_max_width: i32,
    layer_spacing: i32,
    manufacturing_grid: i32,
) -> Option<(i32, i32)> {
    let inst_width = if horizontal { inst.3 - inst.1 } else { inst.2 - inst.0 };
    let max_width = inst_width / (2 * (group_size as i32 + 1));
    let target_width = snap_grid(max_width, false, manufacturing_grid * 2);
    if target_width < layer_min_width {
        return None;
    }
    let width = target_width.min(layer_max_width);
    Some((width, width.max(layer_spacing)))
}

/// **P1c** — `getDefaultLaneOffset`: where the strap at `index` would historically have been put.
///
/// ```text
/// const int target_offset = layer.snapToManufacturingGrid(inst_offset + spacing + width / 2, false);
/// return target_offset + index * (spacing + width);
/// ```
///
/// 🔑 This is still the FIRST position tried; the search below only supplies fallbacks. So a
/// design where nothing blocks the pad places exactly where it always did.
pub fn default_lane_offset(
    index: usize,
    width: i32,
    spacing: i32,
    inst: Rect,
    horizontal: bool,
    manufacturing_grid: i32,
) -> i32 {
    let inst_offset = if horizontal { inst.1 } else { inst.0 };
    let target = snap_grid(inst_offset + spacing + width / 2, false, manufacturing_grid);
    target + index as i32 * (spacing + width)
}

/// **P1d** — `buildOverPad`'s lane bounds: the strap has to stay over the pin it connects to.
///
/// ```text
/// lane_min = (is_horizontal ? pin.yMin() : pin.xMin()) + getWidth() / 2;
/// lane_max = (is_horizontal ? pin.yMax() : pin.xMax()) - getWidth() / 2;
/// if (lane_min > lane_max) return false;
/// ```
pub fn lane_range(pin_shape: Rect, horizontal: bool, width: i32) -> Option<(i32, i32)> {
    let (lo, hi) = if horizontal {
        (pin_shape.1, pin_shape.3)
    } else {
        (pin_shape.0, pin_shape.2)
    };
    let (lane_min, lane_max) = (lo + width / 2, hi - width / 2);
    (lane_min <= lane_max).then_some((lane_min, lane_max))
}

/// **P1e** — the positions `buildOverPad` tries, in order.
///
/// ```text
/// const int step = layer.snapToManufacturingGrid(
///     std::max(layer.getMinIncrementStep(), getWidth() / 8), true);
/// for (int offset = snap(lane_min, true); offset < lane_max; offset += step) offsets.push_back(offset);
/// offsets.push_back(snap(lane_max, false));
/// std::ranges::sort(offsets, [preferred](int l, int r) { return abs(l - preferred) < abs(r - preferred); });
/// offsets.insert(offsets.begin(), preferred);
/// ```
///
/// 🔑 **`preferred` is prepended AFTER the sort**, so it is tried first even though it is also
/// somewhere inside the sorted list — the historical position gets first refusal and the walk is
/// the fallback. That is what keeps designs with clear pads bit-identical to the old behaviour.
///
/// ⚠️ **`<` not `<=` in the loop, and `lane_max` is then appended snapped DOWN** — so the top of
/// the range is always a candidate and is never produced by the stride.
///
/// ⛔ **`std::ranges::sort` is NOT stable.** Two offsets equidistant from `preferred` come out in
/// an unspecified order upstream. We sort stably, which is one fixed choice out of the two the
/// reference is entitled to make; if a case ever turns on it, this comment is the place to start.
pub fn lane_offsets(
    lane_min: i32,
    lane_max: i32,
    preferred: i32,
    width: i32,
    manufacturing_grid: i32,
) -> Vec<i32> {
    let min_increment = if manufacturing_grid > 0 { manufacturing_grid } else { 1 };
    let step = snap_grid(min_increment.max(width / 8), true, manufacturing_grid).max(1);
    let mut offsets = Vec::new();
    let mut offset = snap_grid(lane_min, true, manufacturing_grid);
    while offset < lane_max {
        offsets.push(offset);
        offset += step;
    }
    offsets.push(snap_grid(lane_max, false, manufacturing_grid));
    offsets.sort_by_key(|o| (o - preferred).abs());
    offsets.insert(0, preferred);
    offsets
}

/// **P1f** — `buildGroup`'s ordering: hand out positions to the net with the FEWEST connections.
///
/// ```text
/// for (int count = 0; count < member_count; count++) {
///   int next = -1;
///   for (int index = 0; index < member_count; index++) {
///     if (handled[index]) continue;
///     if (next == -1 || connections[nets[index]] < connections[nets[next]]) next = index;
///   }
///   handled[next] = true; ...; connections[nets[next]]++;
/// }
/// ```
///
/// ⛔ **This is the whole point of the change.** Upstream #9994: *"`-connect_to_pads` only adds
/// sporadic connections"* — a real chip came out with ONE VSS connection per side against five
/// VDD, because core straps block the lanes and whichever net was placed first took what was
/// available. Interleaving the nets is what PR #11213 did about it (*"There are now 12 VDD
/// connections and 14 VSS connections"*).
///
/// ⚠️ **`<` is strict, so the FIRST index wins a tie** — with all counts equal this returns the
/// members in their existing order, which is what keeps a single-net pad unchanged.
///
/// `counts` carries in the connections each net has ALREADY made elsewhere on the grid, and is
/// updated as positions are handed out; the returned order indexes `nets`.
pub fn pick_next_member(
    nets: &[String],
    handled: &[bool],
    counts: &std::collections::HashMap<String, i32>,
) -> Option<usize> {
    let mut next: Option<usize> = None;
    for index in 0..nets.len() {
        if handled[index] {
            continue;
        }
        let better = match next {
            None => true,
            Some(k) => {
                counts.get(&nets[index]).copied().unwrap_or(0)
                    < counts.get(&nets[k]).copied().unwrap_or(0)
            }
        };
        if better {
            next = Some(index);
        }
    }
    next
}

/// The order `buildGroup` visits a pad's connections in **when every one of them places**.
///
/// ⛔ **Upstream increments only on SUCCESS**, and the selection is interleaved with the building:
/// `if (!member->buildOverPad(...)) { continue; }` comes BEFORE `addNetConnection()` and before
/// `connections[net]++`. A member that finds no lane therefore leaves its net's count untouched
/// and the next pick is made as though it had never been tried. ⟹ the caller drives
/// `pick_next_member` around its own build, and this convenience exists for the tests and for the
/// case where placement cannot fail.
pub fn balanced_order(nets: &[String], counts: &mut std::collections::HashMap<String, i32>) -> Vec<usize> {
    let n = nets.len();
    let mut handled = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for _ in 0..n {
        let Some(next) = pick_next_member(nets, &handled, counts) else { break };
        handled[next] = true;
        *counts.entry(nets[next].clone()).or_insert(0) += 1;
        order.push(next);
    }
    order
}

/// **P2** — the pins that face the core.
///
/// Two filters, in order.
///
/// 🔑 **Only pins on the instance's INNER edge.** The test is a single coordinate — a north pad
/// keeps the boxes whose `yMin` is at the instance's own `yMin` — so a pin buried in the middle of
/// the pad, or against its outer edge, is not a connection point.
///
/// ⚠️ **And where the pad offers both orientations, only the useful one.** A pad with pins on both
/// a horizontal and a vertical layer keeps just those whose layer runs the way the strap must go.
/// With only one orientation available it is kept whichever way it runs, because a non-preferred
/// direction beats no connection at all.
pub fn pins_facing_core(pins: &[Pin], inst: Rect, edge: Edge) -> Vec<Pin> {
    let on_inner_edge = |r: Rect| match edge {
        Edge::North => inst.1 >= r.1,
        Edge::South => inst.3 <= r.3,
        Edge::West => inst.2 <= r.2,
        Edge::East => inst.0 >= r.0,
    };
    let facing: Vec<&Pin> = pins.iter().filter(|p| on_inner_edge(p.rect)).collect();

    let has_h = facing.iter().any(|p| p.direction == Direction::Horizontal);
    let has_v = facing.iter().any(|p| p.direction == Direction::Vertical);
    let want = if edge.is_horizontal() {
        Direction::Horizontal
    } else {
        Direction::Vertical
    };
    facing
        .into_iter()
        .filter(|p| !(has_h && has_v) || p.direction == want)
        .cloned()
        .collect()
}

/// **P3** — the nearest shape a pin can reach, out of the candidates on one layer.
///
/// The search box is the pin stretched across the whole die on the axis it reaches along, so a
/// candidate qualifies only if it **fully spans the pin** across that axis and runs the right way:
/// a horizontal reach needs a vertical shape to land on, and the reverse.
///
/// ⚠️ **Distance is measured edge to edge and is NOT clamped at zero.** A shape already overlapping
/// the pin scores negative and therefore wins, which is what the reference does.
pub fn closest_target(pin: Rect, edge: Edge, die: Rect, candidates: &[Rect]) -> Option<Rect> {
    let search = if edge.is_horizontal() {
        (die.0, pin.1, die.2, pin.3)
    } else {
        (pin.0, die.1, pin.2, die.3)
    };
    let mut best: Option<(i32, Rect)> = None;
    for &c in candidates {
        let (cw, ch) = (c.2 - c.0, c.3 - c.1);
        let hit = (
            search.0.max(c.0),
            search.1.max(c.1),
            search.2.min(c.2),
            search.3.min(c.3),
        );
        if hit.0 >= hit.2 || hit.1 >= hit.3 {
            continue;
        }
        if edge.is_horizontal() {
            if ch < cw || hit.3 - hit.1 != search.3 - search.1 {
                continue;
            }
        } else if cw < ch || hit.2 - hit.0 != search.2 - search.0 {
            continue;
        }
        let dist = match edge {
            Edge::West => c.0 - pin.2,
            Edge::East => pin.0 - c.2,
            Edge::South => c.1 - pin.3,
            Edge::North => pin.1 - c.3,
        };
        if best.is_none_or(|(d, _)| dist < d) {
            best = Some((dist, c));
        }
    }
    best.map(|(_, r)| r)
}

/// **P5** — `PadDirectConnectionStraps::strapViaIsObstructed`: would this strap's via to its target
/// be blocked on a layer in between?
///
/// 🔑 **Adjacent layers are never obstructed** — there is no layer in between to obstruct them —
/// and a strap that misses its target entirely counts as obstructed rather than as fine.
///
/// ⚠️ **No net filter and no shape-type filter.** Anything standing on an intermediate routing
/// layer where the via would land blocks it, the grid's own straps included.
///
/// ⚠️ **Inclusive geometry throughout.** `odb::Rect::intersects` and `bgi::intersects` both count a
/// shared edge, so a via touching an obstruction's halo is blocked. Read as strict overlap, the
/// via that just grazes a keep-out is declared fine and the strap is left where it cannot connect.
///
/// `obstructions` are `(routing level, rect already bloated by its own spacing)` — the obstruction
/// tree is keyed by `getObstruction()`, not by the bare metal.
pub fn via_is_obstructed(
    shape: Rect,
    target: Rect,
    shape_level: i32,
    target_level: i32,
    obstructions: &[(i32, Rect)],
) -> bool {
    let (lo, hi) = if target_level > shape_level {
        (shape_level, target_level)
    } else {
        (target_level, shape_level)
    };
    if hi - lo <= 1 {
        return false;
    }
    if !touches(shape, target) {
        return true;
    }
    let via = (
        shape.0.max(target.0),
        shape.1.max(target.1),
        shape.2.min(target.2),
        shape.3.min(target.3),
    );
    obstructions
        .iter()
        .any(|(level, rect)| *level > lo && *level < hi && touches(*rect, via))
}

/// Inclusive overlap — a shared edge counts. ⚠️ Not [`overlaps`], which is the interior test the
/// via/shape association uses.
fn touches(a: Rect, b: Rect) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// **P4** — the strap from a pin to the shape it reaches, and the layer's width limit on it.
///
/// The pin's own rect with the one edge facing the core pushed out to the far side of the target,
/// so the strap covers the pin, the gap and the shape it lands on.
///
/// ⚠️ **A max width trims the FAR side, not both.** `set_yhi(yMin + max)` keeps the low edge where
/// it is, so the strap stays anchored to the pin and loses room at the other end.
pub fn strap_to_shape(pin: Rect, target: Rect, edge: Edge, max_width: Option<i32>) -> Rect {
    let mut r = pin;
    match edge {
        Edge::West => r.2 = target.2,
        Edge::East => r.0 = target.0,
        Edge::South => r.3 = target.3,
        Edge::North => r.1 = target.1,
    }
    if let Some(max) = max_width {
        if edge.is_horizontal() {
            if r.3 - r.1 > max {
                r.3 = r.1 + max;
            }
        } else if r.2 - r.0 > max {
            r.2 = r.0 + max;
        }
    }
    r
}

#[cfg(test)]
mod ring_tests {
    use super::*;

    fn pin(layer: &str, r: Rect) -> Pin {
        Pin {
            layer: layer.into(),
            rect: r,
            direction: Direction::Horizontal,
        }
    }

    fn layers() -> Vec<String> {
        ["M1", "M2", "M3"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_pin_spanning_the_pad_forms_a_ring() {
        // The master is 100 x 40; this pin is 100 wide, so it spans it.
        let p = vec![pin("M1", (0, 0, 100, 5))];
        let (kept, routing) = pins_forming_ring(&p, (100, 40), &layers()).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(routing, 1, "the strap goes on the layer ABOVE the pin's");
    }

    #[test]
    fn a_pin_matching_the_other_dimension_also_counts() {
        // ⚠️ Either dimension against either of the master's — a rotated pad is the same pad.
        let p = vec![pin("M1", (0, 0, 5, 100))];
        assert!(pins_forming_ring(&p, (100, 40), &layers()).is_some());
    }

    #[test]
    fn a_short_pin_forms_no_ring() {
        let p = vec![pin("M1", (0, 0, 20, 5))];
        assert!(pins_forming_ring(&p, (100, 40), &layers()).is_none());
    }

    #[test]
    fn the_topmost_layer_wins_and_lower_ones_are_dropped() {
        let p = vec![pin("M1", (0, 0, 100, 5)), pin("M2", (0, 0, 100, 5))];
        let (kept, routing) = pins_forming_ring(&p, (100, 40), &layers()).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].layer, "M2");
        assert_eq!(routing, 2);
    }

    /// `snapToManufacturingGrid` adjusts only when the value is OFF the grid.
    #[test]
    fn a_value_already_on_the_grid_is_left_alone_either_way() {
        assert_eq!(snap_grid(100, true, 10), 100, "on-grid, rounding up changes nothing");
        assert_eq!(snap_grid(100, false, 10), 100);
        assert_eq!(snap_grid(101, true, 10), 110);
        assert_eq!(snap_grid(101, false, 10), 100);
        assert_eq!(snap_grid(101, true, 0), 101, "no manufacturing grid: unchanged");
    }

    /// ⛔ The historical position is tried FIRST even though the sorted walk also contains it.
    #[test]
    fn the_preferred_offset_is_tried_first_and_the_rest_by_distance_from_it() {
        // lane 100..200, preferred 150, width 80 -> step = max(grid, 10) snapped up.
        let offs = lane_offsets(100, 200, 150, 80, 5);
        assert_eq!(offs[0], 150, "the historical lane gets first refusal");
        let rest = &offs[1..];
        let mut d: Vec<i32> = rest.iter().map(|o| (o - 150).abs()).collect();
        let sorted = { let mut c = d.clone(); c.sort(); c };
        assert_eq!(d, sorted, "the fallbacks walk outward from the preferred position");
        d.clear();
        assert!(offs.contains(&200), "lane_max is always a candidate");
        // ⚠️ Probe must be able to fail: a plain ascending sweep would start at lane_min.
        assert_ne!(offs[0], 100, "not simply the bottom of the lane");
    }

    /// ⚠️ `<` in the stride loop, then `lane_max` appended snapped DOWN.
    #[test]
    fn the_top_of_the_lane_is_appended_not_strided_onto() {
        let offs = lane_offsets(0, 100, 0, 80, 7);
        assert!(offs.contains(&snap_grid(100, false, 7)), "lane_max, snapped down");
        assert!(offs.iter().all(|&o| o <= 100), "nothing past the top of the lane");
    }

    /// A strap must stay over its pin, and a pin narrower than the strap has no lane at all.
    #[test]
    fn a_pin_narrower_than_the_strap_offers_no_lane() {
        assert_eq!(lane_range((0, 100, 10, 300), true, 100), Some((150, 250)));
        assert_eq!(lane_range((0, 100, 10, 140), true, 100), None, "40 tall, 100 wide strap");
    }

    /// ⛔ **The whole point of PR #11213** (upstream #9994: one VSS connection against five VDD).
    #[test]
    fn positions_go_to_whichever_net_has_made_the_fewest_connections() {
        use std::collections::HashMap;
        let nets: Vec<String> = ["VDD", "VDD", "VSS", "VDD"].iter().map(|s| s.to_string()).collect();
        let mut counts: HashMap<String, i32> = HashMap::new();
        let order = balanced_order(&nets, &mut counts);
        let picked: Vec<&str> = order.iter().map(|&i| nets[i].as_str()).collect();
        assert_eq!(picked, vec!["VDD", "VSS", "VDD", "VDD"], "interleaved, not VDD VDD VDD VSS");
        assert_eq!(counts["VDD"], 3);
        assert_eq!(counts["VSS"], 1);
    }

    /// The count carried in from the REST of the grid decides who goes first here.
    #[test]
    fn a_net_already_ahead_elsewhere_on_the_grid_yields_the_first_position() {
        use std::collections::HashMap;
        let nets: Vec<String> = ["VDD", "VSS"].iter().map(|s| s.to_string()).collect();
        let mut counts: HashMap<String, i32> = HashMap::from([("VDD".into(), 5), ("VSS".into(), 1)]);
        let order = balanced_order(&nets, &mut counts);
        assert_eq!(order, vec![1, 0], "VSS is behind, so VSS is served first");
        // ⚠️ Probe must be able to fail: with the counts level the order is the natural one.
        let mut level: HashMap<String, i32> = HashMap::new();
        assert_eq!(balanced_order(&nets, &mut level), vec![0, 1], "a tie keeps the existing order");
    }

    /// ⚠️ The width snaps to TWICE the grid; a pad with too many connections builds nothing.
    #[test]
    fn the_shared_lane_width_snaps_to_twice_the_grid_and_can_refuse_outright() {
        assert_eq!(over_pad_lanes(1, (0, 0, 100, 200), true, 10, 999, 5, 10), Some((40, 40)));
        assert_eq!(over_pad_lanes(20, (0, 0, 100, 200), true, 40, 999, 5, 10), None,
                   "below the layer minimum the reference builds NOTHING");
    }

    #[test]
    fn straps_on_one_pad_share_its_width() {
        // A 200-tall pad with two connections: 200 / (2 * 3) = 33, snapped DOWN to 20 — the width
        // snaps to twice the manufacturing grid, so 20 and not 30.
        let a = over_pad_strap((0, 0, 100, 200), (0, 0, 100, 200), true, 0, 2, 10, 999, 5, 10);
        let b = over_pad_strap((0, 0, 100, 200), (0, 0, 100, 200), true, 1, 2, 10, 999, 5, 10);
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.3 - a.1, 20, "width is the shared slot, snapped to 2 x the grid");
        assert_eq!(b.3 - b.1, 20);
        assert!(b.1 > a.1, "index moves the strap along the pad");
        assert_eq!(b.1 - a.1, 40, "by spacing + width each time");
    }

    #[test]
    fn the_width_snaps_to_twice_the_grid_and_the_offset_to_once() {
        // ⚠️ 200 / (2 * 2) = 50, which snaps DOWN to 40 on a grid of 20 — not to 50.
        let r = over_pad_strap((0, 0, 100, 200), (0, 0, 100, 200), true, 0, 1, 10, 999, 5, 20)
            .unwrap();
        assert_eq!(r.3 - r.1, 40, "width snapped to 2 x 20");
    }

    #[test]
    fn too_many_connections_build_nothing() {
        // ⚠️ Below the layer minimum the reference returns without building, so a crowded pad
        // loses its straps entirely rather than getting thin ones.
        assert!(over_pad_strap((0, 0, 100, 200), (0, 0, 100, 200), true, 0, 20, 40, 999, 5, 10)
            .is_none());
    }

    #[test]
    fn a_vertical_connect_slots_along_x() {
        let r = over_pad_strap((0, 0, 200, 100), (0, 0, 200, 100), false, 0, 1, 10, 999, 5, 10)
            .unwrap();
        assert_eq!(r.2 - r.0, 40, "200 / (2 * 2) = 50, snapped to 2 x the grid");
        assert_eq!((r.1, r.3), (0, 100), "the other axis keeps the pin shape");
    }

    #[test]
    fn the_layer_maximum_caps_the_width() {
        let r = over_pad_strap((0, 0, 100, 800), (0, 0, 100, 800), true, 0, 1, 10, 60, 5, 10)
            .unwrap();
        assert_eq!(r.3 - r.1, 60, "800 / 4 = 200, capped at the layer maximum");
    }

    #[test]
    fn nothing_may_sit_above_the_strap_layer() {
        let p = vec![pin("M1", (0, 0, 100, 5))];
        assert!(!may_run_over_pad(1, &p, &[(2, (0, 0, 10, 10))], &[]));
        assert!(!may_run_over_pad(1, &p, &[], &[(2, (0, 0, 10, 10))]));
    }

    #[test]
    fn something_on_the_strap_layer_refuses_only_where_it_is_in_the_way() {
        let p = vec![pin("M1", (0, 0, 100, 5))];
        // Clear of the pin: allowed.
        assert!(may_run_over_pad(1, &p, &[], &[(1, (0, 50, 10, 60))]));
        // Across it: refused.
        assert!(!may_run_over_pad(1, &p, &[], &[(1, (10, 0, 20, 5))]));
    }

    #[test]
    fn below_the_strap_layer_never_refuses() {
        // ⚠️ Only ABOVE and ON matter; a pad is full of metal below and none of it obstructs.
        let p = vec![pin("M2", (0, 0, 100, 5))];
        assert!(may_run_over_pad(2, &p, &[(0, (0, 0, 100, 100))], &[(1, (0, 0, 100, 100))]));
    }

    #[test]
    fn a_ring_pin_on_the_top_routing_layer_has_nowhere_to_go() {
        // ⚠️ There is no layer above M3 to run the strap on, so this is not an over-pad
        // connection at all — not a connection on M3.
        let p = vec![pin("M3", (0, 0, 100, 5))];
        assert!(pins_forming_ring(&p, (100, 40), &layers()).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE: Rect = (1000, 1000, 9000, 9000);

    #[test]
    fn a_pad_outside_one_side_takes_that_edge() {
        assert_eq!(pad_edge((3000, 9500, 4000, 9900), CORE), Some(Edge::North));
        assert_eq!(pad_edge((3000, 100, 4000, 500), CORE), Some(Edge::South));
        assert_eq!(pad_edge((100, 3000, 500, 4000), CORE), Some(Edge::West));
        assert_eq!(pad_edge((9500, 3000, 9900, 4000), CORE), Some(Edge::East));
    }

    #[test]
    fn a_corner_pad_qualifies_twice_and_is_refused() {
        // ⚠️ Two edges is as good as none: nothing says which way it faces.
        assert_eq!(pad_edge((100, 9500, 500, 9900), CORE), None);
    }

    #[test]
    fn a_pad_overlapping_the_core_has_no_edge() {
        assert_eq!(pad_edge((2000, 2000, 3000, 3000), CORE), None);
    }

    fn pin(layer: &str, rect: Rect, direction: Direction) -> Pin {
        Pin {
            layer: layer.into(),
            rect,
            direction,
        }
    }

    #[test]
    fn only_pins_on_the_inner_edge_face_the_core() {
        let inst: Rect = (3000, 9500, 4000, 9900);
        let pins = [
            pin("m1", (3100, 9500, 3200, 9600), Direction::Vertical), // at the inner edge
            pin("m1", (3300, 9600, 3400, 9700), Direction::Vertical), // buried
            pin("m1", (3500, 9800, 3600, 9900), Direction::Vertical), // outer edge
        ];
        let out = pins_facing_core(&pins, inst, Edge::North);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rect, (3100, 9500, 3200, 9600));
    }

    #[test]
    fn with_both_orientations_only_the_useful_one_is_kept() {
        let inst: Rect = (100, 3000, 500, 4000);
        let pins = [
            pin("mh", (400, 3000, 500, 3100), Direction::Horizontal),
            pin("mv", (400, 3200, 500, 3300), Direction::Vertical),
        ];
        // Reaching west means a horizontal strap, so the horizontal layer wins.
        let out = pins_facing_core(&pins, inst, Edge::West);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].layer, "mh");
    }

    #[test]
    fn with_one_orientation_it_is_kept_whichever_way_it_runs() {
        // ⚠️ A non-preferred direction beats no connection at all.
        let inst: Rect = (100, 3000, 500, 4000);
        let pins = [pin("mv", (400, 3200, 500, 3300), Direction::Vertical)];
        assert_eq!(pins_facing_core(&pins, inst, Edge::West).len(), 1);
    }

    #[test]
    fn the_target_must_span_the_pin_and_run_the_other_way() {
        let die: Rect = (0, 0, 10000, 10000);
        let p: Rect = (400, 3200, 500, 3300);
        // A vertical ring segment spanning the pin's y: usable.
        let good: Rect = (1500, 0, 1700, 10000);
        // A vertical segment that stops short of the pin's y: not usable.
        let short: Rect = (1500, 3250, 1700, 10000);
        // A horizontal shape: wrong way round for a horizontal reach.
        let flat: Rect = (0, 3200, 10000, 3300);
        assert_eq!(closest_target(p, Edge::West, die, &[good]), Some(good));
        assert_eq!(closest_target(p, Edge::West, die, &[short]), None);
        assert_eq!(closest_target(p, Edge::West, die, &[flat]), None);
    }

    #[test]
    fn the_nearest_of_several_targets_wins() {
        let die: Rect = (0, 0, 10000, 10000);
        let p: Rect = (400, 3200, 500, 3300);
        let near: Rect = (1500, 0, 1700, 10000);
        let far: Rect = (4000, 0, 4200, 10000);
        assert_eq!(
            closest_target(p, Edge::West, die, &[far, near]),
            Some(near)
        );
    }

    #[test]
    fn a_strap_runs_from_the_pin_to_the_far_side_of_its_target() {
        let p: Rect = (400, 3200, 500, 3300);
        let target: Rect = (1500, 0, 1700, 10000);
        assert_eq!(
            strap_to_shape(p, target, Edge::West, None),
            (400, 3200, 1700, 3300)
        );
    }

    #[test]
    fn a_max_width_trims_the_far_side_only() {
        let p: Rect = (400, 3200, 500, 3400);
        let target: Rect = (1500, 0, 1700, 10000);
        // 200 tall, limited to 120: the low edge stays put.
        assert_eq!(
            strap_to_shape(p, target, Edge::West, Some(120)),
            (400, 3200, 1700, 3320)
        );
    }
}

#[cfg(test)]
mod refine_tests {
    use super::*;

    // metal8 target, metal10 strap, metal9 in between.
    fn call(shape: Rect, target: Rect, obs: &[(i32, Rect)]) -> bool {
        via_is_obstructed(shape, target, 10, 8, obs)
    }

    #[test]
    fn adjacent_layers_have_nothing_in_between() {
        // Even with an obstruction sitting exactly on the via, levels 9 and 10 have no layer
        // between them to carry it.
        let obs = [(9, (0, 0, 100, 100))];
        assert!(!via_is_obstructed((0, 0, 100, 10), (0, 0, 100, 10), 10, 9, &obs));
    }

    #[test]
    fn a_strap_that_misses_its_target_is_obstructed_not_fine() {
        assert!(call((0, 0, 10, 10), (50, 50, 60, 60), &[]));
    }

    #[test]
    fn a_clear_intermediate_layer_lets_the_via_through() {
        assert!(!call((0, 0, 100, 10), (90, 0, 110, 10), &[]));
    }

    #[test]
    fn only_the_layers_strictly_between_are_consulted() {
        let via = (90, 0, 100, 10);
        // On the strap's own layer and on the target's, an obstruction is not this test's business.
        assert!(!call((0, 0, 100, 10), (90, 0, 110, 10), &[(10, via)]));
        assert!(!call((0, 0, 100, 10), (90, 0, 110, 10), &[(8, via)]));
        assert!(call((0, 0, 100, 10), (90, 0, 110, 10), &[(9, via)]));
    }

    // ⛔ **The reference's own answer is obstructed, and it places the strap there anyway.**
    //
    // On a flipchip design that connects over its pads, the metal10 pad strap at the north-east
    // corner. Its original slot is obstructed, so `refineShapes` slides it along the pin — and
    // takes the very first position, the pin's low edge, whose via is obstructed by the same thing
    // for the same reason.
    //
    // 🔑 **Because the re-check cannot fire.** `refineShape` builds each candidate as
    // `shape->copy()` and calls `strapViaIsObstructed(new_shape.get(), ..., true)`, which opens
    // with `target_shapes_.find(shape)` on a `std::map<Shape*, Shape*>` that only
    // `makeShapesOverPads` fills. The copy is not a key in it, so the lookup misses and the
    // function returns false on its first line.
    //
    // ⟹ **A refined candidate is judged by its CUT alone.** These two tests exist to keep the
    // measurement: the position is genuinely blocked, so anyone reinstating the test will see this
    // pass and the case lose its shape rather than gain one.
    //
    // The obstruction is the metal9 ring, bloated by its own spacing. Nangate45's metal9 states
    // `SPACINGTABLE PARALLELRUNLENGTH 0.0 2.7 4.0 / WIDTH 1.5 → 0.8 0.9 1.5`, so a 5 um ring
    // running the height of the die takes 1.5 um, and 5604000 less 3000 is 5601000.
    const CORNER_TARGET: Rect = (5603680, 386400, 5613680, 5614000); // the metal8 ring, east side
    const METAL9_RING: Rect = (383140, 5601000, 5616680, 5617000); // the north ring, bloated

    #[test]
    fn the_position_the_reference_refines_to_is_itself_obstructed() {
        // The reference's answer: (5603680, 5600830) - (5902000, 5600830), width 1660.
        let refined = (5603680, 5600000, 5902000, 5601660);
        assert!(call(refined, CORNER_TARGET, &[(9, METAL9_RING)]));
    }

    #[test]
    fn and_so_is_every_other_position_along_that_pin() {
        // The pin is 5600000 .. 5610000 and the strap is 1660 wide, so the search runs to 5608340
        // and the ring's keep-out reaches down to 5601000. Nothing in the range is clear, which is
        // why applying the test costs the shape outright rather than moving it.
        for at in [5600000, 5602000, 5605000, 5608340] {
            let candidate = (5603680, at, 5902000, at + 1660);
            assert!(
                call(candidate, CORNER_TARGET, &[(9, METAL9_RING)]),
                "expected {at} to be obstructed"
            );
        }
    }

    #[test]
    fn a_shared_edge_blocks_it() {
        // The obstruction's bloated rect ends exactly where the via begins.
        assert!(call((0, 0, 100, 10), (90, 0, 110, 10), &[(9, (80, 0, 90, 10))]));
        // One unit clear, and it does not.
        assert!(!call((0, 0, 100, 10), (90, 0, 110, 10), &[(9, (80, 0, 89, 10))]));
    }
}
