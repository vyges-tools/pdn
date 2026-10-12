// SPDX-License-Identifier: Apache-2.0
//! Rectilinear regions — the start of the polygon-floorplan port (upstream `Region`, `a718d1cd60`).
//!
//! A region is held as rectangles whose union is the area. Every question asked of one here is
//! about the union — containment, overlap, growth by a margin (which distributes over a union) — so
//! the decomposition chosen does not matter, and it need not match boost's.

use crate::Rect;

/// A rectilinear polygon's area as rectangles: one per maximal x interval in each horizontal band
/// between consecutive vertex ys, by even-odd crossing of its vertical edges. `points` is the ring
/// as odb gives it (closed or not).
pub fn polygon_rects(points: &[(i32, i32)]) -> Vec<Rect> {
    if points.len() < 3 {
        return Vec::new();
    }
    let n = points.len();
    let mut verticals: Vec<(i32, i32, i32)> = Vec::new(); // (x, ylo, yhi)
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        if a.0 == b.0 && a.1 != b.1 {
            verticals.push((a.0, a.1.min(b.1), a.1.max(b.1)));
        }
    }
    let mut ys: Vec<i32> = points.iter().map(|p| p.1).collect();
    ys.sort_unstable();
    ys.dedup();
    let mut out = Vec::new();
    for band in ys.windows(2) {
        let mid2 = band[0] + band[1]; // twice the band's mid y, so no rounding
        let mut xs: Vec<i32> = verticals.iter().filter(|v| 2 * v.1 < mid2 && mid2 < 2 * v.2).map(|v| v.0).collect();
        xs.sort_unstable();
        for pair in xs.chunks(2) {
            if let [x0, x1] = pair {
                if x0 < x1 {
                    out.push((*x0, band[0], *x1, band[1]));
                }
            }
        }
    }
    out
}

/// The area of `a` not covered by any of `cut`, as rectangles of positive area.
pub fn minus(a: &[Rect], cut: &[Rect]) -> Vec<Rect> {
    let mut left: Vec<Rect> = a.iter().copied().filter(|r| r.0 < r.2 && r.1 < r.3).collect();
    for &c in cut {
        let mut next = Vec::with_capacity(left.len());
        for r in left {
            if !(r.0 < c.2 && c.0 < r.2 && r.1 < c.3 && c.1 < r.3) {
                next.push(r);
                continue;
            }
            if r.1 < c.1 { next.push((r.0, r.1, r.2, c.1)); }
            if c.3 < r.3 { next.push((r.0, c.3, r.2, r.3)); }
            let (y0, y1) = (r.1.max(c.1), r.3.min(c.3));
            if r.0 < c.0 { next.push((r.0, y0, c.0, y1)); }
            if c.2 < r.2 { next.push((c.2, y0, r.2, y1)); }
        }
        left = next;
    }
    left
}

/// `Region::contains` — every part of `inner` inside `outer`.
pub fn contains(outer: &[Rect], inner: &[Rect]) -> bool {
    minus(inner, outer).is_empty()
}

/// `Region::getMarginBeyond(from, normal)` for a face of a rectangle: how far `region` reaches out
/// of that face, measured along its whole width — 0 when it does not pass the face at all, else the
/// distance to where the band first leaves the region. Searched over the region's own coordinates
/// on that axis, which is where a band can first leave it.
pub fn margin_beyond(region: &[Rect], from: Rect, normal: (i32, i32)) -> i32 {
    let band = |d: i32| -> Rect {
        match normal {
            (0, -1) => (from.0, from.1 - d, from.2, from.1),
            (0, 1) => (from.0, from.3, from.2, from.3 + d),
            (-1, 0) => (from.0 - d, from.1, from.0, from.3),
            _ => (from.2, from.1, from.2 + d, from.3),
        }
    };
    let mut cands: Vec<i32> = region
        .iter()
        .flat_map(|r| match normal {
            (0, -1) => [from.1 - r.1, from.1 - r.3],
            (0, 1) => [r.3 - from.3, r.1 - from.3],
            (-1, 0) => [from.0 - r.0, from.0 - r.2],
            _ => [r.2 - from.2, r.0 - from.2],
        })
        .filter(|d| *d > 0)
        .collect();
    cands.sort_unstable();
    cands.dedup();
    let mut best = 0;
    for d in cands {
        if contains(region, &[band(d)]) {
            best = d;
        } else {
            break;
        }
    }
    best
}

/// `Region::Edge` — a boundary edge walked with the interior on its LEFT, corner to corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub start: (i32, i32),
    pub end: (i32, i32),
    /// Outward normal: one of (±1, 0), (0, ±1).
    pub normal: (i32, i32),
    /// The corner at each end turns outward (interior angle 90), not inward (270).
    pub convex_at_start: bool,
    pub convex_at_end: bool,
}

fn turn(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> i64 {
    let (x1, y1) = ((b.0 - a.0) as i64, (b.1 - a.1) as i64);
    let (x2, y2) = ((c.0 - b.0) as i64, (c.1 - b.1) as i64);
    (x1 * y2 - y1 * x2).signum()
}

fn signed_area2(p: &[(i32, i32)]) -> i64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 as i64 * p[(i + 1) % n].1 as i64 - p[(i + 1) % n].0 as i64 * p[i].1 as i64).sum()
}

/// `toCornerLoop` — drop collinear vertices; at least four corners; counter-clockwise.
fn corner_loop(points: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut l: Vec<(i32, i32)> = points.to_vec();
    if l.len() > 1 && l.first() == l.last() {
        l.pop();
    }
    if l.len() < 3 {
        return Vec::new();
    }
    let n = l.len();
    let mut c: Vec<(i32, i32)> = (0..n).filter(|&i| turn(l[(i + n - 1) % n], l[i], l[(i + 1) % n]) != 0).map(|i| l[i]).collect();
    if c.len() < 4 {
        return Vec::new();
    }
    if signed_area2(&c) < 0 {
        c.reverse();
    }
    c
}

/// The OUTER boundaries of a union of rectangles, each as a counter-clockwise corner loop (interior
/// on the left). Holes are dropped — `fillHoles` — because every caller wants an outline.
///
/// Traced on the grid of the rectangles' own coordinates: a cell is in when its centre is in some
/// rectangle; each in-cell side facing an out-cell is a unit edge directed with the interior on its
/// left; edges chain end to start. At a vertex two edges leave (two pieces meeting at a corner) the
/// leftmost turn is taken, so the pieces close as separate loops.
pub fn outline_loops(rects: &[Rect]) -> Vec<Vec<(i32, i32)>> {
    let rects: Vec<Rect> = rects.iter().copied().filter(|r| r.0 < r.2 && r.1 < r.3).collect();
    if rects.is_empty() {
        return Vec::new();
    }
    let mut xs: Vec<i32> = rects.iter().flat_map(|r| [r.0, r.2]).collect();
    let mut ys: Vec<i32> = rects.iter().flat_map(|r| [r.1, r.3]).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let (nx, ny) = (xs.len() - 1, ys.len() - 1);
    let inside = |i: isize, j: isize| -> bool {
        if i < 0 || j < 0 || i as usize >= nx || j as usize >= ny {
            return false;
        }
        let (cx2, cy2) = (xs[i as usize] as i64 + xs[i as usize + 1] as i64, ys[j as usize] as i64 + ys[j as usize + 1] as i64);
        rects.iter().any(|r| 2 * r.0 as i64 <= cx2 && cx2 <= 2 * r.2 as i64 && 2 * r.1 as i64 <= cy2 && cy2 <= 2 * r.3 as i64)
    };
    let mut out_edges: std::collections::HashMap<(i32, i32), Vec<(i32, i32)>> = std::collections::HashMap::new();
    let mut add = |a: (i32, i32), b: (i32, i32)| out_edges.entry(a).or_default().push(b);
    for i in 0..nx as isize {
        for j in 0..ny as isize {
            if !inside(i, j) {
                continue;
            }
            let (x0, x1, y0, y1) = (xs[i as usize], xs[i as usize + 1], ys[j as usize], ys[j as usize + 1]);
            if !inside(i, j - 1) { add((x0, y0), (x1, y0)); }
            if !inside(i + 1, j) { add((x1, y0), (x1, y1)); }
            if !inside(i, j + 1) { add((x1, y1), (x0, y1)); }
            if !inside(i - 1, j) { add((x0, y1), (x0, y0)); }
        }
    }
    let mut loops = Vec::new();
    let mut starts: Vec<(i32, i32)> = out_edges.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        while out_edges.get(&s).is_some_and(|v| !v.is_empty()) {
            let mut lp = vec![s];
            let mut prev = s;
            let mut cur = out_edges.get_mut(&s).unwrap().remove(0);
            while cur != s {
                lp.push(cur);
                let next = {
                    let v = out_edges.get_mut(&cur).expect("a boundary edge always continues");
                    // leftmost turn first, then straight, then right
                    let k = (0..v.len()).max_by_key(|&k| turn(prev, cur, v[k])).unwrap();
                    v.remove(k)
                };
                prev = cur;
                cur = next;
            }
            let c = corner_loop(&lp);
            if !c.is_empty() && signed_area2(&lp) > 0 {
                loops.push(c);
            }
        }
    }
    loops
}

/// `Region::getEdges` over the outer loops: each edge with its outward normal (the direction turned
/// clockwise) and whether each end's corner is convex.
pub fn edges(loops: &[Vec<(i32, i32)>]) -> Vec<Edge> {
    let mut out = Vec::new();
    for c in loops {
        let n = c.len();
        for i in 0..n {
            let (prev, start, end, after) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n], c[(i + 2) % n]);
            let (dx, dy) = (end.0 - start.0, end.1 - start.1);
            out.push(Edge {
                start,
                end,
                normal: (dy.signum(), -dx.signum()),
                convex_at_start: turn(prev, start, end) > 0,
                convex_at_end: turn(start, end, after) > 0,
            });
        }
    }
    out
}

/// `Region::bloat` — grow each side independently, `(left, bottom, right, top)`; negative shrinks.
/// Growth distributes over a union, so each rectangle grows on its own; a shrink is the region
/// minus its complement grown the other way (erosion is the dual of dilation).
pub fn bloat(rects: &[Rect], m: [i32; 4]) -> Vec<Rect> {
    let [l, b, r, t] = m;
    let grown: Vec<Rect> = rects.iter().map(|x| (x.0 - l.max(0), x.1 - b.max(0), x.2 + r.max(0), x.3 + t.max(0))).collect();
    let (sl, sb, sr, st) = ((-l).max(0), (-b).max(0), (-r).max(0), (-t).max(0));
    if sl == 0 && sb == 0 && sr == 0 && st == 0 {
        return grown;
    }
    let Some(e) = grown.iter().copied().reduce(|a, c| (a.0.min(c.0), a.1.min(c.1), a.2.max(c.2), a.3.max(c.3))) else {
        return grown;
    };
    let pad = sl.max(sb).max(sr).max(st) + 1;
    let frame = (e.0 - pad, e.1 - pad, e.2 + pad, e.3 + pad);
    let comp = minus(&[frame], &grown);
    // shrinking the LEFT side by sl removes what lies within sl to the right of an outside point
    let eat: Vec<Rect> = comp.iter().map(|c| (c.0 - sr, c.1 - st, c.2 + sl, c.3 + sb)).collect();
    minus(&grown, &eat)
}

/// `Region::getMarginBeyond(from, normal)`, transcribed: the band out of that face to `reach`
/// (beyond any side of the region), minus the region; the nearest blocked piece is the margin, or
/// `reach` if nothing blocks; never negative.
pub fn margin_beyond_exact(region: &[Rect], from: Rect, normal: (i32, i32)) -> i32 {
    let Some(b) = region.iter().copied().reduce(|a, c| (a.0.min(c.0), a.1.min(c.1), a.2.max(c.2), a.3.max(c.3))) else {
        return 0;
    };
    let reach = (b.2 - b.0) + (b.3 - b.1) + 1;
    let band = match normal {
        (_, y) if y > 0 => (from.0, from.3, from.2, from.3 + reach),
        (_, y) if y < 0 => (from.0, from.1 - reach, from.2, from.1),
        (x, _) if x > 0 => (from.2, from.1, from.2 + reach, from.3),
        _ => (from.0 - reach, from.1, from.0, from.3),
    };
    if band.2 - band.0 <= 0 || band.3 - band.1 <= 0 {
        return 0;
    }
    let blocked = minus(&[band], region);
    if blocked.is_empty() {
        return reach;
    }
    blocked.iter().map(|r| match normal {
        (_, y) if y > 0 => r.1 - from.3,
        (_, y) if y < 0 => from.1 - r.3,
        (x, _) if x > 0 => r.0 - from.2,
        _ => from.0 - r.2,
    }).min().unwrap().max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // An L die: the union of the rectangles is the polygon, and a notch stays out of it.
    #[test]
    fn an_l_shaped_die_decomposes_to_its_area() {
        let l = [(0, 0), (100, 0), (100, 40), (40, 40), (40, 100), (0, 100)];
        let r = polygon_rects(&l);
        let area: i64 = r.iter().map(|r| (r.2 - r.0) as i64 * (r.3 - r.1) as i64).sum();
        assert_eq!(area, 100 * 40 + 40 * 60);
        assert!(crate::channels::covered_by((0, 0, 40, 100), &r));
        assert!(!crate::channels::covered_by((0, 0, 50, 50), &r), "reaches into the notch");
        let closed = [(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)];
        assert_eq!(polygon_rects(&closed), vec![(0, 0, 10, 10)]);
    }

    // Rules (toCornerLoop + getEdges): an L traced as ONE counter-clockwise loop of six corners;
    // outward normals; the notch corner is the only concave one.
    #[test]
    fn an_l_outline_has_six_edges_and_one_concave_corner() {
        let l = vec![(0, 0, 100, 40), (0, 40, 40, 100)];
        let loops = outline_loops(&l);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 6);
        let e = edges(&loops);
        let concave: Vec<(i32, i32)> = e.iter().filter(|e| !e.convex_at_end).map(|e| e.end).collect();
        assert_eq!(concave, vec![(40, 40)]);
        let bottom = e.iter().find(|e| e.normal == (0, -1)).unwrap();
        assert_eq!((bottom.start, bottom.end), ((0, 0), (100, 0)), "interior on the left: bottom runs +x");
        assert_eq!(outline_loops(&[(0, 0, 10, 10), (0, 0, 10, 10)]), vec![vec![(0, 0), (10, 0), (10, 10), (0, 10)]], "duplicates are one square");
    }

    // Rules (fillHoles, bloat): a ring of rows round a macro is one outline with the hole dropped;
    // growing and shrinking per side.
    #[test]
    fn holes_are_filled_and_sides_grow_and_shrink_independently() {
        let ring = vec![(0, 0, 30, 10), (0, 20, 30, 30), (0, 10, 10, 20), (20, 10, 30, 20)];
        let loops = outline_loops(&ring);
        assert_eq!(loops.len(), 1, "the hole is not a second outline");
        assert_eq!(loops[0].len(), 4);
        let g = bloat(&[(0, 0, 10, 10)], [1, 2, 3, 4]);
        assert_eq!(g, vec![(-1, -2, 13, 14)]);
        let s = bloat(&[(0, 0, 10, 10)], [-1, -2, 0, 0]);
        let area: i64 = s.iter().map(|r| (r.2 - r.0) as i64 * (r.3 - r.1) as i64).sum();
        assert_eq!(area, 9 * 8, "left in by 1, bottom up by 2");
        assert!(contains(&[(1, 2, 10, 10)], &s) && contains(&s, &[(1, 2, 10, 10)]));
        assert_eq!(margin_beyond_exact(&[(0, 0, 100, 100)], (10, 10, 20, 20), (0, -1)), 10);
    }
}
