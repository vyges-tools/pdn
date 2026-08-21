// SPDX-License-Identifier: Apache-2.0
//! Grid orchestration — the order the components of a grid are built in.
//!
//! A grid is a list of rings and strap sets. Building it is not a matter of asking each in turn:
//! a component that produced nothing is given a second chance once the others have run, and the
//! whole set is then refined until nothing more changes.
//!
//! Nothing here touches a database. The components are behind a trait so the *sequence* can be
//! tested without any geometry at all, which is the part that is easy to get wrong and impossible
//! to see in a finished grid.

/// One thing a grid builds: a ring or a strap set.
pub trait Component {
    /// Build into the shared state. Returns whether **any shape was produced**.
    ///
    /// ⚠️ This is not success or failure. The reference returns `shape_count != getShapeCount()`,
    /// so a component that legitimately has nothing to add reports the same as one that could not
    /// proceed — and both are retried. Reading it as an error flag leads to error handling where
    /// the reference has a dependency retry.
    fn make(&mut self) -> bool;

    /// Adjust already-made shapes. Returns whether anything changed.
    fn refine(&mut self) -> bool;
}

/// **G1** — the order components are built in.
///
/// ⚠️ **Every ring is built before every strap set**, whatever order they were declared in. Each
/// component adds its shapes *and its obstructions* to the state the next one sees, so the order
/// is not cosmetic — but see the spec: rings sit in the core offset and straps inside the core, so
/// in practice they rarely contend and the ordering is hard to observe.
pub fn build_order<'a, C>(rings: &'a mut [C], straps: &'a mut [C]) -> Vec<&'a mut C> {
    rings.iter_mut().chain(straps.iter_mut()).collect()
}

/// **G4** — the boundary a core grid's straps are laid into.
///
/// The domain boundary, **bloated vertically** by half the widest follow pin — the reference's own
/// comment is "account for the width of the follow pins for straps".
///
/// ⚠️ **Vertical only.** A strap that runs along y takes its along extent from this, so its ends
/// reach the outer edges of the topmost and bottommost follow pins rather than stopping at the
/// core. A strap running along x is unaffected, because its along extent comes from x. Bloating
/// both axes moves every horizontal strap too, and is wrong.
///
/// ⚠️ **The widest follow pin, not the one on this layer** — a grid with follow pins on two layers
/// takes the larger, and every strap in it is extended by that.
pub fn domain_boundary(core: crate::Rect, widest_followpin: i32) -> crate::Rect {
    let half = widest_followpin / 2;
    (core.0, core.1 - half, core.2, core.3 + half)
}

/// **G5** — the area the rings occupy, which is what `-extend_to_core_ring` reaches to.
///
/// Starts from the domain boundary and is grown by each ring shape — ⚠️ **on one axis only, chosen
/// by the shape's own proportions**. A segment wider than it is tall lies along the top or bottom,
/// so it pushes the **y** range out; a taller-than-wide segment lies at a side and pushes **x**.
/// A square shape (`dx == dy`) grows both.
///
/// ⚠️ It only ever grows. Each bound is compared against the running value and moved outward, so
/// the result is a union and a ring inside the boundary changes nothing.
pub fn ring_area(domain_boundary: crate::Rect, ring_shapes: &[crate::Rect]) -> crate::Rect {
    let (mut x0, mut y0, mut x1, mut y1) = domain_boundary;
    for &(rx0, ry0, rx1, ry1) in ring_shapes {
        let (dx, dy) = (rx1 - rx0, ry1 - ry0);
        if dx == dy {
            x0 = x0.min(rx0);
            y0 = y0.min(ry0);
            x1 = x1.max(rx1);
            y1 = y1.max(ry1);
        } else if dx > dy {
            y0 = y0.min(ry0);
            y1 = y1.max(ry1);
        } else {
            x0 = x0.min(rx0);
            x1 = x1.max(rx1);
        }
    }
    (x0, y0, x1, y1)
}

/// **G2** — build every component, then give the empty-handed ones one more go.
///
/// ⚠️ **Exactly one retry, and a second failure is silent.** There is no loop and no diagnostic: a
/// component that produces nothing twice simply contributes nothing. Retrying to a fixed point
/// would be a different engine, and so would raising an error.
///
/// The retry order is the order they were deferred in, which is the build order filtered — not the
/// build order re-walked.
pub fn make_all(components: &mut [&mut dyn Component]) -> usize {
    let mut deferred = Vec::new();
    for (i, c) in components.iter_mut().enumerate() {
        if !c.make() {
            deferred.push(i);
        }
    }
    for &i in &deferred {
        components[i].make();
    }
    deferred.len()
}

/// **G3** — refine until nothing changes.
///
/// ⚠️ **Every component is revisited each round**, not just the ones that changed, and the loop
/// ends only when a whole round passes with no change. A single pass is a different engine.
///
/// ⚠️ The `modified` flag is accumulated with `|=` **after** calling `refine` on every component —
/// so a component later in the list still runs even once an earlier one has reported a change.
/// Short-circuiting the round would skip it.
pub fn refine_all(components: &mut [&mut dyn Component]) -> usize {
    let mut rounds = 0;
    loop {
        let mut modified = false;
        for c in components.iter_mut() {
            modified |= c.refine();
        }
        rounds += 1;
        if !modified {
            return rounds;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A component that records what was asked of it and when.
    struct Spy {
        name: &'static str,
        /// How many `make` calls produce nothing before one succeeds.
        empty_makes: usize,
        /// How many `refine` calls report a change before they stop.
        refines_left: usize,
        log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl Component for Spy {
        fn make(&mut self) -> bool {
            self.log.borrow_mut().push(format!("make:{}", self.name));
            if self.empty_makes > 0 {
                self.empty_makes -= 1;
                return false;
            }
            true
        }
        fn refine(&mut self) -> bool {
            self.log.borrow_mut().push(format!("refine:{}", self.name));
            if self.refines_left > 0 {
                self.refines_left -= 1;
                return true;
            }
            false
        }
    }

    fn spy(
        name: &'static str,
        empty_makes: usize,
        refines_left: usize,
        log: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    ) -> Spy {
        Spy {
            name,
            empty_makes,
            refines_left,
            log: log.clone(),
        }
    }

    #[test]
    fn the_strap_boundary_grows_vertically_by_half_the_follow_pin() {
        // ⚠️ y only. 340-wide follow pins push the boundary 170 out at top and bottom, which is
        // what makes a vertical strap reach the outer edge of the outermost rail.
        assert_eq!(
            domain_boundary((20140, 22400, 180500, 179200), 340),
            (20140, 22230, 180500, 179370)
        );
    }

    #[test]
    fn a_grid_with_no_follow_pins_keeps_its_boundary() {
        let core = (0, 0, 100, 100);
        assert_eq!(domain_boundary(core, 0), core);
    }

    #[test]
    fn an_odd_follow_pin_width_bloats_by_the_truncated_half() {
        assert_eq!(domain_boundary((0, 0, 100, 100), 41), (0, -20, 100, 120));
    }

    #[test]
    fn a_horizontal_ring_side_grows_only_the_y_range() {
        // ⚠️ It lies along the top or bottom, so it says nothing about how far the grid reaches
        // in x. Growing both axes would stretch every strap that extends to the rings.
        let b = (100, 100, 900, 900);
        assert_eq!(ring_area(b, &[(0, 50, 1000, 90)]), (100, 50, 900, 900));
    }

    #[test]
    fn a_vertical_ring_side_grows_only_the_x_range() {
        let b = (100, 100, 900, 900);
        assert_eq!(ring_area(b, &[(50, 0, 90, 1000)]), (50, 100, 900, 900));
    }

    #[test]
    fn a_square_ring_shape_grows_both() {
        let b = (100, 100, 900, 900);
        assert_eq!(ring_area(b, &[(50, 50, 90, 90)]), (50, 50, 900, 900));
    }

    #[test]
    fn a_ring_inside_the_boundary_changes_nothing() {
        let b = (100, 100, 900, 900);
        assert_eq!(ring_area(b, &[(200, 200, 800, 240)]), b);
    }

    #[test]
    fn every_side_contributes_to_its_own_axis() {
        let b = (100, 100, 900, 900);
        let sides = [
            (0, 50, 1000, 90),
            (0, 910, 1000, 950),
            (50, 0, 90, 1000),
            (910, 0, 950, 1000),
        ];
        assert_eq!(ring_area(b, &sides), (50, 50, 950, 950));
    }

    #[test]
    fn rings_are_built_before_straps_whatever_order_they_were_declared_in() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut rings = [spy("ring", 0, 0, &log)];
        let mut straps = [spy("strap", 0, 0, &log)];
        let mut order = build_order(&mut rings, &mut straps);
        let mut refs: Vec<&mut dyn Component> =
            order.iter_mut().map(|c| *c as &mut dyn Component).collect();
        make_all(&mut refs);
        assert_eq!(*log.borrow(), vec!["make:ring", "make:strap"]);
    }

    #[test]
    fn a_component_that_produced_nothing_is_retried_after_the_others() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (mut a, mut b, mut c) = (
            spy("a", 1, 0, &log),
            spy("b", 0, 0, &log),
            spy("c", 0, 0, &log),
        );
        let mut refs: Vec<&mut dyn Component> = vec![&mut a, &mut b, &mut c];
        assert_eq!(make_all(&mut refs), 1);
        assert_eq!(
            *log.borrow(),
            vec!["make:a", "make:b", "make:c", "make:a"],
            "a is retried only after every other component has had its first go"
        );
    }

    #[test]
    fn a_second_failure_is_silent_and_not_retried_again() {
        // ⚠️ One retry, not a loop. A component that produces nothing twice contributes nothing and
        // says nothing about it.
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut a = spy("a", 5, 0, &log);
        let mut refs: Vec<&mut dyn Component> = vec![&mut a];
        make_all(&mut refs);
        assert_eq!(
            *log.borrow(),
            vec!["make:a", "make:a"],
            "twice, then left alone"
        );
    }

    #[test]
    fn the_deferred_keep_their_build_order_among_themselves() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (mut a, mut b, mut c) = (
            spy("a", 1, 0, &log),
            spy("b", 0, 0, &log),
            spy("c", 1, 0, &log),
        );
        let mut refs: Vec<&mut dyn Component> = vec![&mut a, &mut b, &mut c];
        assert_eq!(make_all(&mut refs), 2);
        assert_eq!(log.borrow()[3..], ["make:a", "make:c"]);
    }

    #[test]
    fn refining_revisits_every_component_each_round_until_a_round_is_quiet() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        // `a` keeps changing for two rounds; `b` never does but must still be asked each round.
        let (mut a, mut b) = (spy("a", 0, 2, &log), spy("b", 0, 0, &log));
        let mut refs: Vec<&mut dyn Component> = vec![&mut a, &mut b];
        assert_eq!(
            refine_all(&mut refs),
            3,
            "two changing rounds and one quiet one"
        );
        assert_eq!(
            *log.borrow(),
            vec!["refine:a", "refine:b", "refine:a", "refine:b", "refine:a", "refine:b"]
        );
    }

    #[test]
    fn a_later_component_is_still_refined_after_an_earlier_one_reports_a_change() {
        // ⚠️ `modified |= ...` over the whole round, not an early exit. Short-circuiting once a
        // change is seen would skip everything after it.
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (mut a, mut b) = (spy("a", 0, 1, &log), spy("b", 0, 0, &log));
        let mut refs: Vec<&mut dyn Component> = vec![&mut a, &mut b];
        refine_all(&mut refs);
        assert_eq!(
            log.borrow()[1],
            "refine:b",
            "b runs in the same round a changed in"
        );
    }

    #[test]
    fn a_grid_whose_components_never_change_still_costs_one_round() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut a = spy("a", 0, 0, &log);
        let mut refs: Vec<&mut dyn Component> = vec![&mut a];
        assert_eq!(refine_all(&mut refs), 1);
    }
}
