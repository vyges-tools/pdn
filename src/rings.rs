// SPDX-License-Identifier: Apache-2.0
//! The ring of wide metal around the core.
//!
//! One ring per net, nested: the first net gets the innermost loop and each net after it sits one
//! pitch further out. A ring is built from four sides on two layers — the horizontal-running layer
//! carries the bottom and top, the other carries the left and right.
//!
//! Nothing here touches a database.

use crate::{Direction, Rect};

/// One layer of a ring: how wide its metal is and how far apart successive nets sit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    pub direction: Direction,
    pub width: i32,
    pub spacing: i32,
}

impl Layer {
    /// Centre-to-centre step between one net's ring and the next on this layer.
    fn pitch(&self) -> i32 {
        self.spacing + self.width
    }
}

/// One side of one net's ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub layer: String,
    pub net: String,
    pub rect: Rect,
}

/// **R1** — the rectangle the innermost ring is drawn around.
///
/// The domain's area, pushed out by the four offsets in `(left, bottom, right, top)` order. ⚠️ The
/// offsets *grow* the outline: a positive offset puts the ring further from the core, not nearer.
pub fn inner_outline(domain: Rect, offset: [i32; 4]) -> Rect {
    (
        domain.0 - offset[0],
        domain.1 - offset[1],
        domain.2 + offset[2],
        domain.3 + offset[3],
    )
}

/// **R2** — every segment of a ring, in the order the reference emits them.
///
/// `nets` is in the order the grid reports them, and that order is the answer to "which net is
/// innermost": the first net gets the tightest loop and each one after it steps out by a pitch.
///
/// ⚠️ **Which layer carries which pair of sides is not read off the layer's direction alone.**
/// Two layers are walked as `(layer0, layer1)` then `(layer1, layer0)`, and a pass builds the
/// bottom and top when *either* the ring is single-layer and no horizontal pass has happened yet,
/// *or* the ring is two-layer and this pass's layer runs horizontally. So:
///
/// - a **single-layer** ring builds horizontal on its first pass and vertical on its second,
///   whatever direction the layer actually declares;
/// - a **two-layer** ring where neither layer runs horizontally builds left and right **twice**
///   and no bottom or top at all. That is not a guard that was forgotten — it is what the
///   condition says, and a ring on two vertical layers is a malformed request that the layer
///   checks reject earlier.
///
/// `extend_to_boundary` replaces the along-side extent with the grid boundary. ⚠️ It stops the
/// *along* axis growing per net, but the across axis still steps by a pitch — the rings still nest,
/// they just all reach the same distance.
/// **R2** on a region — `Rings::makeShapes` since polygon floorplans (`a718d1cd60`).
///
/// `inner` is the inner ring outline (the domain region grown by the offsets). Net `i`'s ring is
/// that outline grown by `i` pitches — this layer's pitch along its own direction of travel, the
/// other layer's across it — and its edges facing each side are swept outward by this layer's
/// width, then run out along the side by the other layer's width at a CONVEX corner only (at a
/// concave corner the neighbour already shares that square). With `boundary`, each end runs instead
/// to where the boundary region ends beyond it (`getMarginBeyond`).
///
/// Sides are walked bottom then top for the horizontal pass, left then right for the vertical one,
/// net by net — the rectangular builder's order, which this reproduces exactly on a rectangle.
pub fn make_region(
    layer0: &Layer,
    layer1: &Layer,
    nets: &[String],
    inner: &[Rect],
    boundary: Option<&[Rect]>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    let single_layer = layer0.name == layer1.name;
    let mut processed_horizontal = false;
    for (def, other) in [(layer0, layer1), (layer1, layer0)] {
        let (width, pitch) = (def.width, def.pitch());
        let (other_width, other_pitch) = (other.width, other.pitch());
        let horizontal = if single_layer { !processed_horizontal } else { def.direction == Direction::Horizontal };
        if horizontal {
            processed_horizontal = true;
        }
        let ring_edges: Vec<Vec<crate::region::Edge>> = (0..nets.len())
            .map(|i| {
                let (step, other_step) = (i as i32 * pitch, i as i32 * other_pitch);
                let m = if horizontal { [other_step, step, other_step, step] } else { [step, other_step, step, other_step] };
                crate::region::edges(&crate::region::outline_loops(&crate::region::bloat(inner, m)))
            })
            .collect();
        let sides = if horizontal { [(0, -1), (0, 1)] } else { [(-1, 0), (1, 0)] };
        for side in sides {
            for (index, net) in nets.iter().enumerate() {
                for e in ring_edges[index].iter().filter(|e| e.normal == side) {
                    let (xlo, xhi) = (e.start.0.min(e.end.0), e.start.0.max(e.end.0));
                    let (ylo, yhi) = (e.start.1.min(e.end.1), e.start.1.max(e.end.1));
                    let start_is_low = if horizontal { e.start.0 < e.end.0 } else { e.start.1 < e.end.1 };
                    let (convex_low, convex_high) =
                        if start_is_low { (e.convex_at_start, e.convex_at_end) } else { (e.convex_at_end, e.convex_at_start) };
                    let mut rect = if horizontal {
                        let base = if side.1 > 0 { yhi } else { ylo - width };
                        (xlo, base, xhi, base + width)
                    } else {
                        let base = if side.0 > 0 { xhi } else { xlo - width };
                        (base, ylo, base + width, yhi)
                    };
                    let (low_n, high_n) = if horizontal { ((-1, 0), (1, 0)) } else { ((0, -1), (0, 1)) };
                    let (mut ext_low, mut ext_high) =
                        (if convex_low { other_width } else { 0 }, if convex_high { other_width } else { 0 });
                    if let Some(b) = boundary {
                        ext_low = crate::region::margin_beyond_exact(b, rect, low_n);
                        ext_high = crate::region::margin_beyond_exact(b, rect, high_n);
                    }
                    if horizontal {
                        rect.0 -= ext_low;
                        rect.2 += ext_high;
                    } else {
                        rect.1 -= ext_low;
                        rect.3 += ext_high;
                    }
                    out.push(Segment { layer: def.name.clone(), net: net.clone(), rect });
                }
            }
        }
    }
    out
}

/// **R6** — the width every net's ring occupies on each axis, `Rings::getTotalWidth`.
///
/// `width * rings + spacing * (rings - 1)` per layer, and ⚠️ **the pair is swapped where the lower
/// layer does not run horizontally** — the caller wants "how much a horizontal ring takes" and
/// "how much a vertical one takes", which is not the same as "layer0's and layer1's".
pub fn total_width(layer0: &Layer, layer1: &Layer, rings: usize) -> (i32, i32) {
    let n = rings as i32;
    let span = |l: &Layer| l.width * n + l.spacing * (n - 1);
    let (hor, ver) = (span(layer0), span(layer1));
    if layer0.direction == Direction::Horizontal {
        (hor, ver)
    } else {
        (ver, hor)
    }
}

/// One built ring as a follow pin set reads it: `getTotalWidth` (horizontal, vertical), its core
/// offsets `(left, bottom, right, top)` in the placed frame, and its shapes as they stand.
pub type BuiltRing = (i32, i32, [i32; 4], Vec<Rect>);

/// `Grid::getRingReach` — how far a shape at `band` may run out on the side `normal` points to: the
/// furthest edge of any ring side ALONGSIDE the band on that side, but only within that ring's own
/// reach from the domain (offset + total width on that side), which is what tells the band's own
/// leg's ring from the one across a notch. A side bounds only the axis it is thin along (a square
/// corner piece both). No such side: the band's own end.
pub fn ring_reach(rings: &[BuiltRing], band: Rect, normal: (i32, i32)) -> i32 {
    let horizontal_reach = normal.0 != 0;
    let high = if horizontal_reach { normal.0 > 0 } else { normal.1 > 0 };
    let origin = match (horizontal_reach, high) {
        (true, true) => band.2,
        (true, false) => band.0,
        (false, true) => band.3,
        (false, false) => band.1,
    };
    let mut reach = origin;
    for (hor, ver, off, shapes) in rings {
        let limit = if horizontal_reach {
            (if high { off[2] } else { off[0] }) + ver
        } else {
            (if high { off[3] } else { off[1] }) + hor
        };
        for r in shapes {
            let (dx, dy) = (r.2 - r.0, r.3 - r.1);
            let bounds = dx == dy || if horizontal_reach { dx < dy } else { dx > dy };
            if !bounds {
                continue;
            }
            let alongside = if horizontal_reach { !(r.3 <= band.1 || r.1 >= band.3) } else { !(r.2 <= band.0 || r.0 >= band.2) };
            if !alongside {
                continue;
            }
            let far = match (horizontal_reach, high) {
                (true, true) => r.2,
                (true, false) => r.0,
                (false, true) => r.3,
                (false, false) => r.1,
            };
            let distance = if high { far - origin } else { origin - far };
            if distance <= 0 || distance > limit {
                continue;
            }
            reach = if high { reach.max(far) } else { reach.min(far) };
        }
    }
    reach
}

/// **R7** — a PAD offset expressed as the core offset it is equivalent to, `Rings::setPadOffset`.
///
/// 🔑 **`-pad_offsets` is not a second placement rule.** The reference measures the gap from the
/// core out to the inner edge of the pad ring, subtracts the offset asked for and the width the
/// rings themselves will occupy, and hands the remainder to `setOffset` as an ordinary core
/// offset. Everything downstream then behaves as though `-core_offsets` had been given.
///
/// ⚠️ **The ring's own width is subtracted**, so the offset names the gap between the pads and the
/// OUTERMOST loop rather than the distance to where the innermost one starts.
///
/// ⚠️ Sides are ordered left, bottom, right, top, and each is measured against its own edge —
/// a pad ring that is not centred on the core gives four different answers.
pub fn pad_offset_as_core_offset(
    core: Rect,
    pads_inner: Rect,
    pad_offset: [i32; 4],
    hor_width: i32,
    ver_width: i32,
) -> [i32; 4] {
    [
        core.0 - pads_inner.0 - pad_offset[0] - ver_width,
        core.1 - pads_inner.1 - pad_offset[1] - hor_width,
        pads_inner.2 - core.2 - pad_offset[2] - ver_width,
        pads_inner.3 - core.3 - pad_offset[3] - hor_width,
    ]
}

pub fn make(
    layer0: &Layer,
    layer1: &Layer,
    nets: &[String],
    core: Rect,
    boundary: Option<Rect>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    let single_layer = layer0.name == layer1.name;
    let mut processed_horizontal = false;

    for (def, other) in [(layer0, layer1), (layer1, layer0)] {
        let width = def.width;
        let pitch = def.pitch();
        let other_width = other.width;
        let other_pitch = other.pitch();

        let horizontal_pass = if single_layer {
            !processed_horizontal
        } else {
            def.direction == Direction::Horizontal
        };

        if horizontal_pass {
            processed_horizontal = true;

            // ── bottom ───────────────────────────────────────────────────────────────────────
            let (mut x0, mut x1) = match boundary {
                Some(b) => (b.0, b.2),
                None => (core.0 - other_width, core.2 + other_width),
            };
            let (mut y0, mut y1) = (core.1 - width, core.1);
            for net in nets {
                out.push(Segment {
                    layer: def.name.clone(),
                    net: net.clone(),
                    rect: (x0, y0, x1, y1),
                });
                if boundary.is_none() {
                    x0 -= other_pitch;
                    x1 += other_pitch;
                }
                y0 -= pitch;
                y1 -= pitch;
            }

            // ── top ──────────────────────────────────────────────────────────────────────────
            // ⚠️ The along extent is RESET here, but only when not extending to the boundary. With
            // a boundary it keeps the values the bottom loop left, which are the boundary's own
            // and were never stepped — so the two agree, and only by that.
            if boundary.is_none() {
                x0 = core.0 - other_width;
                x1 = core.2 + other_width;
            }
            y0 = core.3;
            y1 = y0 + width;
            for net in nets {
                out.push(Segment {
                    layer: def.name.clone(),
                    net: net.clone(),
                    rect: (x0, y0, x1, y1),
                });
                if boundary.is_none() {
                    x0 -= other_pitch;
                    x1 += other_pitch;
                }
                y0 += pitch;
                y1 += pitch;
            }
        } else {
            // ── left ─────────────────────────────────────────────────────────────────────────
            let (mut x0, mut x1) = (core.0 - width, core.0);
            let (mut y0, mut y1) = match boundary {
                Some(b) => (b.1, b.3),
                None => (core.1 - other_width, core.3 + other_width),
            };
            for net in nets {
                out.push(Segment {
                    layer: def.name.clone(),
                    net: net.clone(),
                    rect: (x0, y0, x1, y1),
                });
                x0 -= pitch;
                x1 -= pitch;
                if boundary.is_none() {
                    y0 -= other_pitch;
                    y1 += other_pitch;
                }
            }

            // ── right ────────────────────────────────────────────────────────────────────────
            x0 = core.2;
            x1 = x0 + width;
            if boundary.is_none() {
                y0 = core.1 - other_width;
                y1 = core.3 + other_width;
            }
            for net in nets {
                out.push(Segment {
                    layer: def.name.clone(),
                    net: net.clone(),
                    rect: (x0, y0, x1, y1),
                });
                x0 += pitch;
                x1 += pitch;
                if boundary.is_none() {
                    y0 -= other_pitch;
                    y1 += other_pitch;
                }
            }
        }
    }
    out
}

/// **R3** — a single-layer ring's shapes are locked once made.
///
/// A locked shape is not trimmed or merged by later stages. ⚠️ This applies to the whole ring and
/// only when both layers are the same one: a two-layer ring's shapes stay editable.
pub fn locked(layer0: &Layer, layer1: &Layer) -> bool {
    layer0.name == layer1.name
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the pad-offset conversion ────────────────────────────────────────────────────────────
    //
    // A 100 x 100 core inside a pad ring whose inner edge is 20 away on every side, two nets, both
    // ring layers 4 wide with 2 of spacing: each axis takes 4*2 + 2*1 = 10.

    const CORE: Rect = (100, 100, 200, 200);
    const PADS_INNER: Rect = (80, 80, 220, 220);

    fn ring_layers() -> (Layer, Layer) {
        (
            layer("m8", Direction::Horizontal, 4, 2),
            layer("m9", Direction::Vertical, 4, 2),
        )
    }

    #[test]
    fn the_total_width_counts_every_nets_loop_and_the_gaps_between() {
        let (h, v) = ring_layers();
        assert_eq!(total_width(&h, &v, 2), (10, 10));
        // One net has no gaps, three have two.
        assert_eq!(total_width(&h, &v, 1), (4, 4));
        assert_eq!(total_width(&h, &v, 3), (16, 16));
    }

    #[test]
    fn the_total_width_is_swapped_when_the_lower_layer_runs_vertically() {
        // The caller wants horizontal-then-vertical, not layer0-then-layer1.
        let lower = layer("m8", Direction::Vertical, 4, 2);
        let upper = layer("m9", Direction::Horizontal, 6, 2);
        assert_eq!(total_width(&lower, &upper, 2), (14, 10));
    }

    #[test]
    fn a_pad_offset_becomes_the_core_offset_that_leaves_the_gap_asked_for() {
        // 20 of room per side, 10 of it taken by the rings, 5 asked for as clearance: 5 left.
        let got = pad_offset_as_core_offset(CORE, PADS_INNER, [5; 4], 10, 10);
        assert_eq!(got, [5, 5, 5, 5]);
    }

    #[test]
    fn each_side_is_measured_against_its_own_edge() {
        // A pad ring pushed right: 10 of room on the left, 30 on the right.
        let pads = (90, 80, 230, 220);
        let got = pad_offset_as_core_offset(CORE, pads, [0, 0, 0, 0], 10, 10);
        assert_eq!(got, [0, 10, 20, 10]);
    }

    #[test]
    fn asking_for_more_room_than_the_pads_leave_goes_negative_rather_than_clamping() {
        // ⚠️ The reference does not clamp, so the ring lands inside the core and says nothing.
        let got = pad_offset_as_core_offset(CORE, PADS_INNER, [15; 4], 10, 10);
        assert_eq!(got, [-5, -5, -5, -5]);
    }

    fn layer(name: &str, dir: Direction, width: i32, spacing: i32) -> Layer {
        Layer {
            name: name.into(),
            direction: dir,
            width,
            spacing,
        }
    }

    fn nets(n: usize) -> Vec<String> {
        ["VDD", "VSS", "VNN"][..n]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn the_offsets_push_the_outline_out_not_in() {
        assert_eq!(
            inner_outline((100, 200, 300, 400), [10, 20, 30, 40]),
            (90, 180, 330, 440)
        );
    }

    #[test]
    fn a_two_layer_ring_gives_each_net_four_sides() {
        let h = layer("m5", Direction::Horizontal, 10, 5);
        let v = layer("m6", Direction::Vertical, 10, 5);
        let segs = make(&h, &v, &nets(2), (0, 0, 1000, 1000), None);
        assert_eq!(segs.len(), 8, "two nets, four sides each");
        assert_eq!(
            segs.iter().filter(|s| s.layer == "m5").count(),
            4,
            "bottom and top, both nets"
        );
        assert_eq!(
            segs.iter().filter(|s| s.layer == "m6").count(),
            4,
            "left and right, both nets"
        );
    }

    #[test]
    fn the_first_net_gets_the_innermost_loop() {
        let h = layer("m5", Direction::Horizontal, 10, 5);
        let v = layer("m6", Direction::Vertical, 10, 5);
        let segs = make(&h, &v, &nets(2), (0, 0, 1000, 1000), None);
        let bottoms: Vec<_> = segs
            .iter()
            .filter(|s| s.layer == "m5" && s.rect.1 < 0)
            .collect();
        assert_eq!(bottoms[0].net, "VDD");
        assert_eq!(bottoms[0].rect.1, -10, "first net sits against the core");
        assert_eq!(bottoms[1].net, "VSS");
        assert_eq!(bottoms[1].rect.1, -25, "second is one pitch further out");
    }

    #[test]
    fn a_later_nets_side_reaches_further_along_to_meet_the_ring_beyond_it() {
        // ⚠️ The along extent grows by the OTHER layer's pitch, not this layer's: the bottom has to
        // reach the left and right sides of the same net, and those step outwards by their own
        // layer's pitch.
        let h = layer("m5", Direction::Horizontal, 10, 5);
        let v = layer("m6", Direction::Vertical, 20, 30); // pitch 50, deliberately different
        let segs = make(&h, &v, &nets(2), (0, 0, 1000, 1000), None);
        let bottoms: Vec<_> = segs
            .iter()
            .filter(|s| s.layer == "m5" && s.rect.1 < 0)
            .collect();
        assert_eq!(
            bottoms[0].rect.0, -20,
            "reaches out by the other layer's width"
        );
        assert_eq!(
            bottoms[1].rect.0, -70,
            "and then by the other layer's PITCH, not its own"
        );
    }

    #[test]
    fn extending_to_the_boundary_fixes_the_along_extent_but_still_nests() {
        let h = layer("m5", Direction::Horizontal, 10, 5);
        let v = layer("m6", Direction::Vertical, 10, 5);
        let b = (-500, -500, 1500, 1500);
        let segs = make(&h, &v, &nets(2), (0, 0, 1000, 1000), Some(b));
        let bottoms: Vec<_> = segs
            .iter()
            .filter(|s| s.layer == "m5" && s.rect.1 < 0)
            .collect();
        assert_eq!((bottoms[0].rect.0, bottoms[0].rect.2), (-500, 1500));
        assert_eq!(
            (bottoms[1].rect.0, bottoms[1].rect.2),
            (-500, 1500),
            "both reach the boundary"
        );
        assert_eq!(bottoms[1].rect.1, -25, "but they still step out by a pitch");
    }

    #[test]
    fn the_top_side_starts_again_from_the_core_rather_than_where_the_bottom_finished() {
        let h = layer("m5", Direction::Horizontal, 10, 5);
        let v = layer("m6", Direction::Vertical, 10, 5);
        let segs = make(&h, &v, &nets(2), (0, 0, 1000, 1000), None);
        let tops: Vec<_> = segs
            .iter()
            .filter(|s| s.layer == "m5" && s.rect.1 >= 1000)
            .collect();
        assert_eq!(
            tops[0].rect.0, -10,
            "the top's first net starts at the core again"
        );
        assert_eq!(tops[0].rect.1, 1000);
    }

    #[test]
    fn a_single_layer_ring_builds_horizontal_first_then_vertical() {
        // ⚠️ Both passes are the same layer, so the DIRECTION cannot decide which sides get built.
        // The first pass takes the horizontal sides and the second is left with the vertical ones,
        // even though this layer declares itself vertical.
        let v = layer("m5", Direction::Vertical, 10, 5);
        let segs = make(&v, &v, &nets(1), (0, 0, 1000, 1000), None);
        assert_eq!(segs.len(), 4, "one net, four sides, all on one layer");
        assert!(segs.iter().all(|s| s.layer == "m5"));
        let horizontals = segs
            .iter()
            .filter(|s| s.rect.2 - s.rect.0 > s.rect.3 - s.rect.1)
            .count();
        assert_eq!(horizontals, 2, "a bottom and a top were still built");
    }

    #[test]
    fn a_layer_with_no_declared_direction_builds_the_vertical_sides() {
        // ⚠️ The test is `== Horizontal`, not `!= Vertical`, so an undirected layer takes the same
        // path a vertical one does rather than being a case of its own.
        let h = layer("m5", Direction::Horizontal, 10, 5);
        let n = layer("m6", Direction::None, 10, 5);
        let segs = make(&h, &n, &nets(1), (0, 0, 1000, 1000), None);
        let on_n: Vec<_> = segs.iter().filter(|s| s.layer == "m6").collect();
        assert_eq!(on_n.len(), 2);
        assert!(
            on_n.iter()
                .all(|s| s.rect.3 - s.rect.1 > s.rect.2 - s.rect.0),
            "left and right"
        );
    }

    #[test]
    fn two_vertical_layers_build_the_side_pair_twice_and_no_bottom_or_top() {
        // ⚠️ Reproduced deliberately. Neither pass satisfies the horizontal condition, so both fall
        // to the else branch. A ring on two vertical layers is a malformed request caught by the
        // layer checks before this point; what happens if it gets here is still worth pinning,
        // because an implementation that "helpfully" forces one pass horizontal is a different
        // engine.
        let v1 = layer("m4", Direction::Vertical, 10, 5);
        let v2 = layer("m6", Direction::Vertical, 10, 5);
        let segs = make(&v1, &v2, &nets(1), (0, 0, 1000, 1000), None);
        assert_eq!(segs.len(), 4);
        assert!(
            segs.iter()
                .all(|s| s.rect.3 - s.rect.1 > s.rect.2 - s.rect.0),
            "every side is a vertical one"
        );
    }

    #[test]
    fn only_a_single_layer_ring_is_locked() {
        let a = layer("m5", Direction::Horizontal, 10, 5);
        let b = layer("m6", Direction::Vertical, 10, 5);
        assert!(locked(&a, &a));
        assert!(!locked(&a, &b));
    }

    #[test]
    fn no_nets_makes_no_ring_rather_than_one_empty_loop() {
        let h = layer("m5", Direction::Horizontal, 10, 5);
        let v = layer("m6", Direction::Vertical, 10, 5);
        assert!(make(&h, &v, &[], (0, 0, 1000, 1000), None).is_empty());
    }

    // Rule (makeShapes since a718d1cd60): on a RECTANGLE the region builder is the old one exactly
    // — same shapes, same order — with and without extending to a boundary.
    #[test]
    fn the_region_builder_is_the_rectangular_one_on_a_rectangle() {
        let h = Layer { name: "m5".into(), direction: Direction::Horizontal, width: 100, spacing: 50 };
        let v = Layer { name: "m6".into(), direction: Direction::Vertical, width: 200, spacing: 70 };
        let nets = vec!["VDD".to_string(), "VSS".to_string()];
        let core = (1000, 2000, 9000, 8000);
        assert_eq!(make_region(&h, &v, &nets, &[core], None), make(&h, &v, &nets, core, None));
        let die = (0, 0, 10000, 10000);
        assert_eq!(make_region(&h, &v, &nets, &[core], Some(&[die])), make(&h, &v, &nets, core, Some(die)));
    }

    // Rule (makeShapes since a718d1cd60): on an L the bottom/top/left/right are walked per edge —
    // three horizontal sides on the outside of the notch become three segments, and the side meeting
    // the notch's concave corner is not extended into it.
    #[test]
    fn an_l_shaped_core_gets_a_ring_along_its_outline() {
        let h = Layer { name: "m5".into(), direction: Direction::Horizontal, width: 10, spacing: 10 };
        let v = Layer { name: "m6".into(), direction: Direction::Vertical, width: 10, spacing: 10 };
        let nets = vec!["VDD".to_string()];
        let l = [(0, 0, 100, 40), (0, 40, 40, 100)];
        let s = make_region(&h, &v, &nets, &l, None);
        let m5: Vec<Rect> = s.iter().filter(|s| s.layer == "m5").map(|s| s.rect).collect();
        assert_eq!(m5, vec![(-10, -10, 110, 0), (40, 40, 110, 50), (-10, 100, 50, 110)], "the notch end (x 40) is not extended");
        let m6: Vec<Rect> = s.iter().filter(|s| s.layer == "m6").map(|s| s.rect).collect();
        assert_eq!(m6, vec![(-10, -10, 0, 110), (100, -10, 110, 50), (40, 40, 50, 110)], "the notch end (y 40) is not extended");
    }

    // Rule (getRingReach): the band's own leg's ring, not the one across the notch — a side further
    // out than that ring's offset + width from the band is another edge's ring. Values: an L whose
    // narrow leg ends at x 87400, ring offset 2000 + total width 2400 on the right.
    #[test]
    fn a_rail_reaches_its_own_legs_ring_and_not_the_wide_legs() {
        let band = (14600, 114630, 87400, 114970);
        let own = (91200, 100000, 91800, 180000); // right side of the narrow leg, 4400 out
        let far = (175000, 0, 175400, 120000); // right side of the wide leg, reaching up past the band
        let top = (0, 180000, 200000, 180400); // a horizontal side: bounds y, not x
        let rings = vec![(2400, 2400, [2000; 4], vec![own, far, top])];
        assert_eq!(ring_reach(&rings, band, (1, 0)), 91800);
        assert_eq!(ring_reach(&rings, band, (-1, 0)), 14600, "nothing alongside on the left: the band's end");
        let short = vec![(2400, 2400, [1000; 4], vec![own])];
        assert_eq!(ring_reach(&short, band, (1, 0)), 87400, "4400 out is past a 3400 reach");
    }
}
