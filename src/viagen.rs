// SPDX-License-Identifier: Apache-2.0
//! Choosing which via to build.
//!
//! A place that needs a via usually admits several: different rules from the technology, different
//! cut counts, different enclosures. This module is the choosing — two preference orders that look
//! alike, are spelled alike, and run in opposite directions.
//!
//! Nothing here touches a database.

use crate::Direction;

/// How far a layer's metal must extend past the cut, in each axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enclosure {
    pub x: i32,
    pub y: i32,
}

impl Enclosure {
    /// **E1** — is this enclosure preferred over another?
    ///
    /// ⚠️ **Preferred means SMALLER.** Metal past the cut is metal in the way, so the tightest
    /// enclosure wins. This is the exact opposite of [`Generator::is_preferred_over`], which picks
    /// the *largest* — two methods of the same name, in the same file, running in opposite
    /// directions. Copying one into the other is a mistake that produces entirely plausible vias.
    ///
    /// ⚠️ **Which axis is minimised first comes from the layer's direction, tested as
    /// `!= Horizontal`.** A layer with no declared direction therefore minimises x, exactly as a
    /// vertical one does, rather than being a case of its own.
    ///
    /// The other axis is the tie-break, and there is no third level: two enclosures equal in both
    /// are not preferred over each other, so the incumbent stays.
    pub fn is_preferred_over(&self, other: Option<&Enclosure>, layer: Direction) -> bool {
        self.is_preferred_over_axis(other, layer != Direction::Horizontal)
    }

    pub fn is_preferred_over_axis(&self, other: Option<&Enclosure>, minimize_x: bool) -> bool {
        let Some(o) = other else {
            return true; // nothing to beat
        };
        if minimize_x {
            if self.x == o.x {
                return self.y < o.y;
            }
            return self.x < o.x;
        }
        if self.y == o.y {
            return self.x < o.x;
        }
        self.y < o.y
    }
}

/// One candidate way of building a via, reduced to what the preference order actually reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generator {
    pub name: String,
    pub cut_area: i32,
    /// `(width, height)` of the via's footprint on the bottom layer.
    pub bottom: (i32, i32),
    pub top: (i32, i32),
    pub bottom_direction: Direction,
    pub top_direction: Direction,
}

impl Generator {
    /// The dimension the layer's own direction makes significant.
    ///
    /// ⚠️ **A HORIZONTAL layer is measured on its HEIGHT.** That reads backwards and is right: a
    /// horizontal wire's height is its width as a conductor, which is the constrained dimension.
    /// Inverting this compiles, runs, and picks the wrong via on every layer.
    fn preferred(dim: (i32, i32), direction: Direction) -> i32 {
        if direction == Direction::Horizontal {
            dim.1
        } else {
            dim.0
        }
    }

    fn non_preferred(dim: (i32, i32), direction: Direction) -> i32 {
        if direction == Direction::Horizontal {
            dim.0
        } else {
            dim.1
        }
    }

    /// **E2** — is this generator preferred over another?
    ///
    /// A five-level lexicographic order, **larger winning at every level**:
    ///
    /// 1. cut area
    /// 2. the bottom layer's preferred-direction dimension
    /// 3. the top layer's preferred-direction dimension
    /// 4. the bottom layer's other dimension
    /// 5. the top layer's other dimension
    ///
    /// ⚠️ **A complete tie returns `false`** — "not preferred over" — so an incumbent is kept and
    /// the order among equals falls to whoever was built first. That is what makes the sort's
    /// stability load-bearing rather than incidental.
    pub fn is_preferred_over(&self, other: Option<&Generator>) -> bool {
        let Some(o) = other else {
            return true;
        };
        if self.cut_area != o.cut_area {
            return self.cut_area > o.cut_area;
        }
        let levels = [
            (
                Self::preferred(self.bottom, self.bottom_direction),
                Self::preferred(o.bottom, o.bottom_direction),
            ),
            (
                Self::preferred(self.top, self.top_direction),
                Self::preferred(o.top, o.top_direction),
            ),
            (
                Self::non_preferred(self.bottom, self.bottom_direction),
                Self::non_preferred(o.bottom, o.bottom_direction),
            ),
            (
                Self::non_preferred(self.top, self.top_direction),
                Self::non_preferred(o.top, o.top_direction),
            ),
        ];
        for (mine, theirs) in levels {
            if mine != theirs {
                return mine > theirs;
            }
        }
        false
    }
}

/// **E3** — the generator to build, out of everything that could be built here.
///
/// ⚠️ **A STABLE sort, and the first afterwards.** With `is_preferred_over` returning `false` for a
/// complete tie, equally-good candidates keep the order they were constructed in — so the winner
/// among equals is whichever rule the technology offered first. An unstable sort picks an arbitrary
/// one of them and is right by luck.
pub fn best(generators: &[Generator]) -> Option<&Generator> {
    let mut order: Vec<&Generator> = generators.iter().collect();
    // A stable sort where "less" means "preferred", so the winner ends up first.
    order.sort_by(|a, b| {
        if a.is_preferred_over(Some(b)) {
            std::cmp::Ordering::Less
        } else if b.is_preferred_over(Some(a)) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    order.first().copied()
}

/// One candidate's cut count and the enclosures it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub cuts: i32,
    pub bottom: Enclosure,
    pub top: Enclosure,
}

/// **E4** — pick the enclosure pair, given candidates in the order they were tried.
///
/// More cuts always wins. On a tie in cut count the **bottom** enclosure decides, and only if the
/// bottom does not prefer the new candidate is the **top** consulted.
///
/// ⚠️ **The pair is saved together.** When the bottom wins, its top comes with it even if that top
/// is worse than the incumbent's. Choosing the best bottom and the best top independently is a
/// different engine, and produces a pair that no single candidate ever offered.
///
/// ⚠️ The first candidate is always taken, because the incumbent starts at zero cuts — not because
/// of a special case for emptiness.
pub fn best_enclosures(candidates: &[Candidate]) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;
    for c in candidates {
        let save = match best {
            None => true,
            Some(b) if b.cuts == c.cuts => {
                c.bottom.is_preferred_over_axis(Some(&b.bottom), true)
                    || c.top.is_preferred_over_axis(Some(&b.top), true)
            }
            Some(b) => b.cuts < c.cuts,
        };
        if save {
            best = Some(*c);
        }
    }
    best
}

/// The pair a via is built with, and what it yields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chosen {
    pub bottom: Enclosure,
    pub top: Enclosure,
    pub cuts: i32,
}

/// **E20** — choose the enclosure pair, over the cross product of both sides' candidates.
///
/// 🔑 **Rows and columns are recomputed for EVERY pair**, because the enclosure decides how many
/// cuts fit. Working out the cut count once and then choosing an enclosure is a different engine:
/// it picks a pair that could not have produced that count.
///
/// ⚠️ **Only pairs that pass the constraints are considered at all**, and if none does the via is
/// not buildable — `None` here is the reference returning `false` from `build`, which drops the
/// candidate rather than falling back to something.
///
/// The save rule, in order: more cuts always wins; on a tie the **bottom** enclosure decides, and
/// only if the bottom does not prefer the new pair is the **top** consulted. ⚠️ The pair is saved
/// **together** — a winning bottom brings its top along even when that top is worse.
pub fn best_enclosure_pair(
    bottoms: &[Enclosure],
    tops: &[Enclosure],
    bottom_layer: Direction,
    top_layer: Direction,
    cuts_for: &dyn Fn(Enclosure, Enclosure) -> i32,
    passes: &dyn Fn(Enclosure, Enclosure, i32) -> bool,
) -> Option<Chosen> {
    let mut best: Option<Chosen> = None;
    for &b in bottoms {
        for &t in tops {
            let cuts = cuts_for(b, t);
            if !passes(b, t, cuts) {
                continue;
            }
            let save = match best {
                None => true,
                Some(w) if w.cuts == cuts => {
                    b.is_preferred_over(Some(&w.bottom), bottom_layer)
                        || t.is_preferred_over(Some(&w.top), top_layer)
                }
                Some(w) => w.cuts < cuts,
            };
            if save {
                best = Some(Chosen {
                    bottom: b,
                    top: t,
                    cuts,
                });
            }
        }
    }
    best
}

/// Whether a via must fit inside the shape on each axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Constraint {
    pub must_fit_x: bool,
    pub must_fit_y: bool,
}

/// **E21** — the constraint a shape imposes at its own end of a stack.
///
/// ⚠️ **A shape need not fit along its own length.** A horizontal shape runs the length of x, so
/// the via has all the room it wants there and only has to fit in **y**; a vertical shape is the
/// mirror. Setting both, or neither, changes which axis takes the overlap enclosure below.
///
/// ⚠️ **A shape that cannot be modified, or that already carries terminal connections, must fit on
/// BOTH axes** — nothing can be widened to accommodate the via.
pub fn constraint_for(shape: Direction, modifiable: bool, has_iterms: bool) -> Constraint {
    if !modifiable || has_iterms {
        return Constraint {
            must_fit_x: true,
            must_fit_y: true,
        };
    }
    Constraint {
        must_fit_x: shape != Direction::Horizontal,
        must_fit_y: shape != Direction::Vertical,
    }
}

/// **E26** — which of a generate rule's two routing layers is the BOTTOM.
///
/// 🔑 **By layer number, never by the order the LEF declares them.**
/// `GenerateViaGenerator`'s constructor collects the rule's three layers, sorts them with
/// `l->getNumber() < r->getNumber()` and indexes bottom / cut / top out of that.
///
/// ⚠️ **A real technology writes one the other way round.** Nangate45's `VIARULE Via9Array-0
/// GENERATE` names `metal10` before `metal9`, alone among its twenty rules. Taken in order it is a
/// metal10 → metal9 rule, the metal9/metal10 pair has no rule at all, and every via on that pair
/// falls through to the technology's own — a path the reference takes only when no rule could be
/// built.
///
/// Returns the indices of (bottom, top) into the rule's layer list. Ties keep declaration order,
/// as a stable sort does.
pub fn rule_layer_order(first_level: i32, second_level: i32) -> (usize, usize) {
    if first_level <= second_level {
        (0, 1)
    } else {
        (1, 0)
    }
}

/// **E24** — `TechViaGenerator::fitsShapes`: may the technology's own via be used here at all?
///
/// 🔑 **This is a SETUP test, not a build one, and that is why it can refuse what nothing else
/// can.** `makeSingleLayerVia` filters its tech-via candidates through `isSetupValid` before
/// `generateDbVia` ever calls `build()`, and `TechViaGenerator::isSetupValid` ends in
/// `fitsShapes()`. A via that fails here never becomes a candidate, so there is no enclosure
/// arithmetic to save it.
///
/// The test is on the via's OWN metal as the technology declares it — `DbTechVia(via, 1, 0, 1, 0)`
/// and `getViaRect(true, false, ...)`, which is `enc_bottom_rect_` / `enc_top_rect_` verbatim —
/// translated to the centre of the intersection. Not the enclosure the via would be *built* with:
/// that is computed later and is a different number entirely.
///
/// ⚠️ **`check_rect` is the INTERSECTION**, since `intersection_only` is true on every constraint
/// this engine makes. The reference clears it only for a shape that is unmodifiable or already
/// carries terminal connections, which is also the branch that sets both `must_fit` flags — see
/// [`constraint_for`]. `full_shape` still governs the three-sided fallback below.
///
/// ⚠️ **The fallback is reachable only from INSIDE a stack.** At either end `constraint_for`
/// always sets at least one axis, so the counting branch belongs to an intermediate level, which
/// has no shape to land on and may bridge along the layer's own routing direction.
pub fn mostly_contains(
    full_shape: crate::Rect,
    intersection: crate::Rect,
    small_shape: crate::Rect,
    c: Constraint,
    layer: Direction,
) -> bool {
    let inside_left = intersection.0 <= small_shape.0;
    let inside_bottom = intersection.1 <= small_shape.1;
    let inside_right = intersection.2 >= small_shape.2;
    let inside_top = intersection.3 >= small_shape.3;
    let inside_x = inside_left && inside_right;
    let inside_y = inside_bottom && inside_top;

    if c.must_fit_x && c.must_fit_y {
        return inside_x && inside_y;
    }
    if c.must_fit_x {
        return inside_x;
    }
    if c.must_fit_y {
        return inside_y;
    }

    // Three sides of four is enough for a level with nothing to fit inside.
    let contains = [
        full_shape.3 >= small_shape.3,
        full_shape.2 >= small_shape.2,
        full_shape.1 <= small_shape.1,
        full_shape.0 <= small_shape.0,
    ]
    .iter()
    .filter(|c| **c)
    .count();
    if contains > 2 {
        return true;
    }
    match layer {
        Direction::Horizontal => inside_y,
        Direction::Vertical => inside_x,
        Direction::None => false,
    }
}

/// **E22** — the enclosure actually built, from the minimum and the room available.
///
/// 🔑 **The chosen enclosure is a MINIMUM, not the answer.** Where the via must fit an axis, the
/// enclosure written is the *overlap* — half of whatever the shape has left over after the cut
/// array — and where it need not, the minimum stands.
///
/// ⚠️ **`use_min` overrides everything**, and it is set for a layer internal to the stack. So an
/// intermediate level takes minimums on both axes however much room it has, which is why a via in
/// the middle of a stack commonly carries no overhang at all while the ends carry plenty.
pub fn built_enclosure(
    use_min: bool,
    minimum: Enclosure,
    overlap: Enclosure,
    c: Constraint,
) -> Enclosure {
    if use_min {
        return minimum;
    }
    Enclosure {
        x: if c.must_fit_x { overlap.x } else { minimum.x },
        y: if c.must_fit_y { overlap.y } else { minimum.y },
    }
}

/// **E27** — the enclosure the via is finally built with sits on the MANUFACTURING GRID.
///
/// 🔑 **It is the last thing `determineRowsAndColumns` does**, after the array branch, the plain
/// branch and the split-cut override alike — so nothing downstream ever sees an unsnapped
/// enclosure, the candidate check included.
///
/// ⚠️ **Rounded DOWN, and that is the point.** The overlap enclosure is half of whatever room the
/// shape has left, so on a grid of 10 a 90-unit remainder gives 45 and the metal comes out 370
/// tall — five units past each end of a 360-tall opening. Snapping down gives 40, and the metal
/// fits. Rounding up, or not snapping at all, puts metal outside the shape it lands on and the
/// via is ripped up at write time for it.
pub fn snap_enclosure(e: Enclosure, manufacturing_grid: i32) -> Enclosure {
    Enclosure {
        x: crate::straps::snap_to_manufacturing_grid(e.x, manufacturing_grid, false),
        y: crate::straps::snap_to_manufacturing_grid(e.y, manufacturing_grid, false),
    }
}

/// **E23** — the overlap enclosure: half the room the cut array leaves in the shape.
///
/// ⚠️ Halved with integer division, and **not clamped**: a shape narrower than its own cut array
/// yields a negative value, which the reference carries through rather than treating as zero.
/// **E30** — the enclosure a shape has OUTSIDE the intersection, per layer.
///
/// 🔑 **The intersection is not the whole shape, and measuring enclosure from it alone throws away
/// real metal.** A follow pin 0.054 wide crossing one 0.018 wide intersects over 0.018 — exactly
/// the cut — so the wider layer appears to have zero enclosure and every via on the grid is
/// refused, while 0.018 of metal sits either side doing nothing. This is the room that is there.
///
/// Returned as `(bottom, top)`: how far the LOWER shape reaches past the upper, and the upper past
/// the lower.
///
/// ⚠️ **The SMALLER of the two sides, floored at zero.** An enclosure is symmetric about the cut,
/// so a shape reaching far past on one side and flush on the other has no usable spare — taking the
/// larger side, or their sum, would claim metal that is not there on the side that matters.
///
/// ⚠️ **The two are exact opposites**, so at most one can be positive on a given axis: whichever
/// shape is wider has the spare and the other has none.
pub fn spare_enclosure(lower: crate::Rect, upper: crate::Rect) -> (Enclosure, Enclosure) {
    let x_lo = upper.0 - lower.0;
    let x_hi = lower.2 - upper.2;
    let y_lo = upper.1 - lower.1;
    let y_hi = lower.3 - upper.3;
    (
        Enclosure {
            x: 0.max(x_lo.min(x_hi)),
            y: 0.max(y_lo.min(y_hi)),
        },
        Enclosure {
            x: 0.max((-x_lo).min(-x_hi)),
            y: 0.max((-y_lo).min(-y_hi)),
        },
    )
}

/// **E31** — the enclosure to try on a side whose rule the intersection alone cannot satisfy.
///
/// 🔑 **Capped at what the rule ASKS, never at what the shape HAS.** The spare is permission to
/// meet the requirement, not licence to claim every spare unit — upstream takes
/// `min(required, built + spare)`, so a shape with plenty of room still reports exactly the
/// enclosure the rule wanted.
///
/// ⚠️ **A via admitted this way MUST NOT BE CACHED.** The answer depends on where the two shapes
/// actually are, not merely on how big their overlap is, so a cache keyed on the crossing's size
/// would hand this via to a crossing of the same size with no spare beside it.
pub fn spare_applied(built: Enclosure, required: Enclosure, spare: Enclosure) -> Enclosure {
    Enclosure {
        x: required.x.min(built.x + spare.x),
        y: required.y.min(built.y + spare.y),
    }
}

pub fn overlap_enclosure(extent: (i32, i32), span: (i32, i32)) -> Enclosure {
    Enclosure {
        x: (extent.0 - span.0) / 2,
        y: (extent.1 - span.1) / 2,
    }
}

/// **E28** — how many cuts of an array count as ADJACENT, from its shape alone.
///
/// 🔑 **Not a distance test.** `ViaGenerator::updateCutSpacing` clamps both dimensions to `1..=4`
/// and reads the answer off a table: a cut in the middle of a 3xN or wider array has four
/// neighbours, and nothing narrower reaches four however long it runs.
///
/// ⚠️ **A 1xN array counts TWO**, not one — the cut in the middle of a row has a neighbour on each
/// side. Only a 1x1 and a 1x2 fall below the two that every rule requires.
pub fn adjacent_cuts(rows: i32, columns: i32) -> i32 {
    let rows = rows.clamp(1, 4);
    let columns = columns.clamp(1, 4);
    let (min_dim, max_dim) = (rows.min(columns), rows.max(columns));
    match min_dim {
        1 if max_dim == 2 => 1,
        1 if max_dim >= 3 => 2,
        2 if max_dim == 2 => 2,
        2 if max_dim >= 3 => 3,
        3 | 4 => 4,
        _ => 0,
    }
}

/// **E29** — the cut pitch an ADJACENTCUTS rule imposes on an array of this shape.
///
/// 🔑 **A wide enough array stops using the cut layer's plain `SPACING`.** Every rule whose cut
/// count the array reaches applies, and the LAST matching one wins — the reference assigns rather
/// than accumulates.
///
/// ⚠️ **`EXCEPTSAMEPGNET` rules are skipped outright**, not applied conditionally: a power grid is
/// exactly the same-net case those rules exempt.
///
/// ⚠️ **Below two adjacent cuts nothing applies at all**, whatever the rules say, so a 1x1 or 1x2
/// array keeps its pitch.
///
/// Returns the new pitch for both axes, or `None` where no rule bites. `rules` is
/// `(cuts, spacing, except_same_pgnet)`.
pub fn adjacent_cut_pitch(
    rows: i32,
    columns: i32,
    cut: (i32, i32),
    rules: &[(u32, i32, bool)],
) -> Option<(i32, i32)> {
    let adj = adjacent_cuts(rows, columns);
    if adj < 2 {
        return None;
    }
    let mut out = None;
    for (cuts, spacing, except_same_pgnet) in rules {
        if *except_same_pgnet || *cuts as i32 > adj {
            continue;
        }
        out = Some((cut.0 + spacing, cut.1 + spacing));
    }
    out
}

/// **E5** — how many cuts fit across a wire of this width.
///
/// ⚠️ **The LARGER of the two enclosures is used on both sides.** Not each layer's own: a via has
/// to satisfy both, so the tighter one is irrelevant. Taking `bot_enc` alone, or averaging, fits
/// cuts that cannot legally be built.
///
/// ⚠️ **The first cut is free.** The cut's own width is subtracted once, then the remainder is
/// divided by the pitch to get the *additional* cuts. `available / pitch` alone is off by one, and
/// a zero pitch means exactly one cut rather than a division by zero.
///
/// `max_cuts` of zero means unlimited, so an absent limit and an explicit zero are the same thing.
pub fn cuts_across(
    width: i32,
    cut: i32,
    bot_enc: i32,
    top_enc: i32,
    pitch: i32,
    max_cuts: i32,
) -> i32 {
    let max_enc = bot_enc.max(top_enc);
    let mut available = width - 2 * max_enc;
    if available < 0 {
        return 0;
    }
    available -= cut;
    if available < 0 {
        return 0;
    }
    if pitch == 0 {
        return 1;
    }
    let cuts = available / pitch + 1;
    if max_cuts != 0 {
        return cuts.min(max_cuts);
    }
    cuts
}

/// **E6** — the width a run of cuts occupies, enclosure included.
///
/// ⚠️ `spacing * (cuts - 1)`: there are one fewer gaps than cuts. And zero cuts occupy **nothing**,
/// not two enclosures' worth — a via that does not exist has no metal.
pub fn cuts_width(cuts: i32, cut_width: i32, spacing: i32, enc: i32) -> i32 {
    if cuts == 0 {
        return 0;
    }
    cut_width * cuts + spacing * (cuts - 1) + 2 * enc
}

/// A LEF58 cut class: a named cut shape the technology's rules are written against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutClass {
    pub name: String,
    pub width: i32,
    /// `None` where the rule declares no length, in which case it is square.
    pub length: Option<i32>,
}

/// **E7** — which cut class a cut of this size belongs to.
///
/// ⚠️ **Either orientation matches.** A rule of width 40 and length 80 is matched by a cut of
/// 40x80 *and* by one of 80x40 — the class describes a shape, not an orientation.
///
/// ⚠️ **The first match wins** and the search stops, so the order the technology declares its
/// classes in decides the answer where two could match.
pub fn cut_class<'a>(classes: &'a [CutClass], cut: (i32, i32)) -> Option<&'a CutClass> {
    classes.iter().find(|r| {
        let length = r.length.unwrap_or(r.width);
        (cut.0 == length && cut.1 == r.width) || (cut.0 == r.width && cut.1 == length)
    })
}

/// A LEF minimum-cut rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinCutRule {
    /// The wire width at or above which the rule applies.
    pub width: i32,
    pub cuts: i32,
    pub above: bool,
    pub below: bool,
    /// `None` applies to any cut class.
    pub cut_class: Option<String>,
}

/// **E8** — does this via satisfy the layer's minimum-cut rules?
///
/// ⚠️ **Only the widest applicable width group is consulted**, and the test is `rule_width < width`
/// — strictly less. A rule written exactly at the wire's width does **not** apply to it.
///
/// ⚠️ **Within that group, ANY rule passing is enough.** The result is an OR, not an AND: a via
/// satisfying one of several alternatives at that width is valid. Requiring all of them rejects
/// vias the technology permits.
///
/// ⚠️ **No applicable rule means valid**, not invalid.
pub fn check_min_cuts(
    rules: &[MinCutRule],
    class: Option<&str>,
    width: i32,
    total_cuts: i32,
    is_below: bool,
) -> bool {
    let applicable: Vec<&MinCutRule> = rules
        .iter()
        .filter(|r| r.cut_class.as_deref() == class)
        .filter(|r| {
            if r.below {
                is_below
            } else if r.above {
                !is_below
            } else {
                true
            }
        })
        .filter(|r| r.width < width)
        .collect();
    let Some(widest) = applicable.iter().map(|r| r.width).max() else {
        return true; // no rule applies, so nothing to violate
    };
    applicable
        .iter()
        .filter(|r| r.width == widest)
        .any(|r| r.cuts <= total_cuts)
}

/// **E25** — `checkMinEnclosure`: does the enclosure the via was BUILT with satisfy a rule?
///
/// 🔑 **Any one rule is enough, and no rules at all is a pass.**
///
/// ⚠️ **The RULE-derived set only.** `getMinimumEnclosures(..., rules_only = true)` asks the cut
/// layer's own enclosure rules; a generate rule's stated `ENCLOSURE` is not among them. So a rule
/// declaring `0 0` does not make the check vacuous — it still demands a **non-negative** enclosure,
/// which is exactly what rejects a via whose cut is taller than the rect it has to sit in.
///
/// ⚠️ **Swap is per rule.** A `DEFAULT` rule, and a technology via's own layer rule, are built by
/// `Enclosure::swap` and may be met in either orientation; `EOL`, `ENDSIDE` and `HORZ_AND_VERT`
/// fix the axes and must be met as stated.
pub fn enclosure_satisfies(built: Enclosure, rules: &[(Enclosure, bool)]) -> bool {
    if rules.is_empty() {
        return true;
    }
    rules.iter().any(|(r, allow_swap)| {
        (r.x <= built.x && r.y <= built.y) || (*allow_swap && r.x <= built.y && r.y <= built.x)
    })
}

/// **E9** — the constraint gate, in order.
///
/// ⚠️ Each check is separately switchable and they run in a fixed order: no cuts at all, then
/// minimum cuts, then minimum enclosure. The order is only visible in *which* reason a rejection
/// reports, but that reason is what a debug trace shows and what a comparison against one reads.
pub fn check_constraints(
    total_cuts: i32,
    min_cuts_ok: bool,
    enclosure_ok: bool,
    check_cuts: bool,
    check_min_cut: bool,
    check_enclosure: bool,
) -> Result<(), &'static str> {
    if check_cuts && total_cuts == 0 {
        return Err("generates no vias");
    }
    if check_min_cut && !min_cuts_ok {
        return Err("violates minimum cut rules");
    }
    if check_enclosure && !enclosure_ok {
        return Err("violates minimum enclosure rules");
    }
    Ok(())
}

/// A LEF58 cut-enclosure rule, reduced to what the selection reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnclosureRule {
    pub name: String,
    /// `None` applies to any cut class.
    pub cut_class: Option<String>,
    pub above: bool,
    pub below: bool,
    /// `None` where the rule declares no width, which behaves as zero.
    pub min_width: Option<i32>,
}

/// What kind of enclosure rule this is, which decides how its two overhangs map to x and y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncType {
    Default,
    Eol,
    EndSide,
    HorzAndVert,
}

/// **E17** — orient an enclosure so its smaller overhang sits on the layer's constrained axis.
///
/// ⚠️ **`NONE` is grouped with HORIZONTAL here**, and with VERTICAL in
/// [`Enclosure::is_preferred_over`]. Two direction tests in the same class that split the
/// undirected case in opposite directions — a `!=` in one and a `==` in the other. Making them
/// agree is a one-character change that moves every undirected layer's enclosure.
pub fn swap_for_layer(e: Enclosure, layer: Direction) -> Enclosure {
    if layer == Direction::Vertical {
        if e.x > e.y {
            return Enclosure { x: e.y, y: e.x };
        }
    } else if e.y > e.x {
        return Enclosure { x: e.y, y: e.x };
    }
    e
}

/// **E18** — the enclosure a LEF58 rule asks for.
///
/// ⚠️ **Only `Default` is re-oriented afterwards.** The other three fix their own axes and are
/// taken as they stand — the rule has already said which overhang belongs where, and swapping it
/// would undo that.
///
/// - `Default` — first to x, second to y, then oriented for the layer.
/// - `Eol` — by the **rectangle's** direction, not the layer's: horizontal keeps the order,
///   vertical reverses it, and ⚠️ undirected takes the **larger of the two on both axes** rather
///   than either one.
/// - `EndSide` — by the CUT's own shape: a cut taller than it is wide reverses the pair.
/// - `HorzAndVert` — first to x, second to y, untouched.
pub fn enclosure_from_rule(
    kind: EncType,
    first: i32,
    second: i32,
    cut: (i32, i32),
    layer: Direction,
    rect: Direction,
) -> Enclosure {
    match kind {
        EncType::Default => swap_for_layer(
            Enclosure {
                x: first,
                y: second,
            },
            layer,
        ),
        EncType::Eol => match rect {
            Direction::Horizontal => Enclosure {
                x: first,
                y: second,
            },
            Direction::Vertical => Enclosure {
                x: second,
                y: first,
            },
            Direction::None => {
                let m = first.max(second);
                Enclosure { x: m, y: m }
            }
        },
        EncType::EndSide => {
            if cut.0 < cut.1 {
                Enclosure {
                    x: second,
                    y: first,
                }
            } else {
                Enclosure {
                    x: first,
                    y: second,
                }
            }
        }
        EncType::HorzAndVert => Enclosure {
            x: first,
            y: second,
        },
    }
}

/// **E10** — the direction a rectangle implies.
///
/// ⚠️ **A square is `None`, not a default.** Three outcomes, not two: taller than wide is vertical,
/// wider than tall is horizontal, and equal is genuinely undirected. Collapsing the third into
/// either of the others picks the wrong enclosure axis for every square via.
pub fn rect_direction(rect: crate::Rect) -> Direction {
    let (w, h) = (rect.2 - rect.0, rect.3 - rect.1);
    if w < h {
        Direction::Vertical
    } else if h < w {
        Direction::Horizontal
    } else {
        Direction::None
    }
}

/// **E11** — the enclosure rules that govern a via of this width, on this side.
///
/// ⚠️ **A rule declaring neither `above` nor `below` governs BOTH.** Reading the two flags as a
/// two-way choice drops every unqualified rule, and unqualified is the common case.
///
/// ⚠️ **Selection is `min_width <= width`** — and note the contrast with the minimum-cut rules,
/// which select on `rule_width < width`, strictly. Two sibling selections in the same file that
/// differ by an equals sign. Making them consistent, in either direction, changes which rules apply
/// at exactly the boundary width.
///
/// As with min-cut, only the **widest** qualifying group is returned, and no qualifying group means
/// no rules rather than an error.
pub fn enclosure_rules<'a>(
    rules: &'a [EnclosureRule],
    class: Option<&str>,
    width: i32,
    above: bool,
) -> Vec<&'a EnclosureRule> {
    let applicable: Vec<&EnclosureRule> = rules
        .iter()
        .filter(|r| r.cut_class.as_deref() == class)
        .filter(|r| {
            let (mut top, mut bot) = (r.above, r.below);
            if !top && !bot {
                top = true;
                bot = true;
            }
            (above && top) || (!above && bot)
        })
        .filter(|r| r.min_width.unwrap_or(0) <= width)
        .collect();
    let Some(widest) = applicable.iter().map(|r| r.min_width.unwrap_or(0)).max() else {
        return Vec::new();
    };
    applicable
        .into_iter()
        .filter(|r| r.min_width.unwrap_or(0) == widest)
        .collect()
}

/// **E12** — the enclosures a set of rules yields, deduplicated and ordered.
///
/// ⚠️ **Ordered and deduplicated on `(x, y)` ALONE.** The reference collects them into a
/// `std::set<Enclosure>` whose comparison reads only those two fields — so two enclosures with the
/// same extents but different swap behaviour are the *same element*, and one is silently discarded.
/// Keeping both changes how many candidates the enclosure choice sees, and so which pair it lands
/// on.
pub fn distinct_enclosures(encs: &[Enclosure]) -> Vec<Enclosure> {
    let mut out: Vec<Enclosure> = Vec::new();
    for e in encs {
        if !out.contains(e) {
            out.push(*e);
        }
    }
    out.sort_by_key(|e| (e.x, e.y));
    out
}

/// **E25** — whether a generate rule may be used for a shape of this width.
///
/// A `VIARULE GENERATE` layer entry may carry a `WIDTH min max` range, and the rule applies only
/// where the shape it lands on falls inside it. ⚠️ **A rule with no range applies at every width** —
/// absent is not zero, and treating it as `0..0` throws away every unrestricted rule there is.
///
/// 🔑 **A split array reports a width of ZERO, so every width-restricted rule fails here.** That is
/// not an accident of the arithmetic: `getRectSize` returns 0 for a split array precisely so that a
/// wide-metal rule cannot claim a via that is going to be placed as scattered single cuts. In ASAP7
/// it is what sends `M2 -> M3` to the technology's `VIA23` instead of the wide-power generate rule,
/// and with it the 742 minimum-area patches that only a two-via stack leaves behind.
pub fn rule_valid_for_width(range: Option<(i32, i32)>, width: i32) -> bool {
    match range {
        None => true,
        Some((min, max)) => min <= width && width <= max,
    }
}

/// **E13** — how many rows (or columns) a cut array has.
///
/// `array_count * core_size + end_size`: a number of full core groups plus whatever the end group
/// contributes. ⚠️ An array of zero groups is not empty — the end group still stands, which is what
/// makes a single-cut via fall out of the same arithmetic rather than needing a case of its own.
pub fn array_count(groups: i32, core_size: i32, end_size: i32) -> i32 {
    groups * core_size + end_size
}

/// The geometry of a generated cut array, as the database stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViaParams {
    pub cut: (i32, i32),
    /// ⚠️ Edge-to-edge, derived from the centre-to-centre pitch.
    pub cut_spacing: (i32, i32),
    pub bottom_enclosure: (i32, i32),
    pub top_enclosure: (i32, i32),
    pub rows: i32,
    pub columns: i32,
}

/// **E19** — the enclosure left over once the cuts are fitted.
///
/// 🔑 **A generate rule commonly declares no enclosure at all**, and the metal overhang is then
/// whatever remains of the shape after the cut array is centred in it: `(extent - span) / 2`.
///
/// ⚠️ **Only on the layer's constrained axis.** A horizontal layer's thickness is its y extent, so
/// its overhang is in y and its x overhang is zero — the metal already runs the length of the wire
/// there and has nothing to overhang. Putting the leftover on both axes doubles the via's footprint.
///
/// ⚠️ Halved with integer division, so an odd leftover loses a unit rather than favouring a side.
pub fn leftover_enclosure(extent: (i32, i32), span: (i32, i32), layer: Direction) -> Enclosure {
    let (dx, dy) = (
        (extent.0 - span.0).max(0) / 2,
        (extent.1 - span.1).max(0) / 2,
    );
    if layer == Direction::Horizontal {
        Enclosure { x: 0, y: dy }
    } else {
        Enclosure { x: dx, y: 0 }
    }
}

/// **E14** — cut spacing from cut pitch.
///
/// ⚠️ **Pitch is centre-to-centre; spacing is edge-to-edge.** The database stores the second and
/// the generator works in the first, so the cut's own size comes off. Storing the pitch as the
/// spacing spreads every cut array by one cut width.
pub fn cut_spacing(pitch: (i32, i32), cut: (i32, i32)) -> (i32, i32) {
    (pitch.0 - cut.0, pitch.1 - cut.1)
}

/// **E15** — the rectangle a generated via occupies, centred on the origin.
///
/// The cuts span `(n - 1) * pitch + size` in each axis — ⚠️ **one fewer pitch than there are cuts**,
/// because the pitch is between centres and the outer half-cuts stick out at each end.
///
/// ⚠️ **The LARGER of the two enclosures is added, not each layer's own.** The same rule as
/// `cuts_across`: the via must satisfy both layers, so the tighter one never governs the extent.
///
/// ⚠️ **The rect is built from integer halves and is therefore always even-sized.** An odd span
/// yields a rect one unit narrower than the nominal span, symmetric about the origin rather than
/// rounded outward. Computing it as `(0, 0, width, height)` and centring afterwards gives a
/// different rectangle for every odd dimension.
pub fn via_rect(
    rows: i32,
    columns: i32,
    pitch: (i32, i32),
    cut: (i32, i32),
    bottom_enclosure: (i32, i32),
    top_enclosure: (i32, i32),
    include_enclosure: bool,
) -> crate::Rect {
    let height = (rows - 1) * pitch.1 + cut.1;
    let width = (columns - 1) * pitch.0 + cut.0;
    let (x_enc, y_enc) = if include_enclosure {
        (
            bottom_enclosure.0.max(top_enclosure.0),
            bottom_enclosure.1.max(top_enclosure.1),
        )
    } else {
        (0, 0)
    };
    let (half_w, half_h) = (width / 2, height / 2);
    (
        -half_w - x_enc,
        -half_h - y_enc,
        half_w + x_enc,
        half_h + y_enc,
    )
}

/// **E16** — the parameters a generated via is stored with.
pub fn via_params(
    rows: i32,
    columns: i32,
    pitch: (i32, i32),
    cut: (i32, i32),
    bottom_enclosure: (i32, i32),
    top_enclosure: (i32, i32),
) -> ViaParams {
    ViaParams {
        cut,
        cut_spacing: cut_spacing(pitch, cut),
        bottom_enclosure,
        top_enclosure,
        rows,
        columns,
    }
}

/// **E17** — the DRCFILL a stack leaves on a layer it merely passes through.
///
/// 🔑 **`Via::writeToDb` works on a polygon SET, and the order of its five steps is the rule:**
///
/// ⚠️ **The per-polygon extents are added in BOTH branches**, not as the alternative to the
/// whole-set bbox. And the leftover filter runs at the very end, after the minimum area — so a
/// patch grown to meet its minimum is judged on what it covers AFTER growing.
///
/// 🔑 **The metal is a LIST, and that is the whole of why an array patches at all.** Taking the
/// two sides' bounding boxes first makes the leftover empty by construction: the bbox of a set is
/// covered by the set's own bbox. An array's four groups leave real gaps between them, the XOR is
/// those gaps, and the patch survives. ⚠️ Passing unions instead costs an ASAP7 design with
/// ARRAYSPACING all four of its patches while every via and every placement already matches the
/// reference exactly.
pub fn intermediate_patches(
    previous_top: &[crate::Rect],
    current_bottom: &[crate::Rect],
    requires_patch: bool,
    min_area: i64,
    direction: Direction,
    manufacturing_grid: i32,
) -> Vec<crate::Rect> {
    let combine: Vec<crate::Rect> = previous_top
        .iter()
        .chain(current_bottom)
        .copied()
        .collect();
    if combine.is_empty() {
        return Vec::new();
    }
    let bbox = |rs: &[crate::Rect]| {
        rs.iter().copied().reduce(|a, b| {
            (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
        })
    };
    let mut pieces: Vec<crate::Rect> = if requires_patch {
        vec![bbox(&combine).unwrap()]
    } else {
        combine.clone()
    };
    // `get_polygons` then `extents` on each: the bounding box of every CONNECTED group of metal.
    for group in connected_groups(&combine) {
        if let Some(b) = bbox(&group) {
            pieces.push(b);
        }
    }
    // 🔑 **`patch_shapes` is a polygon SET, and `+=` is a union.** Where the whole-set bounding box
    // went in first, every per-group extent lies inside it and is absorbed — the set stays one
    // rectangle and `get_rectangles` yields one patch.
    //
    // ⚠️ Kept as a list instead, an array writes its own bounding box AND one box per group:
    // an ASAP7 design with ARRAYSPACING produced twelve patches where the reference writes four,
    // with the reference's own four among them. Right shapes, wrong count, and no rule to blame —
    // only the container.
    let mut out = union_rectangles(&pieces);
    // ⚠️ **The minimum area is applied to the decomposed set and the result unioned again**, before
    // any filtering — a grown patch may swallow its neighbour, and is judged after it does.
    out = union_rectangles(
        &out.into_iter()
            .map(|p| crate::trim::adjust_to_min_area(p, min_area, direction, manufacturing_grid))
            .collect::<Vec<_>>(),
    );
    // `interact(patch_shapes ^ combine_layer)`: keep a patch that touches anything the metal does
    // not already cover. The leftover is inside the patch, so a non-empty leftover and an
    // interaction are the same question. ⚠️ **Last**, after the union and the minimum area.
    out.retain(|p| area(*p) > covered_area(&combine, *p));
    out
}

/// The union of a set of rectangles, sliced back into non-overlapping rectangles.
///
/// 🔑 **This is what makes `patch_shapes += …` a union rather than a list.** A candidate contained
/// in another vanishes into it; two that overlap come back as a decomposition of their union.
///
/// ⚠️ Sliced into maximal VERTICAL slabs, which is what boost's `get_rectangles` gives for a
/// rectilinear set — a different slicing of the same area writes different DEF rows.
fn union_rectangles(rects: &[crate::Rect]) -> Vec<crate::Rect> {
    let live: Vec<crate::Rect> = rects.iter().copied().filter(|r| r.0 < r.2 && r.1 < r.3).collect();
    if live.is_empty() {
        return Vec::new();
    }
    let mut xs: Vec<i32> = live.iter().flat_map(|r| [r.0, r.2]).collect();
    let mut ys: Vec<i32> = live.iter().flat_map(|r| [r.1, r.3]).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    // Per x-band, the maximal runs of covered y-cells.
    let bands: Vec<Vec<(i32, i32)>> = (0..xs.len() - 1)
        .map(|xi| {
            let mut runs: Vec<(i32, i32)> = Vec::new();
            for yi in 0..ys.len() - 1 {
                let covered = live.iter().any(|r| {
                    r.0 <= xs[xi] && r.2 >= xs[xi + 1] && r.1 <= ys[yi] && r.3 >= ys[yi + 1]
                });
                if !covered {
                    continue;
                }
                match runs.last_mut() {
                    Some(last) if last.1 == ys[yi] => last.1 = ys[yi + 1],
                    _ => runs.push((ys[yi], ys[yi + 1])),
                }
            }
            runs
        })
        .collect();
    // Adjacent x-bands with the same runs are one slab.
    let mut out: Vec<crate::Rect> = Vec::new();
    let mut xi = 0;
    while xi < bands.len() {
        if bands[xi].is_empty() {
            xi += 1;
            continue;
        }
        let mut xj = xi + 1;
        while xj < bands.len() && bands[xj] == bands[xi] {
            xj += 1;
        }
        for &(y0, y1) in &bands[xi] {
            out.push((xs[xi], y0, xs[xj], y1));
        }
        xi = xj;
    }
    out
}

fn area(r: crate::Rect) -> i64 {
    ((r.2 - r.0).max(0) as i64) * ((r.3 - r.1).max(0) as i64)
}

/// The rects grouped as boost's `get_polygons` would: metals that touch are one polygon.
///
/// ⚠️ **Closed, as odb has it** — a shared edge joins two rects rather than leaving a hairline gap.
fn connected_groups(rects: &[crate::Rect]) -> Vec<Vec<crate::Rect>> {
    let touching = |a: crate::Rect, b: crate::Rect| {
        a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
    };
    let mut owner: Vec<usize> = (0..rects.len()).collect();
    fn find(owner: &mut Vec<usize>, i: usize) -> usize {
        if owner[i] != i {
            let r = find(owner, owner[i]);
            owner[i] = r;
        }
        owner[i]
    }
    for i in 0..rects.len() {
        for j in (i + 1)..rects.len() {
            if touching(rects[i], rects[j]) {
                let (a, b) = (find(&mut owner, i), find(&mut owner, j));
                if a != b {
                    owner[a] = b;
                }
            }
        }
    }
    let mut groups: Vec<(usize, Vec<crate::Rect>)> = Vec::new();
    for i in 0..rects.len() {
        let root = find(&mut owner, i);
        match groups.iter_mut().find(|(r, _)| *r == root) {
            Some((_, v)) => v.push(rects[i]),
            None => groups.push((root, vec![rects[i]])),
        }
    }
    groups.into_iter().map(|(_, v)| v).collect()
}

/// How much of `clip` the union of `rects` covers.
///
/// ⚠️ **The UNION, so overlapping metal is not counted twice.** Summing the intersections instead
/// makes a patch look already covered wherever two vias overlap, and drops it.
fn covered_area(rects: &[crate::Rect], clip: crate::Rect) -> i64 {
    let clipped: Vec<crate::Rect> = rects
        .iter()
        .map(|r| {
            (
                r.0.max(clip.0),
                r.1.max(clip.1),
                r.2.min(clip.2),
                r.3.min(clip.3),
            )
        })
        .filter(|r| r.0 < r.2 && r.1 < r.3)
        .collect();
    if clipped.is_empty() {
        return 0;
    }
    // Coordinate compression: exact for axis-aligned rects, and no polygon library.
    let mut xs: Vec<i32> = clipped.iter().flat_map(|r| [r.0, r.2]).collect();
    let mut ys: Vec<i32> = clipped.iter().flat_map(|r| [r.1, r.3]).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let mut total: i64 = 0;
    for xi in 0..xs.len().saturating_sub(1) {
        for yi in 0..ys.len().saturating_sub(1) {
            let cell = (xs[xi], ys[yi], xs[xi + 1], ys[yi + 1]);
            if clipped
                .iter()
                .any(|r| r.0 <= cell.0 && r.1 <= cell.1 && r.2 >= cell.2 && r.3 >= cell.3)
            {
                total += area(cell);
            }
        }
    }
    total
}


/// A LEF58 `ARRAYSPACING` rule, as the cut layer states it.
///
/// Once a via holds enough cuts in a row the technology may require them GROUPED: `cuts` per group
/// at the ordinary pitch, with `spacing` between groups. One rule may state several such pairs and
/// the widest group that fits is not the one chosen — see [`array_fit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayRule {
    /// ⚠️ **Empty means EVERY cut class, not none.** `ViaGenerator::isCutClass` returns true
    /// whenever either side is null, so a rule naming no class applies to every via.
    pub cut_class: String,
    pub parallel_overlap: bool,
    /// ⚠️ A long array leaves the ALONG axis alone: only the across axis is clamped to `cuts`.
    pub long_array: bool,
    /// ⚠️ Zero means the rule states no width limit, not a limit of nothing.
    pub array_width: i32,
    /// ⚠️ Zero means the rule states none, and the via keeps the cut pitch it already had.
    pub cut_spacing: i32,
    /// `(cuts, spacing)` — the `ARRAYCUTS n SPACING s` lines, ascending by `cuts`.
    pub cuts_spacing: Vec<(i32, i32)>,
}

/// What an `ARRAYSPACING` rule makes of a via: groups of cuts, and the gap between groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayFit {
    /// How many groups across and up.
    pub groups: (i32, i32),
    /// The cuts in one full group.
    pub core: (i32, i32),
    /// The cuts in the leftover group, `0` where the array divides evenly.
    pub end: (i32, i32),
    /// Centre-to-centre within a group.
    pub cut_pitch: (i32, i32),
    /// Edge-to-edge between groups.
    pub array_spacing: (i32, i32),
    /// What the shape has left over once the cuts are laid out — the caller halves it and hands it
    /// to the same enclosure chooser every other path uses.
    pub double_enclosure: (i32, i32),
}

/// **E20** — `determineRowsAndColumns`'s array branch: which `ARRAYSPACING` rule a via takes, if any.
///
/// 🔑 **The rule chosen is the one yielding the greatest total CUT AREA**, not the widest group and
/// not the first that fits.
///
/// ⚠️ **Strictly greater**, so among equal areas the first rule met wins — the map is ascending by
/// cut count, so that is the smallest group.
///
/// ⚠️ **The gate is on the PLAIN fit, before any rule is considered**: `max(rows, cols) >= 2`. A via
/// that holds a single cut either way never becomes an array however the rules read.
///
/// ⚠️ **`rule_cuts` is tested against the SMALLER dimension**, plus one where the rule allows a long
/// array — `rule_cuts > array_size_min + (isLongArray() ? 1 : 0)` skips the rule.
#[allow(clippy::too_many_arguments)]
pub fn array_fit(
    rules: &[ArrayRule],
    // The via's own cut class; `None` where it has none, which matches every rule.
    via_cut_class: Option<&str>,
    // The shape the cuts must fit inside.
    extent: (i32, i32),
    cut: (i32, i32),
    // The pitch the via would use without any rule, centre to centre.
    pitch: (i32, i32),
    bottom_min_enc: (i32, i32),
    top_min_enc: (i32, i32),
    max_cuts: (i32, i32),
    // The plain fit, which decides whether the rules are consulted at all.
    plain: (i32, i32),
) -> Option<ArrayFit> {
    let (cols, rows) = plain;
    if cols.max(rows) < 2 {
        return None;
    }
    // ⚠️ The area the ARRAY is laid into takes the LARGER of the two minimum enclosures per axis,
    // which is not the same as either one.
    let area = (
        extent.0 - 2 * bottom_min_enc.0.max(top_min_enc.0),
        extent.1 - 2 * bottom_min_enc.1.max(top_min_enc.1),
    );
    let size_min = cols.min(rows);
    let mut max_cut_area: i64 = 0;
    let mut best: Option<ArrayFit> = None;
    for rule in rules {
        if rule.parallel_overlap {
            continue;
        }
        // 🔑 A rule naming no class applies to every via, and a via with no class matches every
        // rule: `isCutClass` compares only when BOTH sides name one.
        if let (Some(mine), false) = (via_cut_class, rule.cut_class.is_empty()) {
            if mine != rule.cut_class {
                continue;
            }
        }
        if rule.array_width != 0 && rule.array_width > extent.0 {
            continue;
        }
        for &(rule_cuts, rule_spacing) in &rule.cuts_spacing {
            if rule_cuts > size_min + i32::from(rule.long_array) {
                continue;
            }
            let spacing = if rule.cut_spacing != 0 {
                (rule.cut_spacing, rule.cut_spacing)
            } else {
                (pitch.0 - cut.0, pitch.1 - cut.1)
            };
            let mut x_cuts = cuts_across(
                extent.0,
                cut.0,
                bottom_min_enc.0,
                top_min_enc.0,
                spacing.0 + cut.0,
                max_cuts.0,
            );
            if !rule.long_array {
                x_cuts = x_cuts.min(rule_cuts);
            }
            let y_cuts = cuts_across(
                extent.1,
                cut.1,
                bottom_min_enc.1,
                top_min_enc.1,
                spacing.1 + cut.1,
                max_cuts.1,
            )
            .min(rule_cuts);
            let group = (
                cuts_width(x_cuts, cut.0, spacing.0, 0),
                cuts_width(y_cuts, cut.1, spacing.1, 0),
            );
            let array_pitch = (group.0 + rule_spacing, group.1 + rule_spacing);
            if array_pitch.0 <= 0 || array_pitch.1 <= 0 {
                continue;
            }
            let full = (
                (area.0 - group.0) / array_pitch.0 + 1,
                (area.1 - group.1) / array_pitch.1 + 1,
            );
            // ⚠️ **The remainder is tested against zero, not against being positive.** Where the
            // groups overshoot the area it is negative, and the leftover group is then computed
            // from a negative width — which `cuts_across` answers with none.
            let remainder = (
                area.0 - full.0 * array_pitch.0,
                area.1 - full.1 * array_pitch.1,
            );
            let mut end = (0, 0);
            if remainder.0 != 0 {
                end.0 = cuts_across(remainder.0, cut.0, 0, 0, spacing.0 + cut.0, max_cuts.0);
            }
            if remainder.1 != 0 {
                end.1 = cuts_across(remainder.1, cut.1, 0, 0, spacing.1 + cut.1, max_cuts.1);
            }
            let total = i64::from(cut.0) * i64::from(cut.1)
                * i64::from(array_count(full.0, x_cuts, end.0))
                * i64::from(array_count(full.1, y_cuts, end.1));
            if max_cut_area >= total {
                continue;
            }
            max_cut_area = total;
            // ⚠️ **The leftover group costs an extra array spacing** — the gap before it — where
            // there is one, and nothing where there is not.
            let via_width = (
                full.0 * cuts_width(x_cuts, cut.0, spacing.0, 0)
                    + (full.0 - 1) * rule_spacing
                    + cuts_width(end.0, cut.0, spacing.0, 0)
                    + if end.0 > 0 { rule_spacing } else { 0 },
                full.1 * cuts_width(y_cuts, cut.1, spacing.1, 0)
                    + (full.1 - 1) * rule_spacing
                    + cuts_width(end.1, cut.1, spacing.1, 0)
                    + if end.1 > 0 { rule_spacing } else { 0 },
            );
            best = Some(ArrayFit {
                groups: full,
                core: (x_cuts, y_cuts),
                end,
                cut_pitch: (spacing.0 + cut.0, spacing.1 + cut.1),
                array_spacing: (rule_spacing, rule_spacing),
                double_enclosure: (extent.0 - via_width.0, extent.1 - via_width.1),
            });
        }
    }
    best
}


/// One base via of a cut array, and where its centre sits relative to the array's own centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayPlacement {
    /// The cut counts of the base via to place — `(columns, rows)`. Vias sharing a count share a
    /// definition, which is what makes an array a handful of definitions and many placements.
    pub cuts: (i32, i32),
    /// Offset from the array centre.
    pub at: (i32, i32),
}

/// **E21** — `DbArrayVia`: the base vias of a cut array and where each one goes.
///
/// 🔑 **An array is not one via.** It is up to four base vias — the core group, a short row, a
/// short column, and the corner where both are short — placed on a grid.
///
/// ⚠️ **The SHORT groups are placed first, at row 0 and column 0** — the remainder leads the array
/// rather than trailing it.
///
/// ⚠️ **The step is measured between the two vias' own extents**, so a step out of a short group is
/// shorter than a step out of a full one:
/// `array_x += (core_via_rect.dx() + last_via_rect.dx()) / 2 + array_spacing_x`. And the row step
/// uses `last_via_rect` from the END of the row just placed, which for a row containing a short
/// column is the LAST via of that row and not its first.
///
/// ⚠️ **A single group each way is not an array at all.** `isCutArray()` is
/// `array_core_x_ != 1 || array_core_y_ != 1`, so one group by one group is an ordinary base via
/// built at the rule's pitch and counts — this returns exactly that one placement, at the centre.
pub fn array_placements(fit: &ArrayFit, cut: (i32, i32)) -> Vec<ArrayPlacement> {
    // `getViaRect(false, true)` — the CUT extent of a group, enclosure excluded.
    let extent = |c: (i32, i32)| {
        (
            (c.0 - 1) * fit.cut_pitch.0 + cut.0,
            (c.1 - 1) * fit.cut_pitch.1 + cut.1,
        )
    };
    let has_end_col = fit.end.0 != 0;
    let has_end_row = fit.end.1 != 0;
    let columns = fit.groups.0 + i32::from(has_end_col);
    let rows = fit.groups.1 + i32::from(has_end_row);
    let core_rect = extent(fit.core);

    let mut total_w = (columns - 1) * (fit.array_spacing.0 + core_rect.0);
    let x_offset = if has_end_col {
        let end = extent((fit.end.0, fit.core.1)).0;
        total_w += end;
        end / 2
    } else {
        total_w += core_rect.0;
        core_rect.0 / 2
    };
    let mut total_h = (rows - 1) * (fit.array_spacing.1 + core_rect.1);
    let y_offset = if has_end_row {
        let end = extent((fit.core.0, fit.end.1)).1;
        total_h += end;
        end / 2
    } else {
        total_h += core_rect.1;
        core_rect.1 / 2
    };

    // The corner takes the short count on whichever axis has one.
    let corner = (
        if has_end_col { fit.end.0 } else { fit.core.0 },
        if has_end_row { fit.end.1 } else { fit.core.1 },
    );
    let mut out = Vec::new();
    let mut at_y = -total_h / 2 + y_offset;
    for row in 0..rows {
        let mut at_x = -total_w / 2 + x_offset;
        let mut last = core_rect;
        for col in 0..columns {
            let cuts = match (row, col) {
                (0, 0) if has_end_row || has_end_col => corner,
                (0, _) if has_end_row => (fit.core.0, fit.end.1),
                (_, 0) if has_end_col => (fit.end.0, fit.core.1),
                _ => fit.core,
            };
            out.push(ArrayPlacement {
                cuts,
                at: (at_x, at_y),
            });
            last = extent(cuts);
            at_x += (core_rect.0 + last.0) / 2 + fit.array_spacing.0;
        }
        at_y += (core_rect.1 + last.1) / 2 + fit.array_spacing.1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real numbers from `asap7_M1_M2_followpin_enclosure`: an M1 follow pin 54 tall crossing
    /// an M2 follow pin 18 tall, over an 18-tall cut. Upstream rule: V1 asks 9 of enclosure below,
    /// and the intersection gives 0 — the 18 either side is what makes the via legal.
    #[test]
    fn the_spare_is_the_metal_OUTSIDE_the_intersection() {
        let m1 = (1080, 1053, 15120, 1107); // 54 tall
        let m2 = (1080, 1071, 15120, 1089); // 18 tall, the whole intersection
        let (bottom, top) = spare_enclosure(m1, m2);
        assert_eq!(bottom.y, 18, "M1 reaches 18 past M2 on each side");
        assert_eq!(top.y, 0, "M2 reaches past nothing");
        assert_eq!((bottom.x, top.x), (0, 0), "they share their x extent exactly");
    }

    #[test]
    fn the_spare_takes_the_SMALLER_side_because_an_enclosure_is_symmetric() {
        // Reaching 40 past on one side and 5 on the other gives 5 of usable enclosure, not 40
        // and not 45: the cut sits between them and the tight side governs.
        let lower = (0, 60, 100, 110); // 40 below the upper, 5 above it
        let upper = (0, 100, 100, 105);
        let (bottom, _) = spare_enclosure(lower, upper);
        assert_eq!(bottom.y, 5);
    }

    #[test]
    fn a_shape_that_reaches_past_on_ONE_side_only_has_no_spare() {
        // ⚠️ Flush on one side means no room, whatever the other side does.
        let lower = (0, 0, 100, 200);
        let upper = (0, 0, 100, 100);
        let (bottom, _) = spare_enclosure(lower, upper);
        assert_eq!(bottom.y, 0, "flush at the low edge");
    }

    #[test]
    fn the_two_sides_are_opposites_so_at_most_one_can_have_spare() {
        let (b, t) = spare_enclosure((0, 0, 100, 100), (0, 30, 100, 70));
        assert_eq!((b.y, t.y), (30, 0));
        let (b, t) = spare_enclosure((0, 30, 100, 70), (0, 0, 100, 100));
        assert_eq!((b.y, t.y), (0, 30), "swapping the roles swaps the spare");
    }

    #[test]
    fn the_applied_enclosure_is_capped_at_what_the_RULE_asks() {
        // 🔑 Not at what the shape has: plenty of room still reports exactly the requirement.
        let built = Enclosure { x: 0, y: 0 };
        let required = Enclosure { x: 0, y: 9 };
        let spare = Enclosure { x: 0, y: 18 };
        assert_eq!(spare_applied(built, required, spare), Enclosure { x: 0, y: 9 });
    }

    #[test]
    fn too_little_spare_does_not_reach_the_requirement() {
        // ⚠️ It must still FAIL afterwards — the spare is not a way to pass a rule that is unmet.
        let got = spare_applied(
            Enclosure { x: 0, y: 0 },
            Enclosure { x: 0, y: 9 },
            Enclosure { x: 0, y: 4 },
        );
        assert_eq!(got.y, 4, "raised to what exists, still short of 9");
    }

    fn gen(name: &str, cut_area: i32, bottom: (i32, i32), top: (i32, i32)) -> Generator {
        Generator {
            name: name.into(),
            cut_area,
            bottom,
            top,
            bottom_direction: Direction::Horizontal,
            top_direction: Direction::Vertical,
        }
    }

    // ── enclosures ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_smaller_enclosure_is_preferred() {
        let tight = Enclosure { x: 10, y: 10 };
        let loose = Enclosure { x: 20, y: 20 };
        assert!(tight.is_preferred_over(Some(&loose), Direction::Vertical));
        assert!(!loose.is_preferred_over(Some(&tight), Direction::Vertical));
    }

    #[test]
    fn anything_beats_nothing() {
        assert!(Enclosure { x: 99, y: 99 }.is_preferred_over(None, Direction::Vertical));
    }

    #[test]
    fn a_vertical_layer_minimises_x_first_and_a_horizontal_one_minimises_y() {
        let a = Enclosure { x: 10, y: 20 };
        let b = Enclosure { x: 20, y: 10 };
        assert!(
            a.is_preferred_over(Some(&b), Direction::Vertical),
            "smaller x wins"
        );
        assert!(
            b.is_preferred_over(Some(&a), Direction::Horizontal),
            "smaller y wins"
        );
    }

    #[test]
    fn an_undirected_layer_behaves_as_a_vertical_one() {
        // ⚠️ The test is `!= Horizontal`, so `None` follows the vertical path rather than being a
        // case of its own.
        let a = Enclosure { x: 10, y: 20 };
        let b = Enclosure { x: 20, y: 10 };
        assert!(a.is_preferred_over(Some(&b), Direction::None));
    }

    #[test]
    fn the_other_axis_is_the_tie_break() {
        let a = Enclosure { x: 10, y: 10 };
        let b = Enclosure { x: 10, y: 20 };
        assert!(a.is_preferred_over(Some(&b), Direction::Vertical));
    }

    #[test]
    fn two_identical_enclosures_do_not_prefer_each_other() {
        let a = Enclosure { x: 10, y: 10 };
        assert!(
            !a.is_preferred_over(Some(&a.clone()), Direction::Vertical),
            "incumbent stays"
        );
    }

    // ── generators ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn more_cut_area_wins_before_anything_else() {
        // ⚠️ Opposite sense to the enclosures: here LARGER is preferred.
        let big = gen("big", 100, (1, 1), (1, 1));
        let small = gen("small", 50, (999, 999), (999, 999));
        assert!(big.is_preferred_over(Some(&small)));
        assert!(!small.is_preferred_over(Some(&big)));
    }

    #[test]
    fn a_horizontal_layer_is_compared_on_its_height() {
        // ⚠️ Reads backwards and is correct: a horizontal wire's height is its conducting width.
        let tall = Generator {
            bottom: (10, 50),
            ..gen("tall", 100, (10, 50), (10, 10))
        };
        let wide = Generator {
            bottom: (50, 10),
            ..gen("wide", 100, (50, 10), (10, 10))
        };
        assert!(
            tall.is_preferred_over(Some(&wide)),
            "bottom is horizontal, so height decides"
        );
    }

    #[test]
    fn a_vertical_layer_is_compared_on_its_width() {
        let a = Generator {
            top: (50, 10),
            ..gen("a", 100, (10, 10), (50, 10))
        };
        let b = Generator {
            top: (10, 50),
            ..gen("b", 100, (10, 10), (10, 50))
        };
        assert!(
            a.is_preferred_over(Some(&b)),
            "top is vertical, so width decides"
        );
    }

    #[test]
    fn the_bottom_layer_outranks_the_top() {
        let a = gen("a", 100, (10, 50), (10, 10));
        let b = gen("b", 100, (10, 10), (99, 99));
        assert!(
            a.is_preferred_over(Some(&b)),
            "a wins on the bottom despite losing on the top"
        );
    }

    #[test]
    fn the_non_preferred_dimensions_are_the_last_resort() {
        // Equal cut area and equal preferred dimensions on both layers; only the other axis differs.
        let a = gen("a", 100, (50, 10), (10, 10));
        let b = gen("b", 100, (20, 10), (10, 10));
        assert!(a.is_preferred_over(Some(&b)));
    }

    #[test]
    fn two_identical_generators_do_not_prefer_each_other() {
        let a = gen("a", 100, (10, 10), (10, 10));
        assert!(!a.is_preferred_over(Some(&a.clone())));
    }

    #[test]
    fn among_equals_the_one_built_first_wins() {
        // 🔑 The stability of the sort is load-bearing, not incidental: `is_preferred_over` returns
        // false for a tie, so nothing else decides the order.
        let a = gen("first", 100, (10, 10), (10, 10));
        let b = gen("second", 100, (10, 10), (10, 10));
        assert_eq!(best(&[a.clone(), b.clone()]).unwrap().name, "first");
        assert_eq!(best(&[b, a]).unwrap().name, "second");
    }

    #[test]
    fn the_best_generator_is_chosen_from_anywhere_in_the_list() {
        let a = gen("a", 50, (10, 10), (10, 10));
        let b = gen("b", 300, (10, 10), (10, 10));
        let c = gen("c", 100, (10, 10), (10, 10));
        assert_eq!(best(&[a, b, c]).unwrap().name, "b");
    }

    #[test]
    fn nothing_to_choose_from_is_not_an_error() {
        assert!(best(&[]).is_none());
    }

    /// Build a pair whose diffs (`other - this`) are exactly the ones the reference printed.
    fn pair(
        bw: i32,
        bh: i32,
        b_hor: bool,
        tw: i32,
        th: i32,
        t_hor: bool,
    ) -> (Generator, Generator) {
        let dir = |h: bool| {
            if h {
                Direction::Horizontal
            } else {
                Direction::Vertical
            }
        };
        let mine = Generator {
            name: "this".into(),
            cut_area: 100,
            bottom: (0, 0),
            top: (0, 0),
            bottom_direction: dir(b_hor),
            top_direction: dir(t_hor),
        };
        let other = Generator {
            name: "other".into(),
            cut_area: 100,
            bottom: (bw, bh),
            top: (tw, th),
            bottom_direction: dir(b_hor),
            top_direction: dir(t_hor),
        };
        (mine, other)
    }

    #[test]
    fn the_comparator_agrees_with_the_references_own_numbers() {
        // 🔑 These six input sets are **the reference's**, taken from its `ViaPreference` debug
        // output on a real design (177 dimension-level comparisons in one case). The verdicts
        // follow from `preferred = is_hor ? height : width` and a diff of `other - this`.
        let cases = [
            //  bottom (w, h, hor)      top (w, h, hor)        this preferred?
            ((0, 0, false), (0, 0, true), false), // a complete tie keeps the incumbent
            ((140, 0, false), (0, 140, true), false), // other's bottom is wider on a vertical layer
            ((0, 0, true), (0, 0, false), false),
            ((0, 0, true), (140, 0, false), false), // decided by the TOP, bottom being level
            ((-140, 0, false), (0, -140, true), true), // this one is wider -> this wins
            ((0, 140, true), (0, 0, false), false), // horizontal bottom judged on HEIGHT
            ((0, -140, true), (0, 0, false), true),
        ];
        for (b, t, want) in cases {
            let (mine, other) = pair(b.0, b.1, b.2, t.0, t.1, t.2);
            assert_eq!(
                mine.is_preferred_over(Some(&other)),
                want,
                "bottom diff {b:?} top diff {t:?}"
            );
        }
    }

    // ── the enclosure pair, over the cross product ───────────────────────────────────────────

    fn enc(x: i32, y: i32) -> Enclosure {
        Enclosure { x, y }
    }

    #[test]
    fn the_cut_count_is_recomputed_for_every_pair() {
        // 🔑 A tighter enclosure leaves room for more cuts. Choosing the enclosure against a fixed
        // count would take the first pair instead of the one that actually fits most.
        let bottoms = [enc(50, 50), enc(10, 10)];
        let tops = [enc(10, 10)];
        let got = best_enclosure_pair(
            &bottoms,
            &tops,
            Direction::Vertical,
            Direction::Vertical,
            &|b, _t| (100 - b.x) / 10,
            &|_, _, _| true,
        )
        .unwrap();
        assert_eq!(got.bottom, enc(10, 10), "the tighter one fits more cuts");
        assert_eq!(got.cuts, 9);
    }

    #[test]
    fn a_pair_failing_the_constraints_is_not_considered() {
        let got = best_enclosure_pair(
            &[enc(1, 1), enc(5, 5)],
            &[enc(1, 1)],
            Direction::Vertical,
            Direction::Vertical,
            &|b, _| if b.x == 1 { 9 } else { 2 },
            &|b, _, _| b.x != 1, // the 9-cut pair is rejected
        )
        .unwrap();
        assert_eq!(got.cuts, 2, "the surviving pair wins despite fewer cuts");
    }

    #[test]
    fn no_pair_passing_means_the_via_is_not_buildable() {
        // ⚠️ `None`, which is the reference returning false from `build` and dropping the
        // candidate — not a fallback to some default enclosure.
        assert!(best_enclosure_pair(
            &[enc(1, 1)],
            &[enc(1, 1)],
            Direction::Vertical,
            Direction::Vertical,
            &|_, _| 4,
            &|_, _, _| false,
        )
        .is_none());
    }

    #[test]
    fn on_a_tie_the_bottom_decides_and_carries_its_top_with_it() {
        // ⚠️ The pair is saved together: the winning bottom's top comes along even though the
        // other pair offered a tighter top.
        let got = best_enclosure_pair(
            &[enc(9, 9), enc(1, 1)],
            &[enc(5, 5)],
            Direction::Vertical,
            Direction::Vertical,
            &|_, _| 4,
            &|_, _, _| true,
        )
        .unwrap();
        assert_eq!(got.bottom, enc(1, 1));
        assert_eq!(got.top, enc(5, 5));
    }

    #[test]
    fn more_cuts_beats_a_tighter_enclosure() {
        let got = best_enclosure_pair(
            &[enc(1, 1), enc(9, 9)],
            &[enc(1, 1)],
            Direction::Vertical,
            Direction::Vertical,
            &|b, _| if b.x == 9 { 8 } else { 2 },
            &|_, _, _| true,
        )
        .unwrap();
        assert_eq!(
            got.bottom,
            enc(9, 9),
            "8 cuts beats a tighter enclosure with 2"
        );
    }

    #[test]
    fn the_whole_cross_product_is_walked() {
        let seen = std::cell::Cell::new(0);
        let _ = best_enclosure_pair(
            &[enc(1, 1), enc(2, 2), enc(3, 3)],
            &[enc(1, 1), enc(2, 2)],
            Direction::Vertical,
            Direction::Vertical,
            &|_, _| 1,
            &|_, _, _| {
                seen.set(seen.get() + 1);
                true
            },
        );
        assert_eq!(seen.get(), 6, "three bottoms against two tops");
    }

    // ── the enclosure actually built ─────────────────────────────────────────────────────────

    #[test]
    fn a_shape_need_not_fit_along_its_own_length() {
        // ⚠️ A horizontal shape runs the length of x, so the via only has to fit in y.
        assert_eq!(
            constraint_for(Direction::Horizontal, true, false),
            Constraint {
                must_fit_x: false,
                must_fit_y: true
            }
        );
        assert_eq!(
            constraint_for(Direction::Vertical, true, false),
            Constraint {
                must_fit_x: true,
                must_fit_y: false
            }
        );
    }

    #[test]
    fn an_unmodifiable_shape_must_fit_on_both_axes() {
        // Nothing can be widened to accommodate the via.
        for (m, i) in [(false, false), (true, true)] {
            assert_eq!(
                constraint_for(Direction::Horizontal, m, i),
                Constraint {
                    must_fit_x: true,
                    must_fit_y: true
                }
            );
        }
    }

    #[test]
    fn an_undirected_shape_must_fit_on_both_axes() {
        // ⚠️ Neither test excuses it: `!= Horizontal` and `!= Vertical` are both true.
        assert_eq!(
            constraint_for(Direction::None, true, false),
            Constraint {
                must_fit_x: true,
                must_fit_y: true
            }
        );
    }

    #[test]
    fn the_axis_that_must_fit_takes_the_overlap_and_the_other_takes_the_minimum() {
        // 🔑 The reference's own numbers for a horizontal metal5 against a 4000-wide ring:
        // minimum 0, overlap 60, must_fit_y -> ENCLOSURE x=0 y=60.
        let got = built_enclosure(
            false,
            Enclosure { x: 0, y: 0 },
            Enclosure { x: 60, y: 60 },
            Constraint {
                must_fit_x: false,
                must_fit_y: true,
            },
        );
        assert_eq!(got, Enclosure { x: 0, y: 60 });
    }

    #[test]
    fn an_internal_layer_takes_minimums_however_much_room_it_has() {
        // ⚠️ Which is why a via in the MIDDLE of a stack commonly carries no overhang while the
        // ends carry plenty — `via4_5` is `0 0 0 0` for exactly this reason.
        let got = built_enclosure(
            true,
            Enclosure { x: 0, y: 0 },
            Enclosure { x: 60, y: 60 },
            Constraint {
                must_fit_x: true,
                must_fit_y: true,
            },
        );
        assert_eq!(got, Enclosure { x: 0, y: 0 });
    }

    #[test]
    fn the_overlap_is_half_the_room_the_array_leaves() {
        assert_eq!(
            overlap_enclosure((4000, 4000), (3880, 3880)),
            Enclosure { x: 60, y: 60 }
        );
    }

    #[test]
    fn an_array_wider_than_its_shape_gives_a_negative_overlap() {
        // ⚠️ Not clamped. The reference carries the negative through rather than flooring it, and
        // flooring it here would hide a via that does not fit.
        assert_eq!(
            overlap_enclosure((100, 100), (500, 500)),
            Enclosure { x: -200, y: -200 }
        );
    }

    // ── cut arrays ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_larger_of_the_two_enclosures_is_used_on_both_sides() {
        // ⚠️ Not each layer's own. A via must satisfy both, so the tighter one never applies.
        // With enclosures 10 and 40 the usable width is 100 - 80 = 20, not 100 - 100 or 100 - 20.
        assert_eq!(cuts_across(100, 20, 10, 40, 0, 0), 1);
        assert_eq!(
            cuts_across(100, 21, 10, 40, 0, 0),
            0,
            "one unit too wide a cut and none fit"
        );
    }

    #[test]
    fn the_first_cut_is_free_and_the_rest_are_paid_for_at_the_pitch() {
        // width 100, no enclosure, cut 20 -> 80 left, pitch 20 -> 4 more, 5 total.
        assert_eq!(cuts_across(100, 20, 0, 0, 20, 0), 5);
    }

    #[test]
    fn a_zero_pitch_means_exactly_one_cut() {
        // ⚠️ Not a division by zero, and not zero cuts.
        assert_eq!(cuts_across(100, 20, 0, 0, 0, 0), 1);
    }

    #[test]
    fn a_wire_narrower_than_its_enclosures_takes_no_cuts() {
        assert_eq!(cuts_across(10, 5, 40, 40, 10, 0), 0);
    }

    #[test]
    fn a_maximum_of_zero_means_no_maximum() {
        assert_eq!(cuts_across(100, 20, 0, 0, 20, 0), 5);
        assert_eq!(
            cuts_across(100, 20, 0, 0, 20, 3),
            3,
            "and a real maximum caps it"
        );
        assert_eq!(
            cuts_across(100, 20, 0, 0, 20, 9),
            5,
            "a maximum above the fit changes nothing"
        );
    }

    #[test]
    fn a_run_of_cuts_has_one_fewer_gaps_than_cuts() {
        assert_eq!(cuts_width(3, 10, 5, 2), 30 + 10 + 4);
        assert_eq!(cuts_width(1, 10, 5, 2), 14, "one cut, no gap");
    }

    #[test]
    fn no_cuts_occupy_nothing_rather_than_two_enclosures() {
        assert_eq!(cuts_width(0, 10, 5, 2), 0);
    }

    // ── cut classes ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_cut_class_matches_either_orientation() {
        // ⚠️ The class describes a shape, not an orientation.
        let c = [CutClass {
            name: "VA".into(),
            width: 40,
            length: Some(80),
        }];
        assert_eq!(cut_class(&c, (80, 40)).unwrap().name, "VA");
        assert_eq!(cut_class(&c, (40, 80)).unwrap().name, "VA");
    }

    #[test]
    fn a_class_with_no_length_is_square() {
        let c = [CutClass {
            name: "VA".into(),
            width: 40,
            length: None,
        }];
        assert_eq!(cut_class(&c, (40, 40)).unwrap().name, "VA");
        assert!(cut_class(&c, (40, 80)).is_none());
    }

    #[test]
    fn the_first_matching_class_wins() {
        // ⚠️ The search stops at the first hit, so declaration order decides between two that
        // could both match.
        let c = [
            CutClass {
                name: "first".into(),
                width: 40,
                length: None,
            },
            CutClass {
                name: "second".into(),
                width: 40,
                length: Some(40),
            },
        ];
        assert_eq!(cut_class(&c, (40, 40)).unwrap().name, "first");
    }

    #[test]
    fn a_cut_matching_nothing_has_no_class() {
        let c = [CutClass {
            name: "VA".into(),
            width: 40,
            length: None,
        }];
        assert!(cut_class(&c, (99, 99)).is_none());
    }

    // ── minimum cuts ─────────────────────────────────────────────────────────────────────────

    fn rule(width: i32, cuts: i32) -> MinCutRule {
        MinCutRule {
            width,
            cuts,
            above: false,
            below: false,
            cut_class: None,
        }
    }

    #[test]
    fn no_applicable_rule_means_valid() {
        assert!(check_min_cuts(&[], None, 100, 1, true));
        assert!(
            check_min_cuts(&[rule(500, 4)], None, 100, 1, true),
            "rule is wider than the wire"
        );
    }

    #[test]
    fn a_rule_written_exactly_at_the_wires_width_does_not_apply() {
        // ⚠️ Strictly less than. A rule at 100 does not govern a 100-wide wire.
        assert!(check_min_cuts(&[rule(100, 4)], None, 100, 1, true));
        assert!(
            !check_min_cuts(&[rule(99, 4)], None, 100, 1, true),
            "but one at 99 does"
        );
    }

    #[test]
    fn only_the_widest_applicable_group_is_consulted() {
        // ⚠️ The narrow rule demands 2 cuts and the wide one demands 8; a via with 4 cuts is
        // judged only against the WIDE one and fails, even though it satisfies the narrow one.
        let rules = [rule(10, 2), rule(50, 8)];
        assert!(!check_min_cuts(&rules, None, 100, 4, true));
        assert!(check_min_cuts(&rules, None, 100, 8, true));
    }

    #[test]
    fn within_the_widest_group_any_rule_passing_is_enough() {
        // 🔑 An OR, not an AND. Requiring every alternative to pass rejects vias the technology
        // permits.
        let rules = [rule(50, 8), rule(50, 2)];
        assert!(
            check_min_cuts(&rules, None, 100, 4, true),
            "satisfies the second alternative"
        );
    }

    #[test]
    fn a_below_rule_governs_only_the_lower_layer() {
        let below = MinCutRule {
            width: 50,
            cuts: 8,
            above: false,
            below: true,
            cut_class: None,
        };
        assert!(
            !check_min_cuts(&[below.clone()], None, 100, 4, true),
            "applies below"
        );
        assert!(
            check_min_cuts(&[below], None, 100, 4, false),
            "and not above"
        );
    }

    #[test]
    fn a_rule_for_another_cut_class_is_ignored() {
        let r = MinCutRule {
            width: 50,
            cuts: 8,
            above: false,
            below: false,
            cut_class: Some("VA".into()),
        };
        assert!(
            check_min_cuts(&[r.clone()], Some("VB"), 100, 1, true),
            "different class"
        );
        assert!(
            !check_min_cuts(&[r], Some("VA"), 100, 1, true),
            "same class"
        );
    }

    #[test]
    fn the_min_cut_check_agrees_with_the_references_own_verdicts() {
        // 🔑 The reference's numbers, from its `MinCut` debug output on asap7:
        //   "Layer M9 (below false) of width 2.0000 has 1 min cut rules."
        //   "Rule width 1.8050 above (true) or below (false) requires 2 vias, has 36 vias: true."
        // Scaled to integers; the rule is scale-invariant.
        let r = MinCutRule {
            width: 18050,
            cuts: 2,
            above: true,
            below: false,
            cut_class: None,
        };
        assert!(
            check_min_cuts(&[r.clone()], None, 20000, 36, false),
            "36 vias against a need of 2"
        );
        assert!(
            check_min_cuts(&[r.clone()], None, 20000, 21, false),
            "the other observed count"
        );
        // ⚠️ The same rule on the layer BELOW does not apply, because it is an `above` rule.
        assert!(check_min_cuts(&[r.clone()], None, 20000, 1, true));
        // ⚠️ And it would reject a via with too few cuts, which the suite never exercises.
        assert!(!check_min_cuts(&[r], None, 20000, 1, false));
    }

    // ── enclosure rules and arrays ───────────────────────────────────────────────────────────

    fn enc_rule(name: &str, above: bool, below: bool, min_width: Option<i32>) -> EnclosureRule {
        EnclosureRule {
            name: name.into(),
            cut_class: None,
            above,
            below,
            min_width,
        }
    }

    #[test]
    fn a_square_rectangle_is_undirected_rather_than_defaulting() {
        // ⚠️ Three outcomes, not two. Collapsing this into horizontal or vertical picks the wrong
        // enclosure axis for every square via.
        assert_eq!(rect_direction((0, 0, 10, 20)), Direction::Vertical);
        assert_eq!(rect_direction((0, 0, 20, 10)), Direction::Horizontal);
        assert_eq!(rect_direction((0, 0, 10, 10)), Direction::None);
    }

    #[test]
    fn a_rule_declaring_neither_above_nor_below_governs_both() {
        // ⚠️ Not a two-way choice — and unqualified is the common case, so reading it as one drops
        // most of the rules.
        let r = [enc_rule("any", false, false, None)];
        assert_eq!(
            enclosure_rules(&r, None, 100, true).len(),
            1,
            "governs above"
        );
        assert_eq!(enclosure_rules(&r, None, 100, false).len(), 1, "and below");
    }

    #[test]
    fn an_above_rule_governs_only_the_upper_side() {
        let r = [enc_rule("top", true, false, None)];
        assert_eq!(enclosure_rules(&r, None, 100, true).len(), 1);
        assert!(enclosure_rules(&r, None, 100, false).is_empty());
    }

    #[test]
    fn an_enclosure_rule_at_exactly_the_wires_width_DOES_apply() {
        // 🔑 The contrast worth holding onto: enclosure selection is `<=`, minimum-cut selection is
        // strictly `<`. Two sibling rules in the same file that differ by an equals sign, and
        // making them agree — in either direction — changes the answer at the boundary width.
        let r = [enc_rule("at", false, false, Some(100))];
        assert_eq!(
            enclosure_rules(&r, None, 100, true).len(),
            1,
            "enclosure: applies at 100"
        );
        assert!(
            check_min_cuts(&[rule(100, 4)], None, 100, 1, true),
            "min-cut: does NOT apply"
        );
    }

    #[test]
    fn only_the_widest_qualifying_enclosure_group_is_returned() {
        let r = [
            enc_rule("narrow", false, false, Some(10)),
            enc_rule("wide", false, false, Some(50)),
        ];
        let got = enclosure_rules(&r, None, 100, true);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "wide");
    }

    #[test]
    fn every_rule_of_the_widest_group_is_returned() {
        let r = [
            enc_rule("a", false, false, Some(50)),
            enc_rule("b", false, false, Some(50)),
            enc_rule("narrow", false, false, Some(10)),
        ];
        assert_eq!(enclosure_rules(&r, None, 100, true).len(), 2);
    }

    #[test]
    fn a_rule_wider_than_the_wire_does_not_apply() {
        let r = [enc_rule("wide", false, false, Some(500))];
        assert!(enclosure_rules(&r, None, 100, true).is_empty());
    }

    #[test]
    fn a_rule_with_no_width_behaves_as_zero() {
        let r = [
            enc_rule("none", false, false, None),
            enc_rule("fifty", false, false, Some(50)),
        ];
        let got = enclosure_rules(&r, None, 100, true);
        assert_eq!(
            got[0].name, "fifty",
            "the widest still wins over the unqualified one"
        );
    }

    #[test]
    fn enclosures_are_deduplicated_on_extent_alone() {
        // ⚠️ The reference collects them into a set keyed on (x, y), so two rules yielding the same
        // extents contribute ONE candidate however they were derived.
        let e = [
            Enclosure { x: 10, y: 20 },
            Enclosure { x: 10, y: 20 },
            Enclosure { x: 5, y: 30 },
        ];
        assert_eq!(
            distinct_enclosures(&e),
            vec![Enclosure { x: 5, y: 30 }, Enclosure { x: 10, y: 20 }]
        );
    }

    #[test]
    fn distinct_enclosures_come_out_in_x_then_y_order() {
        let e = [
            Enclosure { x: 10, y: 5 },
            Enclosure { x: 10, y: 1 },
            Enclosure { x: 2, y: 99 },
        ];
        let got = distinct_enclosures(&e);
        assert_eq!(
            got.iter().map(|e| (e.x, e.y)).collect::<Vec<_>>(),
            vec![(2, 99), (10, 1), (10, 5)]
        );
    }

    #[test]
    fn an_array_of_no_core_groups_still_has_its_end_group() {
        // ⚠️ Which is how a single-cut via falls out of the same arithmetic instead of needing a
        // case of its own.
        assert_eq!(array_count(0, 5, 1), 1);
        assert_eq!(array_count(3, 5, 1), 16);
    }

    // ── cut geometry ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn cut_spacing_is_the_pitch_less_the_cut() {
        // ⚠️ Storing the pitch as the spacing spreads every array by one cut width.
        assert_eq!(cut_spacing((100, 80), (40, 30)), (60, 50));
    }

    #[test]
    fn a_single_cut_spans_just_the_cut() {
        assert_eq!(
            via_rect(1, 1, (100, 100), (40, 30), (0, 0), (0, 0), false),
            (-20, -15, 20, 15)
        );
    }

    #[test]
    fn an_array_spans_one_fewer_pitch_than_it_has_cuts() {
        // 3 columns at pitch 100 with a 40-wide cut spans 2*100 + 40 = 240, not 3*100.
        let r = via_rect(1, 3, (100, 100), (40, 30), (0, 0), (0, 0), false);
        assert_eq!(r.2 - r.0, 240);
    }

    #[test]
    fn the_larger_enclosure_is_added_on_both_sides() {
        // ⚠️ Not each layer's own: the via must satisfy both.
        let r = via_rect(1, 1, (100, 100), (40, 30), (5, 5), (12, 3), true);
        assert_eq!((r.0, r.2), (-20 - 12, 20 + 12), "x takes the larger, 12");
        assert_eq!((r.1, r.3), (-15 - 5, 15 + 5), "y takes the larger, 5");
    }

    #[test]
    fn an_odd_span_yields_an_even_rect_centred_on_the_origin() {
        // ⚠️ Built from integer halves, so a 41-wide span becomes 40 wide, symmetric — not 41
        // rounded outward, and not offset by half a unit.
        let r = via_rect(1, 1, (100, 100), (41, 41), (0, 0), (0, 0), false);
        assert_eq!(r, (-20, -20, 20, 20));
        assert_eq!(r.2 - r.0, 40, "one unit narrower than the nominal span");
    }

    #[test]
    fn excluding_the_enclosure_gives_the_cuts_alone() {
        let with = via_rect(2, 2, (100, 100), (40, 40), (9, 9), (9, 9), true);
        let without = via_rect(2, 2, (100, 100), (40, 40), (9, 9), (9, 9), false);
        assert_eq!(without.2 - without.0, with.2 - with.0 - 18);
    }

    #[test]
    fn the_stored_parameters_carry_spacing_not_pitch() {
        let p = via_params(2, 3, (100, 80), (40, 30), (5, 6), (7, 8));
        assert_eq!(p.cut_spacing, (60, 50));
        assert_eq!((p.rows, p.columns), (2, 3));
        assert_eq!(p.bottom_enclosure, (5, 6));
        assert_eq!(
            p.top_enclosure,
            (7, 8),
            "the two are stored separately, unlike the extent"
        );
    }

    // ── enclosures from rules ────────────────────────────────────────────────────────────────

    #[test]
    fn orienting_puts_the_smaller_overhang_on_the_constrained_axis() {
        let e = Enclosure { x: 60, y: 10 };
        assert_eq!(
            swap_for_layer(e, Direction::Vertical),
            Enclosure { x: 10, y: 60 }
        );
        assert_eq!(
            swap_for_layer(e, Direction::Horizontal),
            e,
            "already smaller in y"
        );
    }

    #[test]
    fn the_undirected_case_is_split_differently_by_the_two_direction_tests() {
        // 🔑 `swap_for_layer` groups None with HORIZONTAL; `is_preferred_over` groups it with
        // VERTICAL. Same class, opposite grouping, one character apart in the source. This test
        // exists to fail if a later tidy-up makes them agree.
        let e = Enclosure { x: 10, y: 60 };
        assert_eq!(
            swap_for_layer(e, Direction::None),
            swap_for_layer(e, Direction::Horizontal)
        );
        let a = Enclosure { x: 10, y: 20 };
        let b = Enclosure { x: 20, y: 10 };
        assert_eq!(
            a.is_preferred_over(Some(&b), Direction::None),
            a.is_preferred_over(Some(&b), Direction::Vertical)
        );
    }

    #[test]
    fn a_default_rule_is_oriented_for_its_layer() {
        let e = enclosure_from_rule(
            EncType::Default,
            60,
            10,
            (28, 28),
            Direction::Vertical,
            Direction::None,
        );
        assert_eq!(e, Enclosure { x: 10, y: 60 });
    }

    #[test]
    fn an_eol_rule_follows_the_rectangles_direction_not_the_layers() {
        let h = enclosure_from_rule(
            EncType::Eol,
            60,
            10,
            (28, 28),
            Direction::Vertical,
            Direction::Horizontal,
        );
        let v = enclosure_from_rule(
            EncType::Eol,
            60,
            10,
            (28, 28),
            Direction::Vertical,
            Direction::Vertical,
        );
        assert_eq!(h, Enclosure { x: 60, y: 10 });
        assert_eq!(
            v,
            Enclosure { x: 10, y: 60 },
            "reversed, and NOT re-oriented for the layer"
        );
    }

    #[test]
    fn an_undirected_eol_rule_takes_the_larger_overhang_on_both_axes() {
        // ⚠️ Not either one of them — the larger, twice.
        let e = enclosure_from_rule(
            EncType::Eol,
            60,
            10,
            (28, 28),
            Direction::Vertical,
            Direction::None,
        );
        assert_eq!(e, Enclosure { x: 60, y: 60 });
    }

    #[test]
    fn an_endside_rule_reads_the_cuts_own_shape() {
        let tall = enclosure_from_rule(
            EncType::EndSide,
            60,
            10,
            (10, 40),
            Direction::Vertical,
            Direction::Horizontal,
        );
        let wide = enclosure_from_rule(
            EncType::EndSide,
            60,
            10,
            (40, 10),
            Direction::Vertical,
            Direction::Horizontal,
        );
        assert_eq!(tall, Enclosure { x: 10, y: 60 });
        assert_eq!(wide, Enclosure { x: 60, y: 10 });
    }

    #[test]
    fn only_a_default_rule_is_re_oriented() {
        // ⚠️ The other three have already said which overhang belongs where; orienting them undoes
        // that. Here a vertical layer would have swapped these, and does not.
        for kind in [EncType::Eol, EncType::EndSide, EncType::HorzAndVert] {
            let e = enclosure_from_rule(
                kind,
                60,
                10,
                (40, 10),
                Direction::Vertical,
                Direction::Horizontal,
            );
            assert_eq!(e, Enclosure { x: 60, y: 10 }, "{kind:?} kept its own axes");
        }
    }

    #[test]
    fn the_leftover_enclosure_is_what_remains_after_the_cuts_are_fitted() {
        // 🔑 The reference's own numbers: 7 cuts at pitch 600 span 6*600 + 280 = 3880 inside a
        // 4000-wide ring, leaving (4000 - 3880) / 2 = 60.
        let e = leftover_enclosure((4000, 4000), (3880, 3880), Direction::Horizontal);
        assert_eq!(e, Enclosure { x: 0, y: 60 });
        let t = leftover_enclosure((4000, 4000), (3880, 3880), Direction::Vertical);
        assert_eq!(
            t,
            Enclosure { x: 60, y: 0 },
            "the other layer takes it on the other axis"
        );
    }

    #[test]
    fn a_cut_array_filling_its_shape_leaves_no_enclosure() {
        assert_eq!(
            leftover_enclosure((3880, 3880), (3880, 3880), Direction::Horizontal),
            Enclosure { x: 0, y: 0 }
        );
    }

    #[test]
    fn an_array_wider_than_its_shape_does_not_give_a_negative_enclosure() {
        assert_eq!(
            leftover_enclosure((100, 100), (500, 500), Direction::Vertical),
            Enclosure { x: 0, y: 0 }
        );
    }

    #[test]
    fn no_enclosure_rules_at_all_is_a_pass() {
        assert!(enclosure_satisfies(Enclosure { x: 0, y: 0 }, &[]));
    }

    #[test]
    fn a_zero_rule_still_refuses_a_negative_enclosure() {
        // 🔑 The whole point of the gate. `END 0.0 SIDE 0.0` is not vacuous: it demands a
        // non-negative enclosure, and a cut taller than the rect it sits in yields a negative one.
        let zero = [(Enclosure { x: 0, y: 0 }, false)];
        assert!(enclosure_satisfies(Enclosure { x: 19, y: 0 }, &zero));
        assert!(!enclosure_satisfies(Enclosure { x: 19, y: -81 }, &zero));
    }

    #[test]
    fn one_rule_passing_is_enough() {
        let rules = [
            (Enclosure { x: 50, y: 50 }, false),
            (Enclosure { x: 5, y: 5 }, false),
        ];
        assert!(enclosure_satisfies(Enclosure { x: 10, y: 10 }, &rules));
    }

    #[test]
    fn a_swappable_rule_may_be_met_the_other_way_round() {
        let swap = [(Enclosure { x: 40, y: 10 }, true)];
        let fixed = [(Enclosure { x: 40, y: 10 }, false)];
        // 10 by 40 meets 40 by 10 only if the rule allows the axes to trade.
        assert!(enclosure_satisfies(Enclosure { x: 10, y: 40 }, &swap));
        assert!(!enclosure_satisfies(Enclosure { x: 10, y: 40 }, &fixed));
    }

    #[test]
    fn an_odd_leftover_loses_a_unit_rather_than_favouring_a_side() {
        assert_eq!(
            leftover_enclosure((101, 101), (0, 0), Direction::Vertical),
            Enclosure { x: 50, y: 0 }
        );
    }

    // ── the constraint gate ──────────────────────────────────────────────────────────────────

    #[test]
    fn the_constraint_checks_report_the_first_reason_in_a_fixed_order() {
        // ⚠️ A candidate failing several reports the FIRST, which is what a debug trace shows and
        // what a comparison against one reads.
        assert_eq!(
            check_constraints(0, false, false, true, true, true),
            Err("generates no vias")
        );
        assert_eq!(
            check_constraints(4, false, false, true, true, true),
            Err("violates minimum cut rules")
        );
        assert_eq!(
            check_constraints(4, true, false, true, true, true),
            Err("violates minimum enclosure rules")
        );
        assert_eq!(check_constraints(4, true, true, true, true, true), Ok(()));
    }

    #[test]
    fn each_check_can_be_switched_off_on_its_own() {
        assert_eq!(
            check_constraints(0, false, false, false, false, false),
            Ok(())
        );
        assert_eq!(
            check_constraints(0, true, true, false, true, true),
            Ok(()),
            "cuts unchecked"
        );
    }

    // ── enclosure pairs ──────────────────────────────────────────────────────────────────────

    #[test]
    fn more_cuts_always_wins() {
        let few = Candidate {
            cuts: 1,
            bottom: Enclosure { x: 1, y: 1 },
            top: Enclosure { x: 1, y: 1 },
        };
        let many = Candidate {
            cuts: 4,
            bottom: Enclosure { x: 9, y: 9 },
            top: Enclosure { x: 9, y: 9 },
        };
        assert_eq!(best_enclosures(&[few, many]).unwrap().cuts, 4);
        assert_eq!(best_enclosures(&[many, few]).unwrap().cuts, 4);
    }

    #[test]
    fn on_equal_cuts_the_tighter_bottom_wins() {
        let a = Candidate {
            cuts: 2,
            bottom: Enclosure { x: 9, y: 9 },
            top: Enclosure { x: 1, y: 1 },
        };
        let b = Candidate {
            cuts: 2,
            bottom: Enclosure { x: 1, y: 1 },
            top: Enclosure { x: 9, y: 9 },
        };
        assert_eq!(
            best_enclosures(&[a, b]).unwrap().bottom,
            Enclosure { x: 1, y: 1 }
        );
    }

    #[test]
    fn the_pair_is_taken_together_even_when_its_top_is_worse() {
        // 🔑 The rule that separates this from choosing each axis independently. The winning
        // candidate has the tighter bottom and the LOOSER top, and both are taken.
        let a = Candidate {
            cuts: 2,
            bottom: Enclosure { x: 9, y: 9 },
            top: Enclosure { x: 1, y: 1 },
        };
        let b = Candidate {
            cuts: 2,
            bottom: Enclosure { x: 1, y: 1 },
            top: Enclosure { x: 9, y: 9 },
        };
        let got = best_enclosures(&[a, b]).unwrap();
        assert_eq!(
            got.top,
            Enclosure { x: 9, y: 9 },
            "not the best top available"
        );
    }

    #[test]
    fn the_top_is_consulted_only_when_the_bottom_does_not_decide() {
        let a = Candidate {
            cuts: 2,
            bottom: Enclosure { x: 5, y: 5 },
            top: Enclosure { x: 9, y: 9 },
        };
        let b = Candidate {
            cuts: 2,
            bottom: Enclosure { x: 5, y: 5 },
            top: Enclosure { x: 1, y: 1 },
        };
        assert_eq!(
            best_enclosures(&[a, b]).unwrap().top,
            Enclosure { x: 1, y: 1 }
        );
    }

    #[test]
    fn the_first_candidate_is_always_taken() {
        let only = Candidate {
            cuts: 1,
            bottom: Enclosure { x: 9, y: 9 },
            top: Enclosure { x: 9, y: 9 },
        };
        assert_eq!(best_enclosures(&[only]), Some(only));
    }

    #[test]
    fn a_passed_through_layer_with_no_minimum_area_gets_no_patch() {
        // 🔑 A metal5 passed through exactly as metal6 is, and the reference writes nothing on it.
        // The guard, not the geometry, is what distinguishes them.
        let metal = (128700, 125210, 129580, 125530);
        assert_eq!(
            intermediate_patches(&[metal], &[metal], false, 0, Direction::Vertical, 5),
            Vec::<crate::Rect>::new()
        );
    }

    #[test]
    fn a_shortfall_against_the_minimum_area_grows_the_patch() {
        // metal6 carries `setArea 2359200`, and 880 x 320 falls far short of it.
        let metal = (128700, 125210, 129580, 125530);
        let patches = intermediate_patches(&[metal], &[metal], false, 2_359_200, Direction::Vertical, 5);
        assert_eq!(patches.len(), 1, "a shortfall must produce a patch");
        let patch = patches[0];
        assert_eq!(
            patch.2 - patch.0,
            880,
            "growth is along the layer, not across it"
        );
        let area = (patch.2 - patch.0) as i64 * (patch.3 - patch.1) as i64;
        assert!(area >= 2_359_200, "grown to {area}, short of the minimum");
    }

    #[test]
    fn two_metals_that_do_not_stack_into_a_rectangle_get_the_bounding_box() {
        // A cross: neither contains the other, so the union is not a rectangle and the bounding
        // box fills the notch even with no minimum area to satisfy.
        let wide = (0, 40, 100, 60);
        let tall = (40, 0, 60, 100);
        assert_eq!(
            intermediate_patches(&[wide], &[tall], false, 0, Direction::Vertical, 1),
            vec![(0, 0, 100, 100)]
        );
    }

    #[test]
    fn a_metal_that_covers_the_other_gets_no_patch() {
        let big = (0, 0, 100, 100);
        let small = (40, 40, 60, 60);
        assert_eq!(
            intermediate_patches(&[big], &[small], false, 0, Direction::Vertical, 1),
            Vec::<crate::Rect>::new()
        );
        assert_eq!(
            intermediate_patches(&[small], &[big], false, 0, Direction::Vertical, 1),
            Vec::<crate::Rect>::new()
        );
    }

    #[test]
    fn adjacency_is_read_off_the_array_shape() {
        // ⚠️ A 1x1 and a 1x2 fall below the two every rule requires.
        assert_eq!(adjacent_cuts(1, 1), 0);
        assert_eq!(adjacent_cuts(1, 2), 1);
        // 🔑 A 1xN counts TWO: the middle cut has a neighbour on each side.
        assert_eq!(adjacent_cuts(1, 3), 2);
        assert_eq!(adjacent_cuts(1, 99), 2);
        assert_eq!(adjacent_cuts(2, 2), 2);
        assert_eq!(adjacent_cuts(2, 5), 3);
        // Anything 3 or wider reaches four, and the clamp keeps it there.
        assert_eq!(adjacent_cuts(3, 3), 4);
        assert_eq!(adjacent_cuts(7, 7), 4);
    }

    #[test]
    fn an_adjacentcuts_rule_widens_a_big_enough_array() {
        // 🔑 The case that found this: a 7x7 array of 280 cuts on a layer stating
        // `SPACING 0.32 ADJACENTCUTS 3` -- 4 adjacent cuts reaches the rule's 3, so the pitch
        // goes from cut+320 to cut+640 and the array drops to what still fits.
        let rules = [(3u32, 640, false)];
        assert_eq!(
            adjacent_cut_pitch(7, 7, (280, 280), &rules),
            Some((920, 920))
        );
        // ⚠️ Too few adjacent cuts and the rule does not bite, however large its spacing.
        assert_eq!(adjacent_cut_pitch(1, 2, (280, 280), &rules), None);
        // A 1xN reaches two, which is still short of three.
        assert_eq!(adjacent_cut_pitch(1, 9, (280, 280), &rules), None);
        // ⚠️ An EXCEPTSAMEPGNET rule is skipped outright -- a power grid is that case.
        assert_eq!(
            adjacent_cut_pitch(7, 7, (280, 280), &[(3, 640, true)]),
            None
        );
        // The LAST matching rule wins, as the reference assigns rather than accumulates.
        assert_eq!(
            adjacent_cut_pitch(7, 7, (280, 280), &[(3, 640, false), (2, 500, false)]),
            Some((780, 780))
        );
    }

    #[test]
    fn an_enclosure_is_snapped_down_onto_the_manufacturing_grid() {
        // 🔑 The case that found this. A 370-tall opening over a 280 cut leaves 90, halved to 45,
        // and 45 on a grid of 10 is metal 370 tall in a 360-tall opening — five units past each
        // end. Snapped down to 40 the metal is 360 and fits exactly.
        assert_eq!(
            snap_enclosure(Enclosure { x: 0, y: 45 }, 10),
            Enclosure { x: 0, y: 40 }
        );
        // Already on the grid: untouched.
        assert_eq!(
            snap_enclosure(Enclosure { x: 260, y: 40 }, 10),
            Enclosure { x: 260, y: 40 }
        );
        // ⚠️ A negative enclosure — which `overlap_enclosure` produces for a shape narrower than
        // its own cut array, and carries through — truncates toward zero, as the reference's
        // integer division does.
        assert_eq!(
            snap_enclosure(Enclosure { x: -45, y: -5 }, 10),
            Enclosure { x: -40, y: 0 }
        );
        // No grid declared: nothing moves.
        assert_eq!(
            snap_enclosure(Enclosure { x: 45, y: 45 }, 0),
            Enclosure { x: 45, y: 45 }
        );
    }

    #[test]
    fn metals_left_apart_by_snapping_are_bridged_only_when_a_via_is_an_array() {
        // 🔑 The whole difference between a snapped array stack and a snapped split-cut one: the
        // same stack, snapped the same way, patched in one and not the other.
        let lower = (0, 0, 280, 200);
        let upper = (0, 420, 280, 620);
        assert_eq!(
            intermediate_patches(&[lower], &[upper], false, 0, Direction::Vertical, 1),
            Vec::<crate::Rect>::new(),
            "single cuts: two polygons, each already its own extents"
        );
        assert_eq!(
            intermediate_patches(&[lower], &[upper], true, 0, Direction::Vertical, 1),
            vec![(0, 0, 280, 620)],
            "an array asks for one box over everything"
        );
    }

    #[test]
    fn a_bounding_box_that_is_exactly_the_two_metals_stacked_adds_nothing() {
        // ⚠️ The box differs from BOTH inputs and is still not written: the test is by area, not by
        // comparing rectangles.
        let lower = (0, 0, 100, 50);
        let upper = (0, 50, 100, 100);
        assert_eq!(
            intermediate_patches(&[lower], &[upper], true, 0, Direction::Vertical, 1),
            Vec::<crate::Rect>::new()
        );
    }

    // ── the array patch, taken from the reference's own output ──────────────────────────────
    //
    // An ASAP7 design with ARRAYSPACING, net VSS, layer M7. The reference places four `via6_7`
    // and four `via7_8` at the same centres — (1418, 1418), (2742, 1418), (1418, 2742),
    // (2742, 2742) — and writes ONE patch, which its DEF states as
    //
    //     NEW M7 1648 + SHAPE DRCFILL ( 2080 1245 ) ( 2080 2915 )
    //
    // that is x 1256..2904 by y 1245..2915. Every via definition and all 24 placements already
    // matched; this is what was missing.
    fn asap7_m7_vss() -> (Vec<crate::Rect>, Vec<crate::Rect>) {
        let centres = [(1418, 1418), (2742, 1418), (1418, 2742), (2742, 2742)];
        // via6_7 ENCLOSURE 11 176 0 11: its TOP metal is the 324 cut span plus 0 across, 11 along.
        let tops = centres
            .iter()
            .map(|(x, y)| (x - 162, y - 173, x + 162, y + 173))
            .collect();
        // via7_8 ENCLOSURE 0 0 11 0: its BOTTOM metal is the cut span exactly.
        let bottoms = centres
            .iter()
            .map(|(x, y)| (x - 162, y - 162, x + 162, y + 162))
            .collect();
        (tops, bottoms)
    }

    #[test]
    fn an_array_patches_because_its_groups_leave_gaps() {
        let (tops, bottoms) = asap7_m7_vss();
        let patches = intermediate_patches(&tops, &bottoms, true, 0, Direction::Vertical, 1);
        assert_eq!(patches, vec![(1256, 1245, 2904, 2915)], "the reference's own patch");
    }

    #[test]
    fn the_same_metal_as_two_bounding_boxes_patches_nothing() {
        // 🔑 Why the lists are the whole point. Reduced to one box a side first, the bounding box
        // of the pair covers everything and the leftover is empty by construction — which is
        // exactly how four patches went missing while every via already matched.
        let (tops, bottoms) = asap7_m7_vss();
        let bbox = |rs: &[crate::Rect]| {
            rs.iter()
                .copied()
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
                .unwrap()
        };
        assert_eq!(
            intermediate_patches(&[bbox(&tops)], &[bbox(&bottoms)], true, 0, Direction::Vertical, 1),
            Vec::<crate::Rect>::new()
        );
    }

    #[test]
    fn the_per_group_boxes_are_absorbed_by_the_whole_set_box() {
        // 🔑 `patch_shapes += …` is a SET union. With the whole-set bounding box already in it,
        // every per-group extent lies inside and adds nothing — one patch, not five. Kept as a
        // list this case wrote twelve patches where the reference writes four, the reference's own
        // four among them.
        let (tops, bottoms) = asap7_m7_vss();
        let patches = intermediate_patches(&tops, &bottoms, true, 0, Direction::Vertical, 1);
        assert_eq!(patches.len(), 1, "one patch, however many groups the array has");
    }

    #[test]
    fn a_group_of_touching_metal_contributes_its_own_extent() {
        // `get_polygons` then `extents` runs in BOTH branches, so two separated pairs give two
        // candidates even where no whole-set box is asked for. Neither adds metal here — each pair
        // is solid — so the answer is empty, and that is the point: the candidates are built first
        // and filtered last.
        let a = [(0, 0, 10, 10), (10, 0, 20, 10)];
        let b = [(100, 0, 110, 10), (110, 0, 120, 10)];
        assert_eq!(
            intermediate_patches(&a, &b, false, 0, Direction::Vertical, 1),
            Vec::<crate::Rect>::new()
        );
    }

    #[test]
    fn overlapping_metal_is_not_counted_twice() {
        // ⚠️ Two vias whose metal overlaps cover their union, not the sum of the two. Summed, the
        // patch looks already covered and is dropped.
        let a = [(0, 0, 20, 10), (10, 0, 30, 10)];
        let b = [(0, 0, 30, 10)];
        assert_eq!(
            intermediate_patches(&a, &b, true, 0, Direction::Vertical, 1),
            Vec::<crate::Rect>::new(),
            "the union is solid, so nothing is added"
        );
    }

    #[test]
    fn no_candidates_gives_nothing_rather_than_a_default() {
        assert_eq!(best_enclosures(&[]), None);
    }
}

#[cfg(test)]
mod rule_layer_order_tests {
    use super::*;

    // Nangate45 routing levels: metal9 is 9, metal10 is 10.
    #[test]
    fn via8array_0_declares_its_layers_lower_first() {
        // `LAYER metal8 ; LAYER metal9 ; LAYER via8 ;`
        assert_eq!(rule_layer_order(8, 9), (0, 1));
    }

    #[test]
    fn via9array_0_declares_its_layers_upper_first() {
        // `LAYER metal10 ; LAYER metal9 ; LAYER via9 ;` — the one rule in the file that does.
        assert_eq!(rule_layer_order(10, 9), (1, 0));
    }
}

#[cfg(test)]
mod tech_via_fit_tests {
    use super::*;
    use crate::Rect;

    // A design with a switched region domain, at the met4/met5 crossing the reference refuses.
    // The region grid's
    // ring blanket cuts the Core's met5 stripe back to x 63600, leaving 770 of overlap with the
    // met4 stripe at 62770..64370 — and the reference answers, in one line per site.
    //
    //   [WARNING PDN-0110] No via inserted between met4 and met5
    //                      at (63.6000, 12.8000) - (64.3700, 14.4000) on VSS
    const INTERSECTION: Rect = (63600, 12800, 64370, 14400);
    // The met4 stripe, which runs the height of the die; the met5 stripe, which runs to 114370.
    const MET4_SHAPE: Rect = (62770, 12800, 64370, 163440);
    const MET5_SHAPE: Rect = (63600, 12800, 114370, 14400);
    // sky130hd `VIA M4M5_PR`: cut -0.4..0.4, met4 -0.59..0.59, met5 -0.71..0.71, placed at the
    // intersection's centre (63985, 13600).
    const M4M5_PR_MET4: Rect = (62805, 12420, 65165, 14780);
    const M4M5_PR_MET5: Rect = (62565, 12180, 65405, 15020);

    #[test]
    fn the_technologys_via_is_refused_where_the_reference_reports_no_via_inserted() {
        // met4 is the vertical shape, so it must fit in x — and 2360 of metal does not sit in 770.
        let bottom = constraint_for(Direction::Vertical, true, false);
        assert!(!mostly_contains(
            MET4_SHAPE,
            INTERSECTION,
            M4M5_PR_MET4,
            bottom,
            Direction::Vertical
        ));
    }

    #[test]
    fn the_axis_a_shape_runs_along_is_not_tested() {
        // met5 is horizontal, so only y is asked of it — and 2840 of metal does not sit in 1600
        // either, but the reason is y, not x. Made narrow in y alone, it passes.
        let top = constraint_for(Direction::Horizontal, true, false);
        assert!(!mostly_contains(
            MET5_SHAPE,
            INTERSECTION,
            M4M5_PR_MET5,
            top,
            Direction::Horizontal
        ));
        let short_in_y = (62565, 12800, 65405, 14400);
        assert!(mostly_contains(
            MET5_SHAPE,
            INTERSECTION,
            short_in_y,
            top,
            Direction::Horizontal
        ));
    }

    #[test]
    fn the_check_is_against_the_intersection_and_not_the_shape() {
        // 🔑 The whole of the defect this rule closes. The met4 stripe is 1600 wide and could hold
        // a via that the 770 of overlap cannot; asking the SHAPE admits it.
        let bottom = constraint_for(Direction::Vertical, true, false);
        let fits_the_stripe = (62970, 12420, 64170, 14780);
        assert!(!mostly_contains(
            MET4_SHAPE,
            INTERSECTION,
            fits_the_stripe,
            bottom,
            Direction::Vertical
        ));
        assert!(mostly_contains(
            MET4_SHAPE,
            fits_the_stripe,
            fits_the_stripe,
            bottom,
            Direction::Vertical
        ));
    }

    #[test]
    fn an_unmodifiable_shape_is_asked_for_both_axes() {
        let both = constraint_for(Direction::Horizontal, false, false);
        let inside = (63700, 12900, 64300, 14300);
        assert!(mostly_contains(
            MET5_SHAPE,
            INTERSECTION,
            inside,
            both,
            Direction::Horizontal
        ));
        let over_in_x = (63500, 12900, 64300, 14300);
        assert!(!mostly_contains(
            MET5_SHAPE,
            INTERSECTION,
            over_in_x,
            both,
            Direction::Horizontal
        ));
    }

    #[test]
    fn a_level_inside_the_stack_needs_three_sides_of_the_shape() {
        // Neither axis is constrained, so the count decides, and it is taken against the SHAPE.
        let free = Constraint::default();
        let three_sides = (62700, 12900, 64300, 14300);
        assert!(mostly_contains(
            MET4_SHAPE,
            INTERSECTION,
            three_sides,
            free,
            Direction::Vertical
        ));
        // Past the shape on both x sides: two of four, so the layer's own direction decides, and a
        // vertical layer is allowed to bridge along y but must still sit inside x.
        let past_both_x = (62700, 12900, 64500, 14300);
        assert!(!mostly_contains(
            MET4_SHAPE,
            INTERSECTION,
            past_both_x,
            free,
            Direction::Vertical
        ));
        assert!(mostly_contains(
            MET4_SHAPE,
            (62700, 12900, 64500, 14300),
            past_both_x,
            free,
            Direction::Vertical
        ));
    }
}

#[cfg(test)]
mod array_tests {
    use super::*;

    // ASAP7's cut layers, whose LEF states
    //
    //     ARRAYSPACING CUTSPACING 0.114
    //       ARRAYCUTS 3 SPACING 1.0
    //       ARRAYCUTS 4 SPACING 1.5
    //       ARRAYCUTS 5 SPACING 2.0 ;
    //
    // at 1000 database units per micron.
    fn asap7() -> Vec<ArrayRule> {
        vec![ArrayRule {
            cut_class: String::new(),
            parallel_overlap: false,
            long_array: false,
            array_width: 0,
            cut_spacing: 114,
            cuts_spacing: vec![(3, 1000), (4, 1500), (5, 2000)],
        }]
    }

    // A 2000 x 2000 strap crossing, a 32 x 32 cut, and the layer's own 46 spacing — the pitch of
    // 78 our engine builds a flat 25 x 25 at.
    fn fit(rules: &[ArrayRule]) -> Option<ArrayFit> {
        array_fit(rules, None, (2000, 2000), (32, 32), (78, 78), (50, 50), (50, 50), (0, 0), (25, 25))
    }

    #[test]
    fn the_greatest_cut_area_wins_and_it_is_the_smallest_group() {
        // Three groups of 3 at the rule's 114 spacing occupy 324 and repeat every 1324, so two
        // groups fit in the 1900 available and carry 6 cuts across; 4 and 5 fit only one group
        // each, for 4 and 5. 6 x 6 cuts beats 5 x 5, so the rule that allows the SMALLEST group is
        // the one that wins.
        let f = fit(&asap7()).expect("the rules apply");
        assert_eq!(f.core, (3, 3), "three cuts to a group");
        assert_eq!(f.groups, (2, 2), "two groups each way");
        assert_eq!(f.cut_pitch, (146, 146), "32 wide at the rule's 114 spacing");
        assert_eq!(f.array_spacing, (1000, 1000));
    }

    #[test]
    fn the_reference_names_its_own_answer() {
        // `via6_7_2000_2000_3_3_146_146` — rows, columns and both cut pitches, straight off the
        // via the reference writes. Ours was `via6_7_2000_2000_25_25_78_78`.
        let f = fit(&asap7()).unwrap();
        assert_eq!((f.core.1, f.core.0, f.cut_pitch.0, f.cut_pitch.1), (3, 3, 146, 146));
    }

    #[test]
    fn a_via_that_holds_one_cut_either_way_is_never_an_array() {
        // ⚠️ The gate is the PLAIN fit, before any rule is read.
        assert_eq!(
            array_fit(&asap7(), None, (2000, 2000), (32, 32), (78, 78), (50, 50), (50, 50), (0, 0), (1, 1)),
            None
        );
    }

    #[test]
    fn a_rule_asking_for_more_cuts_than_the_smaller_side_holds_is_skipped() {
        // ⚠️ The test is `rule_cuts > array_size_min`, and `array_size_min` is the SMALLER of the
        // plain fit. With 3 x 2 the smaller side holds two, so ARRAYCUTS 3 is one too many and
        // every line of this rule is skipped — no array at all, not a smaller one.
        let plain_fit = (3, 2);
        assert_eq!(
            array_fit(&asap7(), None, (2000, 2000), (32, 32), (78, 78), (50, 50), (50, 50), (0, 0), plain_fit),
            None
        );
        // 🔑 A LONG array is allowed exactly one more: `rule_cuts > min + 1`. So ARRAYCUTS 3 now
        // applies where it did not, and the ALONG axis is left unclamped — every cut that fits.
        let mut long = asap7();
        long[0].long_array = true;
        let f = array_fit(&long, None, (2000, 2000), (32, 32), (78, 78), (50, 50), (50, 50), (0, 0), plain_fit)
            .expect("a long array admits one more group");
        assert_eq!(f.core.1, 3, "the across axis is still clamped to the rule");
        assert!(f.core.0 > 3, "the along axis is not");
    }

    #[test]
    fn a_parallel_overlap_rule_is_never_used() {
        let mut rules = asap7();
        rules[0].parallel_overlap = true;
        assert_eq!(fit(&rules), None);
    }

    #[test]
    fn a_rule_for_another_cut_class_does_not_apply_and_a_nameless_one_applies_to_all() {
        let mut named = asap7();
        named[0].cut_class = "VX".to_string();
        assert!(array_fit(&named, Some("OTHER"), (2000, 2000), (32, 32), (78, 78), (50, 50), (50, 50), (0, 0), (25, 25)).is_none());
        assert!(array_fit(&named, Some("VX"), (2000, 2000), (32, 32), (78, 78), (50, 50), (50, 50), (0, 0), (25, 25)).is_some());
        // ⚠️ Nameless means EVERY class, which is the half that is easy to invert.
        assert!(array_fit(&asap7(), Some("VX"), (2000, 2000), (32, 32), (78, 78), (50, 50), (50, 50), (0, 0), (25, 25)).is_some());
    }

    #[test]
    fn a_rule_wider_than_the_shape_is_skipped_and_zero_is_no_limit() {
        let mut wide = asap7();
        wide[0].array_width = 5000;
        assert_eq!(fit(&wide), None);
        wide[0].array_width = 0;
        assert!(fit(&wide).is_some(), "zero states no limit, not a limit of nothing");
    }

    #[test]
    fn an_even_array_is_the_core_via_at_every_spot() {
        // Two groups of 3 each way, no remainder: four placements, one definition.
        let f = fit(&asap7()).unwrap();
        let p = array_placements(&f, (32, 32));
        assert_eq!(p.len(), 4);
        assert!(p.iter().all(|q| q.cuts == (3, 3)), "one definition, four spots");
        // A group of three at pitch 146 spans 324; two of them are 1000 apart, so their centres
        // are 1324 apart and sit symmetrically about zero.
        let xs: Vec<i32> = p.iter().map(|q| q.at.0).collect();
        assert_eq!(xs[1] - xs[0], 1324, "the step is a group plus the array spacing");
        assert_eq!(xs[0], -(xs[1]), "the array is centred on zero");
    }

    #[test]
    fn a_remainder_leads_the_array_and_takes_its_own_definition() {
        // ⚠️ The SHORT group is placed at row 0 and column 0, not last.
        let f = ArrayFit {
            groups: (2, 1),
            core: (3, 3),
            end: (1, 0),
            cut_pitch: (146, 146),
            array_spacing: (1000, 1000),
            double_enclosure: (0, 0),
        };
        let p = array_placements(&f, (32, 32));
        assert_eq!(p.len(), 3, "two full groups and one short, in a single row");
        assert_eq!(p[0].cuts, (1, 3), "the short column leads");
        assert_eq!(p[1].cuts, (3, 3));
        assert_eq!(p[2].cuts, (3, 3));
        // ⚠️ The step out of the short group is measured between the two extents, so it is
        // shorter than the step between two full ones: (324 + 32)/2 + 1000 against 324 + 1000.
        assert_eq!(p[1].at.0 - p[0].at.0, (324 + 32) / 2 + 1000);
        assert_eq!(p[2].at.0 - p[1].at.0, 324 + 1000);
    }

    #[test]
    fn one_group_each_way_is_a_single_via_at_the_centre() {
        // `isCutArray()` is false here, and this answers the same thing: one placement, centred.
        let f = ArrayFit {
            groups: (1, 1),
            core: (3, 3),
            end: (0, 0),
            cut_pitch: (146, 146),
            array_spacing: (1000, 1000),
            double_enclosure: (0, 0),
        };
        assert_eq!(
            array_placements(&f, (32, 32)),
            vec![ArrayPlacement { cuts: (3, 3), at: (0, 0) }]
        );
    }

    #[test]
    fn with_no_cut_spacing_of_its_own_the_rule_keeps_the_via_pitch() {
        let mut plain = asap7();
        plain[0].cut_spacing = 0;
        let f = fit(&plain).expect("the rules still apply");
        assert_eq!(f.cut_pitch, (78, 78), "the pitch the via already had");
    }
}
