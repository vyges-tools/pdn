// SPDX-License-Identifier: Apache-2.0
//! Where vias go — finding the candidates and thinning them.
//!
//! A via belongs wherever two shapes on connected layers overlap on the same net. Finding those
//! places is the easy half; the hard half is the order in which the candidates are then thrown
//! away, because each removal changes what the next test sees.
//!
//! This module decides **where**. What via is actually built there is a separate question.
//!
//! Nothing here touches a database.

use crate::Rect;

/// A shape a via can land on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub layer: String,
    pub net: String,
    pub rect: Rect,
}

/// A declared connection between two layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub lower: String,
    pub upper: String,
    /// Layers a via between these two must pass through, which is where an obstruction blocks it.
    pub intermediate: Vec<String>,
}

/// A candidate via.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Via {
    pub net: String,
    pub area: Rect,
    /// The two shapes themselves, kept alongside their intersection.
    ///
    /// 🔑 **The via's metal is sized against these, not against `area`.** `stack_rects` hands the
    /// stack's two ENDS their own shape and only the levels in between the intersection, so a
    /// two-layer connect never sees the intersection at all. Sizing both ends from `area` clips
    /// every patch to the overlap, and a patch that cannot reach past its shape never widens it.
    pub lower_rect: Rect,
    pub upper_rect: Rect,
    pub lower: String,
    pub upper: String,
    /// Which `Connect` produced it — the declaration order that decides ties later.
    pub connect: usize,
}

/// Why a candidate was thrown away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    Obstructed,
    Overlapping,
}

/// **V5** — put a connect's two layers the right way round.
///
/// ⚠️ **A connect is normalised on construction**, so `{metal6 metal1}` and `{metal1 metal6}` are
/// the same connect. Comparing a via's layers against the pair as written would match only one of
/// the two spellings.
pub fn normalise(lower: (String, i32), upper: (String, i32)) -> (String, String) {
    if lower.1 > upper.1 {
        (upper.0, lower.0)
    } else {
        (lower.0, upper.0)
    }
}

/// **V6** — the routing layers a via stack must pass through.
///
/// Everything strictly between the two ends, by layer **number** — which interleaves routing and
/// cut layers, so the routing ones are picked out afterwards.
///
/// ⚠️ **A layer carrying a LEF58 type is skipped entirely.** Those are neither routing nor cut
/// layers in the sense this cares about, and treating one as an intermediate routing layer inserts
/// a via into a stack that has nowhere to put it.
///
/// `layers` is `(name, number, routing_level, has_lef58_type)` in number order.
pub fn intermediate_routing(
    layers: &[(String, i32, i32, bool)],
    lower_number: i32,
    upper_number: i32,
) -> Vec<String> {
    let routing: Vec<&str> = layers
        .iter()
        .filter(|(_, _, level, _)| *level != 0)
        .map(|(name, ..)| name.as_str())
        .collect();
    intermediate_layers(layers, lower_number, upper_number)
        .into_iter()
        .filter(|n| routing.contains(&n.as_str()))
        .collect()
}

/// **V11** — every layer a via stack passes through, cut layers included.
///
/// ⚠️ **This is a DIFFERENT set from [`intermediate_routing`], and the difference matters.**
/// `Connect`'s constructor walks every layer number strictly between the two ends and keeps all of
/// them, skipping only those carrying a LEF58 type. Cut layers are in. The routing-only set is
/// what the via *stack* is built from; this fuller set is what the obstruction test uses, and
/// using the narrow one there silently lets a via through a blocked cut layer.
pub fn intermediate_layers(
    layers: &[(String, i32, i32, bool)],
    lower_number: i32,
    upper_number: i32,
) -> Vec<String> {
    layers
        .iter()
        .filter(|(_, n, _, _)| *n > lower_number && *n < upper_number)
        .filter(|(_, _, _, lef58)| !lef58)
        .map(|(name, _, _, _)| name.clone())
        .collect()
}

/// **V7** — the rectangle each layer of a stack gets.
///
/// ⚠️ **The two ends keep their own shapes; every layer between takes the INTERSECTION.** A stack
/// is only as wide as the narrowest thing it passes through, and giving an intermediate layer
/// either end's rectangle builds metal there that nothing asked for.
///
/// The result has one entry per routing layer, so a stack through `n` intermediate layers has
/// `n + 2` rectangles and `n + 1` vias between them.
pub fn stack_rects(lower: Rect, upper: Rect, intermediate: usize) -> Vec<Rect> {
    let mid = intersect(lower, upper);
    let mut out = Vec::with_capacity(intermediate + 2);
    out.push(lower);
    out.extend(std::iter::repeat_n(mid, intermediate));
    out.push(upper);
    out
}

/// **V15** — a stack whose middle levels are widened to what each layer can take.
///
/// 🔑 **`Connect::makeVia` has two ways of laying out a stack**, and the plain one is only the
/// second.
///
/// A stack is **complex** when any intermediate routing layer's minimum width is wider than the
/// intersection is narrow — a rail 18 units wide crossing a strap, with layers between that cannot
/// hold anything so thin. Each such layer's rect is then grown to what *it* needs, so the stack
/// tapers out in the middle and back in at the ends rather than failing.
///
/// ⚠️ **Grown about the CENTRE, and unevenly.** The shortfall is split `min_add / 2` on the low side
/// and the remainder on the high, so an odd shortfall favours the high side. Then the low edge
/// snaps **down** and the high edge **up** — the rect can only get wider for the snap, never
/// narrower.
///
/// ⚠️ **Per axis, and only where short.** A layer already wide enough on an axis is untouched there.
///
/// 🔑 **The GATE and the GROWTH use different widths, and the gate is the smaller.**
/// `isComplexStackedVia` asks whether any intermediate layer's **raw** `getMinWidth()` exceeds the
/// intersection; `generateComplexStackedViaRects` then grows to `Connect::getMinWidth()`, which is
/// that plus twice the worst enclosure. So a stack can clear the gate and still be narrower than
/// the growth target — and the reference leaves it alone.
///
/// ⚠️ **Applying the growth unconditionally is therefore NOT the same as the branch.** Doing so
/// widens stacks the reference never touches: it cost nine shapes in each of the six power-switch
/// cases, all of which had been exact.
///
/// `raw_min_widths` gates; `min_widths` grows. Both are one entry per intermediate layer, in order.
pub fn stack_rects_tapered(
    lower: Rect,
    upper: Rect,
    raw_min_widths: &[i32],
    min_widths: &[i32],
    manufacturing_grid: i32,
) -> Vec<Rect> {
    let mid = intersect(lower, upper);
    let narrow = (mid.2 - mid.0).min(mid.3 - mid.1);
    let complex = raw_min_widths.iter().any(|w| *w > narrow);
    let mut out = Vec::with_capacity(min_widths.len() + 2);
    out.push(lower);
    for (i, _) in min_widths.iter().enumerate() {
        out.push(if complex {
            grow_to_min_width(mid, min_widths[i], manufacturing_grid)
        } else {
            mid
        });
    }
    out.push(upper);
    out
}

/// **V16** — one rect grown to a minimum width on both axes, as `adjust_rect` does.
pub fn grow_to_min_width(rect: Rect, min_width: i32, manufacturing_grid: i32) -> Rect {
    let snap = |v: i32, up: bool| {
        crate::straps::snap_to_manufacturing_grid(v, manufacturing_grid, up)
    };
    let (mut x0, mut y0, mut x1, mut y1) = rect;
    if x1 - x0 < min_width {
        let add = min_width - (x1 - x0);
        let low = add / 2;
        x0 = snap(x0 - low, false);
        x1 = snap(x1 + (add - low), true);
    }
    if y1 - y0 < min_width {
        let add = min_width - (y1 - y0);
        let low = add / 2;
        y0 = snap(y0 - low, false);
        y1 = snap(y1 + (add - low), true);
    }
    (x0, y0, x1, y1)
}

/// **V17** — the second rect every intermediate level carries.
///
/// 🔑 **A level of a stack holds a SET of candidate rects, not one.** `makeSingleLayerVia` takes
/// `lower_rects` and `upper_rects`, crosses every pair with every rule, builds them all and keeps
/// the best — so a level that offers two shapes gets two chances to find an enclosure that fits.
///
/// This is the second one: the level's rect narrowed to exactly the layer's own width across the
/// preferred direction, centred on where it already was.
///
/// ⚠️ **Exactly the width, not at least it.** The span is *assigned*, so this candidate is narrower
/// than the rect on a wide intersection and WIDER on a thin one. It is not a shrink.
///
/// ⚠️ **The layer's `getWidth`, which is not `getMinWidth`.** The taper grows to the latter plus
/// enclosures; this narrows to the former. A technology that states them differently gets two
/// genuinely different candidates, which is the entire point of keeping both.
pub fn min_enclosure_rect(rect: Rect, layer_width: i32, horizontal: bool) -> Rect {
    let (x0, y0, x1, y1) = rect;
    if horizontal {
        let centre = y0 + (y1 - y0) / 2;
        let lo = centre - layer_width / 2;
        (x0, lo, x1, lo + layer_width)
    } else {
        let centre = x0 + (x1 - x0) / 2;
        let lo = centre - layer_width / 2;
        (lo, y0, lo + layer_width, y1)
    }
}

/// **V18** — `generateMinEnclosureViaRects`: give every intermediate level its second candidate.
///
/// Runs on BOTH stack layouts, the plain one and the tapered one, so a multi-level stack always
/// offers two rects per intermediate level even where nothing tapered.
///
/// ⚠️ **Union, not replacement** — unless the connect named the layer under `-min_width`, in which
/// case the narrowed rect is all that is left. The reference's reason is routing, not vias: a level
/// carrying no strap of its own would otherwise be filled to the full stripe and block the tracks
/// beside it.
///
/// The ends are untouched. `lower` and `upper` are the shapes themselves and there is no second
/// version of a shape.
pub fn add_min_enclosure_rects(
    stack: &mut [Vec<Rect>],
    layer_widths: &[i32],
    horizontal: &[bool],
    min_width_only: &[bool],
) {
    for (i, &width) in layer_widths.iter().enumerate() {
        let Some(level) = stack.get_mut(i + 1) else {
            break; // fewer levels than intermediate layers: nothing to widen
        };
        let narrowed: Vec<Rect> = level
            .iter()
            .map(|r| min_enclosure_rect(*r, width, horizontal[i]))
            .collect();
        if min_width_only.get(i).copied().unwrap_or(false) {
            *level = narrowed;
        } else {
            level.extend(narrowed);
        }
        // A `std::set`: sorted by (x0, y0, x1, y1) and free of duplicates. The order is not
        // cosmetic — generators are constructed in it, and a tie in the preference sort is broken
        // by construction order.
        level.sort_unstable();
        level.dedup();
    }
}

/// **V9** — the rect a via is actually built in, given the two shapes it joins.
///
/// 🔑 **The plain intersection of the two**, which is all the reference does.
///
/// The rect names the via, decides how many cuts fit, and is what `generateViaRects` gives the
/// levels between a stack's two ends.
///
/// ⚠️ **This used to take each axis from the shape that CONSTRAINS it — x from the vertical shape,
/// y from the horizontal — leaving the rect UNCLIPPED.** The two rules agree whenever each shape
/// spans the other on the axis in question, which every case that motivated the old one did: a via
/// named `2000_340` is the strap's width by the rail's *and* their overlap, and neither reading is
/// distinguishable from it.
///
/// 🔑 **They diverge only where one shape ENDS inside the other**, and then the unclipped rect is
/// far too big. A repair strap starting part-way up a strap it crosses got the crossing shape's
/// full width instead of the 126 they actually share — enough to turn a via the reference builds
/// as a 2-row array into a single row whose metal overhangs the strap it sits on.
pub fn via_area(lower: Rect, upper: Rect) -> Rect {
    intersect(lower, upper)
}

/// **V10** — is one end of a via still held by a shape after trimming?
///
/// 🔑 **Trimming invalidates vias, and a shrink does it as surely as a removal.**
/// `GridComponent::replaceShape` removes the old shape first — which nulls that end on every via
/// attached to it — and then re-attaches only those vias whose **area intersects the new rect**.
/// `cleanupVias` then drops any via left with a null end, `Via::isValid` being no more than
/// `lower_ != nullptr && upper_ != nullptr`.
///
/// ⚠️ **The test is at the STACK's ends, not at each level.** The reference models a metal1-to-
/// metal6 connection as ONE `Via` holding the metal1 and metal6 shapes; the levels between it are
/// inside the generated via, not vias of their own. Testing each of our per-level placements
/// against a shape on its own two layers would drop every intermediate level, since no shape is
/// ever emitted on a layer the stack merely passes through.
///
/// ⚠️ Intersection is **closed** here, as `Rect::intersects` is: a via whose area only touches the
/// trimmed shape is still held by it.
pub fn still_held(area: Rect, layer: &str, net: &str, shapes: &[(String, String, Rect)]) -> bool {
    shapes
        .iter()
        .any(|(n, l, r)| n == net && l == layer && intersects(area, *r))
}

/// **V12** — where a via is actually placed, which is not the centre of what it is sized from.
///
/// 🔑 **Two different rects.** [`via_area`] gives the rect the via is BUILT in — each axis from the
/// shape that constrains it. The placement point comes from somewhere else entirely: the plain
/// intersection of the two shapes, **snapped**, and the via sits at its centre.
///
/// From `Connect::makeVia`:
///
/// - `intersection = lower_rect.intersect(upper_rect)` — the plain overlap;
/// - each bound snapped to a multiple of **twice** the manufacturing grid, the mins **up** and the
///   maxes **down**;
/// - `x = round(0.5 * (xMin + xMax))`, and the same for y;
/// - ⚠️ a centre that is **off the manufacturing grid** yields a DUMMY via — nothing is placed at
///   all, and the whole stack is lost with it.
///
/// ⚠️ **The snap is not `ceil`.** `pos / grid` truncates toward zero, so on a negative bound
/// "round up" moves differently than rounding down would:
///
/// ```text
/// grid = 2 x 10 = 20
/// -170 -> -170/20 truncates to -8, +1 = -7, x20 = -140   (a floor-based ceil gives -160)
///  170 ->  170/20 = 8,               x20 =  160
/// ```
///
/// 🔑 That ten-unit shift is not cosmetic. A rail spanning `-170..170` gives a via centred at
/// **10**, so its metal sits ten units above the rail's top — and the write stage then refuses to
/// grow a horizontal rail across its own direction and rips the via out. Centring on the via's
/// own area instead puts the metal exactly inside the rail, nothing is ripped, and the design ends
/// up carrying vias the reference does not.
pub fn placement_point(lower: Rect, upper: Rect, manufacturing_grid: i32) -> Option<(i32, i32)> {
    let i = intersect(lower, upper);
    let snap = |v: i32, up: bool| {
        crate::straps::snap_to_manufacturing_grid(v, manufacturing_grid * 2, up)
    };
    let (x0, y0) = (snap(i.0, true), snap(i.1, true));
    let (x1, y1) = (snap(i.2, false), snap(i.3, false));
    // `std::round` on a half is away from zero, which integer division is not.
    let mid = |a: i32, b: i32| {
        let s = a + b;
        if s % 2 == 0 {
            s / 2
        } else if s > 0 {
            (s + 1) / 2
        } else {
            (s - 1) / 2
        }
    };
    let (x, y) = (mid(x0, x1), mid(y0, y1));
    if manufacturing_grid > 0 && (x % manufacturing_grid != 0 || y % manufacturing_grid != 0) {
        return None; // off grid: the reference builds a dummy via, which places nothing
    }
    Some((x, y))
}

/// **V8** — does this stack need the complex treatment?
///
/// ⚠️ **Any** intermediate routing layer whose minimum width exceeds the intersection's **narrower**
/// dimension makes the stack complex — the simple stack would put sub-minimum metal on that layer.
/// Testing the wider dimension, or the average, passes stacks that cannot be built.
pub fn is_complex(intermediate_min_widths: &[i32], intersection: Rect) -> bool {
    let min_dim = (intersection.2 - intersection.0).min(intersection.3 - intersection.1);
    intermediate_min_widths.iter().any(|w| *w > min_dim)
}

/// **V1** — every place a via could go, in the reference's own order.
///
/// 🔑 **The order is `connect` declaration order, then lower shapes, then the upper-layer query.**
/// It is not an incidental detail: the overlap thinning below keeps the *later* of two equal-sized
/// candidates, so swapping two `add_pdn_connect` statements can change which via survives.
///
/// ⚠️ **Overlap is interior-only.** The reference queries an R-tree with `intersects`, which counts
/// a shared edge, and then rejects the hit with `overlaps`, which does not. Two shapes that merely
/// touch make **no** via. Using either predicate alone gets a different answer — one invents vias
/// on abutting shapes, the other is just slower.
///
/// Shapes must be on the same net; a crossing of two different nets is not a connection.
pub fn intersections(connects: &[Connect], shapes: &[Shape]) -> Vec<Via> {
    let mut out = Vec::new();
    for (i, c) in connects.iter().enumerate() {
        let lower: Vec<&Shape> = shapes.iter().filter(|s| s.layer == c.lower).collect();
        let upper: Vec<&Shape> = shapes.iter().filter(|s| s.layer == c.upper).collect();
        if lower.is_empty() || upper.is_empty() {
            continue;
        }
        for l in &lower {
            for u in &upper {
                if l.net != u.net || !overlaps(l.rect, u.rect) {
                    continue;
                }
                out.push(Via {
                    net: l.net.clone(),
                    area: intersect(l.rect, u.rect),
                    lower_rect: l.rect,
                    upper_rect: u.rect,
                    lower: c.lower.clone(),
                    upper: c.upper.clone(),
                    connect: i,
                });
            }
        }
    }
    out
}

/// **V2** — drop candidates an obstruction sits in the middle of.
///
/// A via passes through its connect's intermediate layers, and anything on one of those layers that
/// touches the via's area blocks it.
///
/// ⚠️ **This runs BEFORE the overlap thinning, and the order matters.** The overlap pass builds its
/// view from what survives here, so a candidate removed for being obstructed never gets the chance
/// to eliminate a smaller legal one. Reversing the two loses vias that should exist.
pub fn remove_obstructed(
    vias: Vec<Via>,
    connects: &[Connect],
    obstructions: &[(String, Rect)],
) -> (Vec<Via>, Vec<(Via, Failed)>) {
    let index = ObstructionIndex::build(obstructions);
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for v in vias {
        let layers = &connects[v.connect].intermediate;
        let blocked = layers.iter().any(|l| index.any_hit(l, v.area));
        if blocked {
            dropped.push((v, Failed::Obstructed));
        } else {
            kept.push(v);
        }
    }
    (kept, dropped)
}

/// The obstructions, bucketed by layer and by position, for [`remove_obstructed`].
///
/// 🔑 **A pure ACCELERATION — the predicate is unchanged.** `any_hit` answers exactly what
/// `obstructions.iter().any(|(l, r)| layer == l && intersects(area, *r))` answers, and the pass is
/// an existence test, so visiting a rect twice or in a different order cannot change the result.
/// ⟹ **Its correctness test is that the output is IDENTICAL, not that it is plausible.**
///
/// ⛔ **Why it exists.** The reference does this test through a boost R-tree —
/// `search_obs.qbegin(bgi::intersects(via->getArea()) && …)` — and we did it by scanning every
/// obstruction for every candidate. On `pads_connect_from_non_pref_edge` that is **22,881
/// candidates against 712,108 obstructions**, and it measured **128 seconds** in a run that has to
/// do it twice, once before `repairVias` and once after. The reference finishes the whole design in
/// seconds. Nothing about the rule was wrong; the loop was.
///
/// **A uniform grid, not an R-tree.** The data is what makes that the right pick: hundreds of
/// thousands of small instance-pin rects spread evenly over a die, queried by small via areas. A
/// grid costs one multiply per lookup and needs no dependency.
///
/// ⚠️ **A rect far larger than a cell would be listed in every cell it spans**, so anything
/// covering more than [`Self::MAX_CELLS_PER_RECT`] cells goes in a per-layer `oversized` list that
/// is scanned linearly. That keeps the memory bounded no matter how a technology mixes a macro
/// blockage with pin-sized ones. There are few such rects in practice, which is why the linear
/// scan for them costs nothing.
struct ObstructionIndex {
    layers: std::collections::HashMap<String, LayerBuckets>,
}

struct LayerBuckets {
    /// The indexed area. Anything outside it cannot be hit, and no cell is allocated for it.
    origin: (i32, i32),
    cell: i64,
    nx: i64,
    ny: i64,
    cells: Vec<Vec<u32>>,
    rects: Vec<Rect>,
    /// Rects spanning too many cells to list, scanned linearly.
    oversized: Vec<Rect>,
}

impl ObstructionIndex {
    /// A rect wider than this many cells on either axis goes to `oversized`.
    const MAX_CELLS_PER_RECT: i64 = 64;

    fn build(obstructions: &[(String, Rect)]) -> Self {
        let mut by_layer: std::collections::HashMap<&str, Vec<Rect>> = Default::default();
        for (l, r) in obstructions {
            by_layer.entry(l.as_str()).or_default().push(*r);
        }
        let mut layers = std::collections::HashMap::new();
        for (l, rects) in by_layer {
            layers.insert(l.to_string(), LayerBuckets::build(rects));
        }
        ObstructionIndex { layers }
    }

    fn any_hit(&self, layer: &str, area: Rect) -> bool {
        match self.layers.get(layer) {
            Some(b) => b.any_hit(area),
            None => false,
        }
    }
}

impl LayerBuckets {
    fn build(rects: Vec<Rect>) -> Self {
        // The bounding box of everything on the layer, and a cell size that puts roughly one rect
        // in each cell. ⚠️ Guarded at 1: a layer whose shapes are all one point would divide by
        // zero, and a single-cell grid is still correct, only slow.
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for r in &rects {
            x0 = x0.min(r.0);
            y0 = y0.min(r.1);
            x1 = x1.max(r.2);
            y1 = y1.max(r.3);
        }
        let (w, h) = ((x1 as i64 - x0 as i64).max(1), (y1 as i64 - y0 as i64).max(1));
        let n = rects.len().max(1) as i64;
        let cell = (((w as f64 * h as f64) / n as f64).sqrt() as i64).max(1);
        let nx = (w / cell + 1).clamp(1, 4096);
        let ny = (h / cell + 1).clamp(1, 4096);
        // ⚠️ The clamp above can leave a cell too small to cover the span, so recompute from the
        // counts actually used rather than trusting the first estimate.
        let cell_x = (w / nx + 1).max(1);
        let cell_y = (h / ny + 1).max(1);
        let cell = cell_x.max(cell_y);
        let nx = (w / cell + 1).max(1);
        let ny = (h / cell + 1).max(1);

        let mut b = LayerBuckets {
            origin: (x0, y0),
            cell,
            nx,
            ny,
            cells: vec![Vec::new(); (nx * ny) as usize],
            rects: Vec::with_capacity(rects.len()),
            oversized: Vec::new(),
        };
        for r in rects {
            let (cx0, cy0, cx1, cy1) = b.span(r);
            if (cx1 - cx0 + 1) > ObstructionIndex::MAX_CELLS_PER_RECT
                || (cy1 - cy0 + 1) > ObstructionIndex::MAX_CELLS_PER_RECT
            {
                b.oversized.push(r);
                continue;
            }
            let at = b.rects.len() as u32;
            b.rects.push(r);
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    b.cells[(cy * b.nx + cx) as usize].push(at);
                }
            }
        }
        b
    }

    /// The cell range a rect covers, clamped into the grid.
    fn span(&self, r: Rect) -> (i64, i64, i64, i64) {
        let cx0 = ((r.0 as i64 - self.origin.0 as i64) / self.cell).clamp(0, self.nx - 1);
        let cy0 = ((r.1 as i64 - self.origin.1 as i64) / self.cell).clamp(0, self.ny - 1);
        let cx1 = ((r.2 as i64 - self.origin.0 as i64) / self.cell).clamp(0, self.nx - 1);
        let cy1 = ((r.3 as i64 - self.origin.1 as i64) / self.cell).clamp(0, self.ny - 1);
        (cx0, cy0, cx1, cy1)
    }

    fn any_hit(&self, area: Rect) -> bool {
        if self.oversized.iter().any(|r| intersects(area, *r)) {
            return true;
        }
        // ⚠️ **Clamping the query into the grid is not enough on its own** — a query wholly outside
        // the indexed area would clamp onto an edge cell and test rects it cannot touch. That is
        // harmless here only because `intersects` is then asked anyway; the clamp is for indexing
        // safety, and the rect test is what decides.
        let (cx0, cy0, cx1, cy1) = self.span(area);
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                for &at in &self.cells[(cy * self.nx + cx) as usize] {
                    if intersects(area, self.rects[at as usize]) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// **V3** — of two overlapping candidates on the same layer pair, keep the larger.
///
/// ⚠️ **Three details, each of which changes the answer:**
///
/// - **The test is `<=`, not `<`.** Two candidates of equal area both satisfy it against each
///   other, so the tie is broken by order rather than by size.
/// - **A candidate is marked failed the moment it loses**, inside the loop, and later candidates
///   see that. So of two equal candidates the **first** dies and the second lives — and for a chain
///   of three equal overlapping candidates, one survives. Computing every removal first and then
///   applying them removes all three.
/// - **Only candidates on the same layer pair compete.** A via between metal1 and metal4 does not
///   thin one between metal4 and metal5 that happens to sit under it.
pub fn remove_overlapping(vias: Vec<Via>) -> (Vec<Via>, Vec<(Via, Failed)>) {
    let mut failed = vec![false; vias.len()];
    for i in 0..vias.len() {
        if failed[i] {
            continue;
        }
        let beaten = (0..vias.len()).any(|j| {
            j != i
                && !failed[j]
                && vias[j].lower == vias[i].lower
                && vias[j].upper == vias[i].upper
                && intersects(vias[i].area, vias[j].area)
                && area(vias[i].area) <= area(vias[j].area)
        });
        if beaten {
            failed[i] = true;
        }
    }
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for (i, v) in vias.into_iter().enumerate() {
        if failed[i] {
            dropped.push((v, Failed::Overlapping));
        } else {
            kept.push(v);
        }
    }
    (kept, dropped)
}

/// **V4** — the whole thinning pipeline, in order.
pub fn place(
    connects: &[Connect],
    shapes: &[Shape],
    obstructions: &[(String, Rect)],
) -> (Vec<Via>, Vec<(Via, Failed)>) {
    let candidates = intersections(connects, shapes);
    let (kept, mut dropped) = remove_obstructed(candidates, connects, obstructions);
    let (kept, more) = remove_overlapping(kept);
    dropped.extend(more);
    (kept, dropped)
}

fn area(r: Rect) -> i64 {
    (r.2 - r.0) as i64 * (r.3 - r.1) as i64
}

/// Interior overlap — a shared edge does not count.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// Closed intersection — a shared edge counts.
fn intersects(a: Rect, b: Rect) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

fn intersect(a: Rect, b: Rect) -> Rect {
    (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3))
}

/// **V13** — the nearest routing track to a position, `TechLayer::snapToGrid`.
///
/// ⚠️ **An empty grid returns the position untouched**, which is the whole of how `-ongrid` is
/// selective: every layer is asked to snap and only the ones named populate a grid, so the call is
/// unconditional at the call site and the answer is a no-op everywhere else.
///
/// 🔑 **It walks ascending and STOPS the moment the distance stops shrinking.** That is a scan for
/// the nearest, not a search — it relies on the track list being sorted, and on a tie it keeps the
/// FIRST, so a position exactly between two tracks snaps to the lower one.
///
/// `greater_than` skips tracks below a floor; the reference defaults it to zero.
pub fn snap_to_grid(pos: i32, grid: &[i32], greater_than: i32) -> i32 {
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

/// **V14** — which of a level's two layers snaps which axis.
///
/// From `DbGenerateStackedVia::generate`:
///
/// ```text
/// if lower is HORIZONTAL:  x <- upper's grid,  y <- lower's grid
/// else:                    x <- lower's grid,  y <- upper's grid
/// ```
///
/// 🔑 **The LOWER layer's direction chooses, and each layer then supplies one axis.** Read either
/// way round it comes to the same thing where routing layers alternate — the horizontal layer
/// supplies y and the vertical one x, because those are the axes their tracks are spaced along —
/// but the reference branches on the lower layer alone, and two layers of the same direction would
/// follow that branch rather than the tidier reading.
///
/// Returns `(layer supplying x, layer supplying y)` as `false` for lower and `true` for upper.
pub fn snap_sources(lower_is_horizontal: bool) -> (bool, bool) {
    if lower_is_horizontal {
        (true, false)
    } else {
        (false, true)
    }
}

/// **V19** — the same question for a TECH via, which answers it differently.
///
/// `DbTechVia::generate` starts from `row_snap = top`, `col_snap = bottom` and swaps the pair only
/// when the **TOP** layer is vertical:
///
/// ```text
/// if top is VERTICAL:  x <- upper's grid,  y <- lower's grid
/// else:                x <- lower's grid,  y <- upper's grid
/// ```
///
/// ⚠️ **It branches on the TOP layer where [`snap_sources`] branches on the lower**, and the two
/// agree only while routing layers alternate. Give both levels the same direction — two follow-pin
/// layers, say — and they choose opposite layers for both axes.
///
/// Returns `(layer supplying x, layer supplying y)` as `false` for lower and `true` for upper.
pub fn techvia_snap_sources(upper_is_vertical: bool) -> (bool, bool) {
    if upper_is_vertical {
        (true, false)
    } else {
        (false, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tech_via_and_a_stack_choose_snap_layers_by_different_layers() {
        // Alternating layers: the two rules agree, which is why one stood in for the other.
        // lower horizontal => upper vertical.
        assert_eq!(snap_sources(true), techvia_snap_sources(true));
        // lower vertical => upper horizontal.
        assert_eq!(snap_sources(false), techvia_snap_sources(false));
        // ⚠️ Two layers of the SAME direction, and they part company. Both horizontal: the stack
        // rule takes x from the upper, the tech via rule takes x from the lower.
        assert_eq!(snap_sources(true), (true, false));
        assert_eq!(techvia_snap_sources(false), (false, true));
    }

    #[test]
    fn a_rect_already_wide_enough_is_left_alone() {
        assert_eq!(grow_to_min_width((0, 0, 100, 100), 50, 1), (0, 0, 100, 100));
    }

    #[test]
    fn a_shortfall_is_split_with_the_remainder_on_the_high_side() {
        // ⚠️ 7 short: 3 low, 4 high. Not symmetric, and not rounded away.
        assert_eq!(grow_to_min_width((0, 0, 3, 100), 10, 1), (-3, 0, 7, 100));
    }

    #[test]
    fn the_snap_truncates_toward_zero_and_can_pull_a_negative_edge_IN() {
        // ⚠️ `snapToManufacturingGrid` is `pos / grid * grid` on C++ integers, which truncates
        // TOWARD ZERO — so "round down" is only down for positives. A low edge at -3 on a grid of
        // 5 comes back as 0, narrowing the rect rather than widening it.
        //
        // 🔑 Faithful, not a defect: the same asymmetry decides the via placement point, and
        // reproducing `floor` here would disagree with the reference on every rect that reaches
        // below the origin.
        assert_eq!(grow_to_min_width((0, 0, 3, 100), 10, 5), (0, 0, 10, 100));
        // Away from zero it behaves as expected.
        assert_eq!(
            grow_to_min_width((1000, 0, 1003, 100), 10, 5),
            (995, 0, 1010, 100)
        );
    }

    #[test]
    fn each_axis_is_judged_on_its_own() {
        // Wide in x, thin in y: only y moves.
        assert_eq!(grow_to_min_width((0, 0, 100, 10), 50, 1), (0, -20, 100, 30));
    }

    #[test]
    fn a_stack_no_layer_finds_too_narrow_is_the_plain_one() {
        // The intersection is 20 across and no layer's raw minimum exceeds it: plain layout.
        let (lower, upper) = ((0, 0, 100, 20), (40, -50, 60, 80));
        let plain = stack_rects(lower, upper, 2);
        let tapered = stack_rects_tapered(lower, upper, &[5, 5], &[60, 60], 1);
        assert_eq!(plain, tapered, "the gate is the RAW width, not the grown one");
    }

    #[test]
    fn the_gate_is_the_raw_width_and_the_growth_is_the_larger_one() {
        // 🔑 The distinction that matters. Raw 30 clears the 20-wide intersection, so the stack IS
        // complex — and it is then grown to 60, not to 30.
        let (lower, upper) = ((0, 0, 100, 20), (40, -50, 60, 80));
        let tapered = stack_rects_tapered(lower, upper, &[30], &[60], 1);
        // ⚠️ Both axes: the intersection is 20 x 20 and 60 is wanted on each.
        assert_eq!(tapered[1], (20, -20, 80, 40), "grown to the larger width");
    }

    #[test]
    fn an_empty_grid_leaves_the_position_alone() {
        assert_eq!(snap_to_grid(1234, &[], 0), 1234);
    }

    #[test]
    fn a_position_snaps_to_the_nearest_track() {
        let g = [100, 200, 300, 400];
        assert_eq!(snap_to_grid(280, &g, 0), 300);
        assert_eq!(snap_to_grid(220, &g, 0), 200);
    }

    #[test]
    fn a_tie_keeps_the_lower_track() {
        // ⚠️ `new_delta < delta` is strict, so the first of two equally near tracks wins.
        assert_eq!(snap_to_grid(150, &[100, 200], 0), 100);
    }

    #[test]
    fn the_scan_stops_as_soon_as_it_starts_getting_worse() {
        // 🔑 Not a minimum over the whole list. A track list that is not sorted ascending is read
        // only as far as its first rise, which is the reference's behaviour and not a safeguard.
        // 208 is 2 from the 210 at the end and 8 from the 200 before it, but the scan gives up at
        // the 1000 in between and never reaches it.
        assert_eq!(snap_to_grid(208, &[100, 200, 1000, 210], 0), 200);
    }

    #[test]
    fn a_floor_skips_the_tracks_below_it() {
        assert_eq!(snap_to_grid(120, &[100, 200, 300], 200), 200);
    }

    #[test]
    fn the_lower_layers_direction_picks_which_layer_supplies_each_axis() {
        assert_eq!(snap_sources(true), (true, false), "lower horizontal: x from upper");
        assert_eq!(snap_sources(false), (false, true), "lower vertical: x from lower");
    }

    /// The measured via: a metal2 strap centred on the core edge, over a metal1 rail.
    ///
    /// Every number is read off the reference's own output — the strap centre, the rail extent,
    /// and the via's name `via1_2_2000_340_1_6_300_300`.
    const RAIL: Rect = (20140, 22230, 180500, 22570);
    const STRAP: Rect = (19140, 0, 21140, 201600);

    #[test]
    fn via_area_is_the_plain_intersection() {
        // 🔑 What the reference computes, and nothing more:
        // `lower_rect_.intersection(upper_rect_, intersection_rect_)`.
        assert_eq!(via_area(RAIL, STRAP), intersect(RAIL, STRAP));
        // A rail that spans the strap gives the strap's full width, which is the 2000 by 340 the
        // reference named this via -- no unclipped rule needed to get there.
        let untrimmed: Rect = (0, 22230, 180500, 22570);
        let area = via_area(untrimmed, STRAP);
        assert_eq!((area.2 - area.0, area.3 - area.1), (2000, 340));
        assert_eq!((area.0 + area.2) / 2, 20140);
    }

    #[test]
    fn the_probe_case_places_ten_units_above_the_rail_centre() {
        // 🔑 The bottom rail spans -170..170 and the strap starts at the die edge. The plain
        // intersection is the rail's own band, and snapping it to twice a grid of 10 gives
        // -140..160 — centre 10, not 0. Ten units is the whole difference between a via that
        // fits inside the rail and one the write stage rips out.
        let rail: Rect = (0, -170, 200260, 170);
        let strap: Rect = (3520, -170, 4480, 201770);
        assert_eq!(placement_point(rail, strap, 10), Some((4000, 10)));
    }

    #[test]
    fn the_snap_is_truncation_not_a_floor() {
        // ⚠️ -170 rounds UP to -140, because `pos / grid` truncates toward zero before the
        // increment. A floor-based ceil would give -160 and every via below the origin moves.
        assert_eq!(crate::straps::snap_to_manufacturing_grid(-170, 20, true), -140);
        assert_eq!(crate::straps::snap_to_manufacturing_grid(170, 20, false), 160);
    }

    #[test]
    fn snapping_guarantees_the_centre_lands_on_the_grid() {
        // 🔑 Both bounds become multiples of 2g, so their midpoint is `g * (a + b)` — on the grid
        // by construction. The reference still tests it and builds a DUMMY via if it fails, which
        // places nothing at all; that guard is kept here for the case where the technology states
        // no manufacturing grid and no snapping happened.
        for g in [1, 5, 7, 10] {
            for (a, b) in [(0, 30), (5, 35), (-170, 170), (13, 97)] {
                let lower: Rect = (a, -170, b, 170);
                let upper: Rect = (a, -170, b, 170);
                let Some((x, y)) = placement_point(lower, upper, g) else {
                    panic!("no placement for grid {g} over {a}..{b}");
                };
                assert_eq!((x % g, y % g), (0, 0), "grid {g}, {a}..{b}");
            }
        }
    }

    #[test]
    fn an_already_snapped_intersection_is_left_alone() {
        let a: Rect = (0, 0, 200, 40);
        let b: Rect = (40, 0, 80, 200);
        assert_eq!(placement_point(a, b, 10), Some((60, 20)));
    }

    #[test]
    fn a_shape_ending_inside_the_other_clips_the_via_rect() {
        // ⚠️ **Where the two rules used to differ, and the reason the old one existed.** `RAIL`
        // here is the rail AFTER trimming -- it starts at the strap's own centre, the core edge
        // the strap straddles. Fed a trimmed rail, the intersection is half the strap wide, which
        // is what an unclipped rule was once written to avoid. Vias are now built before trimming,
        // so the rail arrives whole and the question does not come up.
        assert_eq!(via_area(RAIL, STRAP), (20140, 22230, 21140, 22570));
        // 🔑 And the clipping is real where a shape genuinely ends inside the other: a repair
        // strap starting part-way up a crossing strap shares only what they overlap.
        let crossing: Rect = (74544, 5499, 200000, 5787);
        let repair: Rect = (74544, 5661, 74664, 77769);
        let area = via_area(repair, crossing);
        assert_eq!(area.3 - area.1, 126, "126, not the crossing shape's full 288");
    }

    #[test]
    fn via_area_agrees_with_the_intersection_where_the_strap_crosses_fully() {
        let inner: Rect = (39140, 0, 41140, 201600);
        assert_eq!(via_area(RAIL, inner), intersect(RAIL, inner));
    }

    #[test]
    fn via_area_is_symmetric_in_its_arguments() {
        assert_eq!(via_area(STRAP, RAIL), via_area(RAIL, STRAP));
    }

    #[test]
    fn two_shapes_of_the_same_orientation_fall_back_to_the_intersection() {
        // 🔑 metal1 and metal2 follow pins: both horizontal, whatever their layers prefer.
        // Neither constrains the other's length, so the intersection is the whole answer.
        let upper_rail: Rect = (20140, 25030, 180500, 25370);
        assert_eq!(via_area(RAIL, upper_rail), intersect(RAIL, upper_rail));
        let a: Rect = (0, 0, 10, 100);
        let b: Rect = (2, 2, 8, 90);
        assert_eq!(via_area(a, b), intersect(a, b), "both vertical");
    }

    #[test]
    fn a_square_shape_constrains_neither_axis() {
        let square: Rect = (0, 0, 100, 100);
        assert_eq!(via_area(RAIL, square), intersect(RAIL, square));
    }

    #[test]
    fn a_via_whose_shape_survived_is_still_held() {
        let shapes = vec![("VDD".to_string(), "metal1".to_string(), (0, 0, 1000, 340))];
        assert!(still_held((100, 0, 300, 340), "metal1", "VDD", &shapes));
    }

    #[test]
    fn a_via_left_outside_a_shrunken_shape_is_not_held() {
        // 🔑 The whole point of the stage: trimming shrank the shape past this via.
        let shapes = vec![("VDD".to_string(), "metal1".to_string(), (0, 0, 200, 340))];
        assert!(!still_held((500, 0, 700, 340), "metal1", "VDD", &shapes));
    }

    #[test]
    fn a_via_whose_shape_was_removed_is_not_held() {
        assert!(!still_held((100, 0, 300, 340), "metal1", "VDD", &[]));
    }

    #[test]
    fn a_shape_on_another_net_or_layer_does_not_hold_it() {
        let area = (100, 0, 300, 340);
        let wrong_net = vec![("VSS".to_string(), "metal1".to_string(), (0, 0, 1000, 340))];
        assert!(!still_held(area, "metal1", "VDD", &wrong_net));
        let wrong_layer = vec![("VDD".to_string(), "metal2".to_string(), (0, 0, 1000, 340))];
        assert!(!still_held(area, "metal1", "VDD", &wrong_layer));
    }

    #[test]
    fn touching_the_trimmed_shape_still_holds_the_via() {
        // ⚠️ Closed, as `Rect::intersects` is.
        let shapes = vec![("VDD".to_string(), "metal1".to_string(), (0, 0, 100, 340))];
        assert!(still_held((100, 0, 300, 340), "metal1", "VDD", &shapes));
    }

    /// A thinning candidate. These tests exercise `area` alone, so the two shape rects — which
    /// only the build path reads — are filled from it.
    fn candidate(net: &str, area: Rect, lower: &str, upper: &str, connect: usize) -> Via {
        Via {
            net: net.into(),
            area,
            lower_rect: area,
            upper_rect: area,
            lower: lower.into(),
            upper: upper.into(),
            connect,
        }
    }

    fn shape(layer: &str, net: &str, rect: Rect) -> Shape {
        Shape {
            layer: layer.into(),
            net: net.into(),
            rect,
        }
    }

    fn connect(lower: &str, upper: &str) -> Connect {
        Connect {
            lower: lower.into(),
            upper: upper.into(),
            intermediate: vec![],
        }
    }

    #[test]
    fn a_connect_is_normalised_so_the_lower_layer_is_below() {
        // ⚠️ `{metal6 metal1}` and `{metal1 metal6}` are the same connect.
        assert_eq!(
            normalise(("m6".into(), 6), ("m1".into(), 1)),
            ("m1".to_string(), "m6".to_string())
        );
        assert_eq!(
            normalise(("m1".into(), 1), ("m6".into(), 6)),
            ("m1".to_string(), "m6".to_string())
        );
    }

    #[test]
    fn the_intermediate_routing_layers_are_those_strictly_between() {
        // Numbers interleave routing and cut layers, so both are walked and the cut ones dropped.
        let layers = vec![
            ("m1".to_string(), 1, 1, false),
            ("v1".to_string(), 2, 0, false),
            ("m2".to_string(), 3, 2, false),
            ("v2".to_string(), 4, 0, false),
            ("m3".to_string(), 5, 3, false),
        ];
        assert_eq!(intermediate_routing(&layers, 1, 5), vec!["m2"]);
        assert!(
            intermediate_routing(&layers, 1, 3).is_empty(),
            "adjacent layers have none"
        );
    }

    #[test]
    fn a_layer_with_a_lef58_type_is_skipped_entirely() {
        // ⚠️ Neither routing nor cut in the sense this cares about; treating one as an intermediate
        // inserts a via into a stack with nowhere to put it.
        let layers = vec![
            ("m1".to_string(), 1, 1, false),
            ("mim".to_string(), 2, 9, true),
            ("m3".to_string(), 3, 3, false),
        ];
        assert!(intermediate_routing(&layers, 1, 3).is_empty());
    }

    #[test]
    fn a_stack_gives_every_middle_layer_the_intersection() {
        // ⚠️ The ends keep their own shapes; a stack is only as wide as the narrowest thing it
        // passes through.
        let lower = (0, 0, 100, 20);
        let upper = (40, -50, 60, 200);
        let got = stack_rects(lower, upper, 2);
        assert_eq!(
            got.len(),
            4,
            "two intermediate layers means four rectangles"
        );
        assert_eq!(got[0], lower);
        assert_eq!(got[3], upper);
        assert_eq!(got[1], (40, 0, 60, 20));
        assert_eq!(
            got[2], got[1],
            "every middle layer takes the same intersection"
        );
    }

    #[test]
    fn a_stack_with_nothing_in_between_is_just_the_two_ends() {
        let got = stack_rects((0, 0, 10, 10), (0, 0, 10, 10), 0);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_stack_is_complex_when_any_middle_layer_needs_more_width_than_it_gets() {
        // ⚠️ The NARROWER dimension of the intersection is what has to satisfy every minimum.
        let intersection = (0, 0, 100, 20);
        assert!(!is_complex(&[10, 15], intersection));
        assert!(
            is_complex(&[10, 25], intersection),
            "25 exceeds the 20-wide dimension"
        );
        assert!(
            !is_complex(&[], intersection),
            "nothing in between is never complex"
        );
    }

    #[test]
    fn the_second_candidate_takes_exactly_the_layer_width_across_the_grain() {
        // A horizontal layer is narrowed in Y and keeps its X untouched, centred where it was.
        assert_eq!(
            min_enclosure_rect((0, 0, 100, 40), 10, true),
            (0, 15, 100, 25)
        );
        // Vertical: the same, across the other axis.
        assert_eq!(
            min_enclosure_rect((0, 0, 40, 100), 10, false),
            (15, 0, 25, 100)
        );
    }

    #[test]
    fn the_second_candidate_widens_a_rect_thinner_than_the_layer() {
        // ⚠️ The span is ASSIGNED, so this is not a shrink — a 4-tall rect on a 10-wide layer
        // comes back 10 tall. Treating it as a narrowing loses the candidate that fits.
        assert_eq!(
            min_enclosure_rect((0, 8, 100, 12), 10, true),
            (0, 5, 100, 15)
        );
    }

    #[test]
    fn an_odd_width_puts_the_extra_unit_on_the_high_side() {
        // centre 20, lo = 20 - 7/2 = 17, hi = 17 + 7 = 24 — so 3 below the centre and 4 above.
        assert_eq!(min_enclosure_rect((0, 0, 100, 40), 7, true), (0, 17, 100, 24));
    }

    #[test]
    fn every_intermediate_level_gains_a_candidate_and_the_ends_do_not() {
        let mut stack = vec![
            vec![(0, 0, 100, 40)],
            vec![(0, 0, 40, 40)],
            vec![(0, 0, 40, 100)],
        ];
        add_min_enclosure_rects(&mut stack, &[10], &[true], &[false]);
        assert_eq!(stack[0], vec![(0, 0, 100, 40)], "the lower shape is itself");
        assert_eq!(
            stack[1],
            vec![(0, 0, 40, 40), (0, 15, 40, 25)],
            "the middle offers both, sorted"
        );
        assert_eq!(stack[2], vec![(0, 0, 40, 100)], "the upper shape is itself");
    }

    #[test]
    fn a_min_width_layer_is_left_with_only_the_narrow_candidate() {
        let mut stack = vec![vec![(0, 0, 100, 40)], vec![(0, 0, 40, 40)], vec![(0, 0, 40, 100)]];
        add_min_enclosure_rects(&mut stack, &[10], &[true], &[true]);
        assert_eq!(stack[1], vec![(0, 15, 40, 25)], "the full rect is dropped");
    }

    #[test]
    fn a_candidate_equal_to_the_rect_it_came_from_does_not_double_the_level() {
        // The rect is already exactly the layer width, so the set stays at one.
        let mut stack = vec![vec![(0, 0, 100, 40)], vec![(0, 15, 40, 25)], vec![(0, 0, 40, 100)]];
        add_min_enclosure_rects(&mut stack, &[10], &[true], &[false]);
        assert_eq!(stack[1], vec![(0, 15, 40, 25)]);
    }

    #[test]
    fn a_via_goes_where_two_shapes_on_connected_layers_overlap() {
        let s = [
            shape("m1", "VDD", (0, 0, 100, 20)),
            shape("m2", "VDD", (40, -50, 60, 200)),
        ];
        let v = intersections(&[connect("m1", "m2")], &s);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].area, (40, 0, 60, 20), "the overlap, not either shape");
    }

    #[test]
    fn shapes_that_merely_touch_make_no_via() {
        // ⚠️ The trap. The R-tree query counts a shared edge and the follow-up test does not; only
        // the pair gives the reference's answer.
        let s = [
            shape("m1", "VDD", (0, 0, 100, 20)),
            shape("m2", "VDD", (100, 0, 120, 20)),
        ];
        assert!(intersections(&[connect("m1", "m2")], &s).is_empty());
    }

    #[test]
    fn shapes_on_different_nets_do_not_connect() {
        let s = [
            shape("m1", "VDD", (0, 0, 100, 20)),
            shape("m2", "VSS", (40, -50, 60, 200)),
        ];
        assert!(intersections(&[connect("m1", "m2")], &s).is_empty());
    }

    #[test]
    fn a_connect_whose_layers_have_no_shapes_yields_nothing() {
        let s = [shape("m1", "VDD", (0, 0, 100, 20))];
        assert!(intersections(&[connect("m1", "m2")], &s).is_empty());
    }

    #[test]
    fn candidates_come_out_in_connect_declaration_order() {
        // 🔑 This order decides which of two equal candidates survives the thinning below.
        let s = [
            shape("m1", "VDD", (0, 0, 100, 20)),
            shape("m2", "VDD", (40, -50, 60, 200)),
            shape("m4", "VDD", (10, -50, 30, 200)),
        ];
        let a = intersections(&[connect("m1", "m2"), connect("m1", "m4")], &s);
        let b = intersections(&[connect("m1", "m4"), connect("m1", "m2")], &s);
        assert_eq!((a[0].upper.as_str(), a[1].upper.as_str()), ("m2", "m4"));
        assert_eq!((b[0].upper.as_str(), b[1].upper.as_str()), ("m4", "m2"));
    }

    #[test]
    fn an_obstruction_on_an_intermediate_layer_blocks_the_via() {
        let c = [Connect {
            lower: "m1".into(),
            upper: "m4".into(),
            intermediate: vec!["m2".into()],
        }];
        let s = [
            shape("m1", "VDD", (0, 0, 100, 20)),
            shape("m4", "VDD", (40, -50, 60, 200)),
        ];
        let (kept, dropped) = place(&c, &s, &[("m2".into(), (45, 5, 55, 15))]);
        assert!(kept.is_empty());
        assert_eq!(dropped[0].1, Failed::Obstructed);
    }

    #[test]
    fn an_obstruction_on_an_unrelated_layer_does_not() {
        let c = [Connect {
            lower: "m1".into(),
            upper: "m4".into(),
            intermediate: vec!["m2".into()],
        }];
        let s = [
            shape("m1", "VDD", (0, 0, 100, 20)),
            shape("m4", "VDD", (40, -50, 60, 200)),
        ];
        let (kept, _) = place(&c, &s, &[("m7".into(), (45, 5, 55, 15))]);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn of_two_overlapping_candidates_the_larger_survives() {
        let vias = vec![
            candidate("VDD", (0, 0, 10, 10), "m1", "m2", 0),
            candidate("VDD", (0, 0, 20, 20), "m1", "m2", 0),
        ];
        let (kept, dropped) = remove_overlapping(vias);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].area, (0, 0, 20, 20));
        assert_eq!(dropped[0].1, Failed::Overlapping);
    }

    #[test]
    fn of_two_equal_overlapping_candidates_the_first_dies_and_the_second_lives() {
        // ⚠️ `<=`, and the loser is marked immediately so the winner no longer sees it. Both of
        // those are needed: with `<` neither would lose, and with a batch pass both would.
        let mk = |x: i32| candidate("VDD", (x, 0, x + 10, 10), "m1", "m2", 0);
        let (kept, _) = remove_overlapping(vec![mk(0), mk(5)]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].area.0, 5, "the second one survives");
    }

    #[test]
    fn a_chain_of_three_equal_candidates_leaves_one_standing() {
        // 🔑 The case that separates the reference's sequential marking from the obvious batch
        // implementation. Batch removal takes all three: each is beaten by a neighbour.
        let mk = |x: i32| candidate("VDD", (x, 0, x + 10, 10), "m1", "m2", 0);
        let (kept, _) = remove_overlapping(vec![mk(0), mk(5), mk(10)]);
        assert_eq!(kept.len(), 1, "not zero");
    }

    #[test]
    fn candidates_on_different_layer_pairs_do_not_compete() {
        let a = candidate("VDD", (0, 0, 10, 10), "m1", "m2", 0);
        let b = candidate("VDD", (0, 0, 20, 20), "m4", "m5", 1);
        let (kept, _) = remove_overlapping(vec![a, b]);
        assert_eq!(
            kept.len(),
            2,
            "a metal1-metal2 via does not thin a metal4-metal5 one"
        );
    }

    #[test]
    fn an_obstructed_candidate_cannot_thin_a_legal_one() {
        // 🔑 Why obstruction removal comes first. The big candidate is obstructed; if it were still
        // present during the overlap pass it would eliminate the small one, and the design would
        // end up with no via where a legal one exists.
        let c = [Connect {
            lower: "m1".into(),
            upper: "m2".into(),
            intermediate: vec!["x".into()],
        }];
        let s = [
            shape("m1", "VDD", (0, 0, 100, 100)),
            shape("m2", "VDD", (0, 0, 20, 20)),
            shape("m2", "VDD", (0, 0, 60, 60)),
        ];
        let (kept, _) = place(&c, &s, &[("x".into(), (30, 30, 50, 50))]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].area, (0, 0, 20, 20), "the small legal one survives");
    }

    // ── the obstruction index ────────────────────────────────────────────────────────────────
    //
    // 🔑 **An acceleration is judged against the thing it replaces, not against expectations.**
    // `ObstructionIndex` exists only to answer the old linear scan faster, so every test below
    // compares the two answers directly. A case list would only ever check the shapes someone
    // thought of.

    /// The predicate `remove_obstructed` used before the index, kept verbatim as the oracle.
    fn naive_hit(obstructions: &[(String, Rect)], layer: &str, area: Rect) -> bool {
        obstructions
            .iter()
            .any(|(l, r)| l == layer && intersects(area, *r))
    }

    /// A deterministic LCG — a seeded sweep must be reproducible, and a dependency for eight lines
    /// of arithmetic is not worth it.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 11
        }
        fn in_range(&mut self, lo: i32, hi: i32) -> i32 {
            lo + (self.next() % ((hi - lo) as u64 + 1)) as i32
        }
    }

    #[test]
    fn the_index_answers_exactly_what_the_linear_scan_answers() {
        let mut rng = Lcg(0x5EED);
        let layers = ["Metal2", "Metal3", "Via2"];
        let mut obstructions: Vec<(String, Rect)> = Vec::new();
        for _ in 0..4000 {
            let l = layers[(rng.next() % 3) as usize];
            let x = rng.in_range(-5_000, 100_000);
            let y = rng.in_range(-5_000, 100_000);
            // ⚠️ A deliberate mix of sizes: pin-sized rects, and a few far larger than a cell so
            // the `oversized` path is exercised rather than merely present.
            let (w, h) = if rng.next() % 50 == 0 {
                (rng.in_range(20_000, 60_000), rng.in_range(20_000, 60_000))
            } else {
                (rng.in_range(0, 400), rng.in_range(0, 400))
            };
            obstructions.push((l.to_string(), (x, y, x + w, y + h)));
        }
        let index = ObstructionIndex::build(&obstructions);

        let mut checked = 0;
        let mut hits = 0;
        for _ in 0..20_000 {
            // ⚠️ Query outside the indexed area too — clamping a query into the grid must not
            // invent a hit on an edge cell.
            let x = rng.in_range(-20_000, 120_000);
            let y = rng.in_range(-20_000, 120_000);
            let w = rng.in_range(0, 300);
            let h = rng.in_range(0, 300);
            let area = (x, y, x + w, y + h);
            for l in layers.iter().chain(std::iter::once(&"NoSuchLayer")) {
                let want = naive_hit(&obstructions, l, area);
                assert_eq!(
                    index.any_hit(l, area),
                    want,
                    "layer {l} area {area:?} disagreed with the linear scan"
                );
                checked += 1;
                hits += want as usize;
            }
        }
        // ⛔ **A check that cannot fail proves nothing.** If the sweep never found an obstruction,
        // every `assert_eq!` above compared false to false and the index was never exercised.
        assert_eq!(checked, 80_000);
        assert!(hits > 1_000, "only {hits} hits — the sweep is not testing the positive path");
    }

    #[test]
    fn a_rect_too_big_to_bucket_is_still_found() {
        // ⛔ **This test exists because the sweep above did NOT cover the `oversized` path.**
        // Deleting the oversized scan left that sweep green: with 4000 rects the grid is ~63 cells
        // across, so a rect only lands in the oversized list if it spans nearly the whole layer,
        // and none of the sweep's "large" rects did. A path no test reaches is a path with no
        // witness, whatever the coverage of the tests around it.
        //
        // 10,000 small rects over a 1,000,000 span puts the grid at ~100 cells across, so one
        // full-extent rect covers 100 > MAX_CELLS_PER_RECT and must take the linear path.
        let mut rng = Lcg(0xB16);
        let mut obstructions: Vec<(String, Rect)> = Vec::new();
        for _ in 0..10_000 {
            let x = rng.in_range(0, 1_000_000);
            let y = rng.in_range(0, 1_000_000);
            obstructions.push(("Metal2".to_string(), (x, y, x + 100, y + 100)));
        }
        // ⚠️ **On the SAME layer as the small ones.** The grid is built per layer, so a big rect
        // alone on its own layer gets a grid sized for one rect and is never oversized — the first
        // spelling of this test put it on `Metal3` and asserted a list that stayed empty.
        obstructions.push(("Metal2".to_string(), (0, 0, 1_000_000, 1_000_000)));
        let index = ObstructionIndex::build(&obstructions);

        // The path is REACHED, not merely present.
        assert_eq!(
            index.layers["Metal2"].oversized.len(),
            1,
            "the full-extent rect must take the oversized path, or this test proves nothing"
        );

        // And it is ANSWERED: a query far outside every small rect still hits the big one.
        let mut outside_hits = 0;
        for _ in 0..2_000 {
            let x = rng.in_range(-50_000, 1_050_000);
            let y = rng.in_range(-50_000, 1_050_000);
            let area = (x, y, x + 50, y + 50);
            assert_eq!(
                index.any_hit("Metal2", area),
                naive_hit(&obstructions, "Metal2", area),
                "oversized rect disagreed with the linear scan at {area:?}"
            );
            if x >= 0 && y >= 0 && x <= 1_000_000 && y <= 1_000_000 {
                outside_hits += 1;
            }
        }
        assert!(outside_hits > 100, "the sweep never queried inside the oversized rect");
    }

    #[test]
    fn an_empty_layer_and_an_empty_index_hit_nothing() {
        let index = ObstructionIndex::build(&[]);
        assert!(!index.any_hit("Metal2", (0, 0, 10, 10)));
        let one = [("Metal2".to_string(), (5, 5, 5, 5))];
        let index = ObstructionIndex::build(&one);
        // A degenerate zero-area rect still has to be found: `intersects` is the CLOSED test, so a
        // touch counts, and building the grid must not divide by a zero span.
        assert!(index.any_hit("Metal2", (0, 0, 5, 5)));
        assert!(!index.any_hit("Metal2", (6, 6, 10, 10)));
        assert!(!index.any_hit("Metal3", (0, 0, 10, 10)));
    }
}
