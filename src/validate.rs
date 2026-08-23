// SPDX-License-Identifier: Apache-2.0
//! The checks a grid component's stated dimensions must pass before it is accepted.
//!
//! 🔑 **These run when the component is DECLARED, not when the grid is built.** The reference
//! validates inside `addRing`/`addStrap`, so a bad width is refused by the command that stated it
//! and nothing is added — a design naming three bad straps reports three separate diagnostics, one
//! per command, and builds none of them. ⟹ Each declaration has to be checkable on its own; a
//! single run carrying all three can only ever report the first.
//!
//! Every rule here is a pure function over values the caller reads from the database, so the
//! technology is a set of numbers in a test rather than a LEF file.
//!
//! ⚠️ **The first rule to fire wins and stops the run.** The reference raises through its logger,
//! which throws, so the checks after it never execute. Returning every violation instead would
//! report diagnostics the reference never emits.

use crate::Direction;

/// A refused component: the reference's own message number and text.
///
/// The number is asserted by the upstream tests as much as the words are — `PDN-0117` and
/// `PDN-0118` differ only in which multiple they demand, and a test that read the text alone would
/// pass with the codes swapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub code: u32,
    pub message: String,
}

impl Diag {
    fn new(code: u32, message: String) -> Option<Diag> {
        Some(Diag { code, message })
    }
}

impl std::fmt::Display for Diag {
    /// The reference's own line: `[ERROR PDN-0106] <message>`, the number zero-padded to four.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[ERROR PDN-{:04}] {}", self.code, self.message)
    }
}

/// What a routing layer says about the shapes allowed on it.
///
/// ⚠️ `max_width` is the technology's own default — a very large number — where the layer declares
/// no `MAXWIDTH`, not zero and not an absent value. Modelling "no maximum" as `None` and forgetting
/// a branch turns every wide strap into a violation.
#[derive(Debug, Clone)]
pub struct LayerRules {
    pub name: String,
    pub min_width: i32,
    pub max_width: i32,
    /// The layer's preferred direction, which is what a width table is measured against.
    pub direction: Direction,
    /// `WIDTHTABLE` rules as `(is_wrong_direction, widths)`, in the technology's own order.
    pub width_tables: Vec<(bool, Vec<i32>)>,
    /// Database units per micron, for rendering a value the way the reference renders it.
    pub units_per_micron: i32,
    /// The technology's manufacturing grid, or `None` where it declares none.
    pub manufacturing_grid: Option<i32>,
}

impl LayerRules {
    /// A value in microns, as the reference prints it: four decimal places, always.
    ///
    /// ℹ️ Both sides format the exact binary double, so a value that is not a tie renders
    /// identically. A tie at the fifth decimal cannot arise from a database whose units per micron
    /// divide 10000 — 1000 and 2000 both do — because every representable value is then already
    /// exact in four places.
    fn um(&self, v: i32) -> String {
        format!("{:.4}", v as f64 / self.units_per_micron as f64)
    }
}

/// **PDN-0106 / 0107 / 0114 / 0117** — is this width allowed on this layer, for a shape running
/// this way?
///
/// `direction` is the SHAPE's direction, which is not always the layer's: a follow pin takes its
/// direction from the standard-cell rows, and a strap may be laid across its layer's grain.
pub fn check_width(l: &LayerRules, width: i32, direction: Direction) -> Option<Diag> {
    // The layer's own minimum, below which no shape is legal.
    if width < l.min_width {
        return Diag::new(
            106,
            format!(
                "Width ({} um) specified for layer {} is less than minimum width ({} um).",
                l.um(width),
                l.name,
                l.um(l.min_width)
            ),
        );
    }

    if width > l.max_width {
        return Diag::new(
            107,
            format!(
                "Width ({} um) specified for layer {} is greater than maximum width ({} um).",
                l.um(width),
                l.name,
                l.um(l.max_width)
            ),
        );
    }

    // ⚠️ **A width table is a whitelist that does not always apply**, and there are three separate
    // ways out of it. `WRONGDIRECTION` INVERTS which shapes it governs rather than adding a
    // condition; an empty table governs nothing; and a width past the table's last entry is off
    // the end of the table rather than absent from it.
    for (wrong_direction, widths) in &l.width_tables {
        let applies = if *wrong_direction {
            direction != l.direction
        } else {
            direction == l.direction
        };
        if !applies || widths.is_empty() {
            continue;
        }
        if width > *widths.last().unwrap() {
            continue;
        }
        if widths.contains(&width) {
            continue;
        }
        let listed: Vec<String> = widths.iter().map(|w| l.um(*w)).collect();
        return Diag::new(
            114,
            // ⚠️ "in not a valid width" is the reference's own wording, typo and all. The upstream
            // golden asserts the exact string, so correcting the grammar fails the test.
            format!(
                "Width ({} um) specified for layer {} in not a valid width, must be {}.",
                l.um(width),
                l.name,
                listed.join(", ")
            ),
        );
    }

    // ⚠️ **A width must be a multiple of TWICE the manufacturing grid** — a wire is centred on a
    // track, so half of it lands either side and each half has to sit on the grid. Spacing takes
    // the grid itself (PDN-0118). The two rules read alike and are not the same rule.
    if let Some(grid) = l.manufacturing_grid {
        let double = 2 * grid;
        if double != 0 && width % double != 0 {
            return Diag::new(
                117,
                format!(
                    "Width ({} um) specified must be a multiple of {} um.",
                    l.um(width),
                    l.um(double)
                ),
            );
        }
    }

    None
}

/// **PDN-0108 / 0118** — is this spacing allowed beside a shape of this width?
///
/// `min_spacing` is what the technology answers for a shape of `width`, which the caller reads from
/// the database: the larger of the layer's own spacing rules and its two-widths table.
pub fn check_spacing(l: &LayerRules, spacing: i32, min_spacing: i32) -> Option<Diag> {
    if spacing < min_spacing {
        return Diag::new(
            108,
            format!(
                "Spacing ({} um) specified for layer {} is less than minimum spacing ({} um).",
                l.um(spacing),
                l.name,
                l.um(min_spacing)
            ),
        );
    }

    // The grid itself here, not twice it — see the note on PDN-0117.
    if let Some(grid) = l.manufacturing_grid {
        if grid != 0 && spacing % grid != 0 {
            return Diag::new(
                118,
                format!(
                    "Spacing ({} um) specified must be a multiple of {} um.",
                    l.um(spacing),
                    l.um(grid)
                ),
            );
        }
    }

    None
}

/// **PDN-0191** — does a plain distance sit on the manufacturing grid?
///
/// ⚠️ **`noun` is part of the message and varies by caller** — `Pitch`, `Offset`, `Core offset`.
/// One code carries several texts, so the code alone does not identify what was checked.
pub fn check_on_grid(l: &LayerRules, noun: &str, value: i32) -> Option<Diag> {
    let grid = l.manufacturing_grid?;
    if grid == 0 || value % grid == 0 {
        return None;
    }
    Diag::new(
        191,
        format!(
            "{} of {} um does not fit the manufacturing grid of {} um.",
            noun,
            l.um(value),
            l.um(grid)
        ),
    )
}

/// The dimensions a strap set states.
#[derive(Debug, Clone, Copy)]
pub struct StrapDims {
    pub width: i32,
    /// The stated spacing, or the derived one where the command stated none.
    pub spacing: i32,
    pub pitch: i32,
    pub offset: i32,
    /// The layer's own minimum spacing for a shape of `width`, read from the database.
    pub min_spacing: i32,
    /// `-snap_to_grid`: the set is to be snapped to the layer's routing tracks.
    pub snap: bool,
    /// Whether the layer HAS routing tracks in the strap's direction, read from the database.
    pub has_track_grid: bool,
    /// The grid's own extent across the strap's direction — what the group has to fit inside.
    pub grid_width: i32,
    /// How many nets the group carries; the group is that many straps and the gaps between them.
    pub net_count: i32,
}

/// **PDN-0215** — a set asked to snap to tracks on a layer that declares none.
///
/// ⚠️ **Only when snapping was ASKED FOR.** A layer with no routing grid is perfectly ordinary;
/// it is the combination with `-snap_to_grid` that has no answer.
pub fn check_snap(l: &LayerRules, snap: bool, has_track_grid: bool) -> Option<Diag> {
    if !snap || has_track_grid {
        return None;
    }
    Diag::new(
        215,
        format!(
            "Unable to snap strap on {} to grid, no routing grid defined for layer.",
            l.name
        ),
    )
}

/// **PDN-0003 / 0004 / 0005** — the two layers a connect rule names.
///
/// 🔑 **All three are raised by `Connect`'s CONSTRUCTOR, in this order**, so a rule that breaks
/// more than one reports the first: same-layer, then layer0 not routing, then layer1 not routing.
/// The reference raises through a logger that throws, so nothing after the first check runs — the
/// order is behaviour, not an implementation detail. `core_grid_check_connect_layers` asserts all
/// three, one per `catch` block, and `{via1 metal5}` against `{metal1 via2}` is what separates
/// 0004 from 0005.
///
/// ⚠️ **"Routing layer" is `getRoutingLevel() != 0`, not the layer's LEF type.** A cut layer and a
/// masterslice both answer 0; asking whether the name looks like a via would get `via1` right and
/// an unnamed masterslice wrong.
pub fn check_connect_layers(name0: &str, level0: i32, name1: &str, level1: i32) -> Option<Diag> {
    if name0 == name1 {
        return Diag::new(
            3,
            format!("Layers must be different in connect rule: {name0}"),
        );
    }
    if level0 == 0 {
        return Diag::new(4, format!("{name0} must be a routing layer"));
    }
    if level1 == 0 {
        return Diag::new(5, format!("{name1} must be a routing layer"));
    }
    None
}

/// **PDN-0185** — the group of straps does not fit in the grid, once the offset is taken.
///
/// The group is `net_count` straps and the gaps between them: `n*width + (n-1)*spacing`. It fails
/// when `grid_width < offset + group`.
///
/// ⚠️ **Three different precisions in one message** — the grid width prints to TWO decimals and the
/// two widths to ONE, where every other diagnostic in this family uses four. A message assembled
/// from the family's habit rather than from this line is wrong in three places at once.
pub fn check_group_fits(
    l: &LayerRules,
    grid_name: &str,
    grid_width: i32,
    offset: i32,
    group_width: i32,
) -> Option<Diag> {
    if grid_width >= offset + group_width {
        return None;
    }
    Diag::new(
        185,
        format!(
            "Insufficient width ({:.2} um) to add straps on layer {} in grid \"{}\" with total strap width {:.1} um and offset {:.1} um.",
            grid_width as f64 / l.units_per_micron as f64,
            l.name,
            grid_name,
            group_width as f64 / l.units_per_micron as f64,
            offset as f64 / l.units_per_micron as f64,
        ),
    )
}

/// The width a group of straps occupies: one per net, with the stated spacing between them.
pub fn strap_group_width(net_count: i32, width: i32, spacing: i32) -> i32 {
    net_count * width + (net_count - 1) * spacing
}

/// Every check a strap set's own dimensions must pass, in the order the reference runs them.
///
/// ⚠️ **The order is the answer, not just the outcome.** A command stating both a bad width and a
/// bad pitch reports the width, because the reference stops at the first. Running the manufacturing
/// grid checks before the layer ones would report the pitch instead.
pub fn check_strap(
    l: &LayerRules,
    d: StrapDims,
    direction: Direction,
    grid_name: &str,
) -> Option<Diag> {
    let group = strap_group_width(d.net_count, d.width, d.spacing);
    check_width(l, d.width, direction)
        .or_else(|| check_spacing(l, d.spacing, d.min_spacing))
        .or_else(|| check_on_grid(l, "Width", d.width))
        .or_else(|| check_on_grid(l, "Spacing", d.spacing))
        .or_else(|| check_on_grid(l, "Pitch", d.pitch))
        .or_else(|| check_on_grid(l, "Offset", d.offset))
        // ⚠️ The snap check sits AFTER the manufacturing-grid ones and BEFORE the fit check —
        // a set that is both unsnappable and too wide reports the snap.
        .or_else(|| check_snap(l, d.snap, d.has_track_grid))
        .or_else(|| check_group_fits(l, grid_name, d.grid_width, d.offset, group))
}

/// Every check one of a ring's two layers must pass.
///
/// ⚠️ **A ring measures its width against the LAYER's direction**, never against the shape's — the
/// ring runs both ways round, so there is no single shape direction to measure. The strap check
/// takes the shape's.
///
/// `offsets` are the four core offsets, each checked as `Core offset`.
pub fn check_ring_layer(
    l: &LayerRules,
    width: i32,
    spacing: i32,
    min_spacing: i32,
    offsets: &[i32],
) -> Option<Diag> {
    check_width(l, width, l.direction)
        .or_else(|| check_spacing(l, spacing, min_spacing))
        .or_else(|| check_on_grid(l, "Width", width))
        .or_else(|| check_on_grid(l, "Spacing", spacing))
        .or_else(|| offsets.iter().find_map(|o| check_on_grid(l, "Core offset", *o)))
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_connect_rule_names_two_different_routing_layers() {
        // Upstream rule, `Connect::Connect`: layer0 == layer1 raises PDN-0003, then
        // getRoutingLevel() == 0 raises PDN-0004 for the first layer and PDN-0005 for the second.
        assert_eq!(
            check_connect_layers("metal5", 5, "metal5", 5).unwrap().code,
            3
        );
        assert_eq!(check_connect_layers("via1", 0, "metal5", 5).unwrap().code, 4);
        assert_eq!(check_connect_layers("metal1", 1, "via2", 0).unwrap().code, 5);
        assert!(check_connect_layers("metal1", 1, "metal5", 5).is_none());
    }

    #[test]
    fn the_first_connect_rule_to_fire_is_the_whole_answer() {
        // ⚠️ The reference's logger throws, so a rule breaking two checks reports only the first.
        // Naming the SAME non-routing layer twice breaks both 0003 and 0004; 0003 is checked
        // first, so that is the diagnostic — reordering the checks would silently change it.
        let d = check_connect_layers("via1", 0, "via1", 0).unwrap();
        assert_eq!(d.code, 3);
        assert_eq!(d.message, "Layers must be different in connect rule: via1");
    }

    #[test]
    fn the_two_routing_layer_checks_differ_only_in_which_layer_failed() {
        // The message text is identical for 0004 and 0005; only the code and the layer name
        // differ, so a test reading the words alone would pass with the two swapped.
        let first = check_connect_layers("via1", 0, "via2", 0).unwrap();
        assert_eq!(first.code, 4);
        assert_eq!(first.message, "via1 must be a routing layer");
        let second = check_connect_layers("metal1", 1, "via2", 0).unwrap();
        assert_eq!(second.code, 5);
        assert_eq!(second.message, "via2 must be a routing layer");
    }
    use super::*;

    /// Nangate45: 2000 database units to the micron, a manufacturing grid of 0.005 um, and a
    /// metal5 whose minimum width and minimum spacing are both 0.14 um.
    fn nangate_metal5() -> LayerRules {
        LayerRules {
            name: "metal5".into(),
            min_width: 280,
            max_width: i32::MAX,
            direction: Direction::Horizontal,
            width_tables: Vec::new(),
            units_per_micron: 2000,
            manufacturing_grid: Some(10),
        }
    }

    #[test]
    fn a_width_under_the_layer_minimum_is_refused() {
        // Upstream rule: `checkLayerWidth` raises PDN-0106 when `width < layer->getMinWidth()`.
        let d = check_width(&nangate_metal5(), 200, Direction::Horizontal).unwrap();
        assert_eq!(d.code, 106);
        assert_eq!(
            d.message,
            "Width (0.1000 um) specified for layer metal5 is less than minimum width (0.1400 um)."
        );
    }

    #[test]
    fn a_width_over_the_layer_maximum_is_refused() {
        // Upstream rule: PDN-0107 when `width > layer->getMaxWidth()`. ASAP7's M8 declares a
        // MAXWIDTH of 2 um against 1000 units to the micron.
        let m8 = LayerRules {
            name: "M8".into(),
            min_width: 32,
            max_width: 2000,
            direction: Direction::Vertical,
            width_tables: Vec::new(),
            units_per_micron: 1000,
            manufacturing_grid: Some(1),
        };
        let d = check_width(&m8, 4000, Direction::Vertical).unwrap();
        assert_eq!(d.code, 107);
        assert_eq!(
            d.message,
            "Width (4.0000 um) specified for layer M8 is greater than maximum width (2.0000 um)."
        );
    }

    #[test]
    fn an_unstated_maximum_width_refuses_nothing() {
        // ⚠️ The technology's default is a very large number, not zero: a layer with no MAXWIDTH
        // accepts any width the other rules allow.
        assert!(check_width(&nangate_metal5(), 100_000, Direction::Horizontal).is_none());
    }

    /// ASAP7's M2, whose width table lists six widths and whose direction is horizontal.
    fn asap7_m2() -> LayerRules {
        LayerRules {
            name: "M2".into(),
            min_width: 18,
            max_width: i32::MAX,
            direction: Direction::Horizontal,
            width_tables: vec![(false, vec![18, 90, 162, 234, 306, 378])],
            units_per_micron: 1000,
            manufacturing_grid: Some(1),
        }
    }

    #[test]
    fn a_width_absent_from_the_table_is_refused_and_the_message_lists_the_table() {
        // Upstream rule: PDN-0114, and the message names every allowed width in the rule's own
        // order, comma separated.
        let d = check_width(&asap7_m2(), 72, Direction::Horizontal).unwrap();
        assert_eq!(d.code, 114);
        assert_eq!(
            d.message,
            "Width (0.0720 um) specified for layer M2 in not a valid width, \
             must be 0.0180, 0.0900, 0.1620, 0.2340, 0.3060, 0.3780."
        );
    }

    #[test]
    fn a_plain_table_governs_only_shapes_running_the_layers_own_way() {
        // Upstream rule: a rule that is not WRONGDIRECTION checks the table only when the shape's
        // direction EQUALS the layer's. A follow pin laid across M1 is why this matters: the same
        // width is refused on M2, whose direction the rows share, and allowed on M1, whose they
        // do not.
        assert!(check_width(&asap7_m2(), 72, Direction::Vertical).is_none());
    }

    #[test]
    fn a_wrongdirection_table_governs_the_opposite_shapes() {
        // ⚠️ The flag INVERTS the test rather than adding to it.
        let mut l = asap7_m2();
        l.width_tables = vec![(true, vec![18, 90])];
        assert!(check_width(&l, 72, Direction::Horizontal).is_none());
        assert_eq!(check_width(&l, 72, Direction::Vertical).unwrap().code, 114);
    }

    #[test]
    fn a_width_past_the_tables_last_entry_is_outside_the_table_not_absent_from_it() {
        // Upstream rule: `width > width_table.back()` switches the check off entirely.
        assert!(check_width(&asap7_m2(), 400, Direction::Horizontal).is_none());
        // And one below the last entry but not in the table is still refused.
        assert_eq!(
            check_width(&asap7_m2(), 300, Direction::Horizontal).unwrap().code,
            114
        );
    }

    #[test]
    fn an_empty_table_governs_nothing() {
        let mut l = asap7_m2();
        l.width_tables = vec![(false, Vec::new())];
        assert!(check_width(&l, 72, Direction::Horizontal).is_none());
    }

    #[test]
    fn a_width_must_be_a_multiple_of_TWICE_the_manufacturing_grid() {
        // Upstream rule: PDN-0117 tests `width % (2 * manufacturing grid)`. Nangate45's grid is
        // 0.005 um, so a width has to sit on 0.010 um — 2.001 um does not.
        let d = check_width(&nangate_metal5(), 4002, Direction::Horizontal).unwrap();
        assert_eq!(d.code, 117);
        assert_eq!(
            d.message,
            "Width (2.0010 um) specified must be a multiple of 0.0100 um."
        );
        // ⚠️ And a width on the grid but not on twice it is still refused: this is the case that
        // tells the two rules apart.
        assert_eq!(
            check_width(&nangate_metal5(), 4010, Direction::Horizontal).unwrap().code,
            117
        );
    }

    #[test]
    fn a_spacing_under_the_layer_minimum_is_refused() {
        // Upstream rule: PDN-0108 against `TechLayer::getSpacing(width)`, which the caller reads
        // from the database.
        let d = check_spacing(&nangate_metal5(), 200, 280).unwrap();
        assert_eq!(d.code, 108);
        assert_eq!(
            d.message,
            "Spacing (0.1000 um) specified for layer metal5 is less than minimum spacing (0.1400 um)."
        );
    }

    #[test]
    fn a_spacing_must_be_a_multiple_of_the_manufacturing_grid_ITSELF() {
        // ⚠️ PDN-0118 uses the grid, PDN-0117 twice it. Nangate45's grid is 0.005 um, so a
        // spacing of 0.504 um is refused where a spacing of 0.505 um would not be — and 0.505
        // would still fail the WIDTH rule, which is what makes the pair easy to conflate.
        let l = LayerRules {
            name: "metal1".into(),
            min_width: 130,
            ..nangate_metal5()
        };
        let d = check_spacing(&l, 1008, 0).unwrap();
        assert_eq!(d.code, 118);
        assert_eq!(
            d.message,
            "Spacing (0.5040 um) specified must be a multiple of 0.0050 um."
        );
        assert!(check_spacing(&l, 1010, 0).is_none());
    }

    #[test]
    fn an_off_grid_distance_names_what_was_checked() {
        // Upstream rule: PDN-0191's first field is the caller's own noun, so the same code reads
        // differently for a pitch and for a ring's core offset.
        let l = nangate_metal5();
        let d = check_on_grid(&l, "Pitch", 8008).unwrap();
        assert_eq!(d.code, 191);
        assert_eq!(
            d.message,
            "Pitch of 4.0040 um does not fit the manufacturing grid of 0.0050 um."
        );
        assert_eq!(
            check_on_grid(&l, "Core offset", 8008).unwrap().message,
            "Core offset of 4.0040 um does not fit the manufacturing grid of 0.0050 um."
        );
    }

    #[test]
    fn a_technology_with_no_manufacturing_grid_refuses_nothing_on_that_account() {
        let mut l = nangate_metal5();
        l.manufacturing_grid = None;
        assert!(check_on_grid(&l, "Pitch", 8008).is_none());
        assert!(check_width(&l, 4002, Direction::Horizontal).is_none());
        assert!(check_spacing(&l, 1008, 0).is_none());
    }

    #[test]
    fn a_strap_reports_the_FIRST_rule_it_breaks_not_the_worst() {
        // ⚠️ The reference raises through a logger that throws, so the checks after the first
        // never run. A strap breaking both the width rule and the pitch rule reports the width.
        let l = LayerRules {
            name: "metal1".into(),
            min_width: 130,
            ..nangate_metal5()
        };
        let d = check_strap(
            &l,
            StrapDims { width: 150, spacing: 1000, pitch: 8008, offset: 0, min_spacing: 0,
                        snap: false, has_track_grid: true, grid_width: i32::MAX, net_count: 2 },
            Direction::Horizontal,
            "Core",
        )
        .unwrap();
        assert_eq!(d.code, 117, "the width is checked before the pitch");
    }

    #[test]
    fn a_strap_on_grid_everywhere_is_accepted() {
        let l = LayerRules {
            name: "metal1".into(),
            min_width: 130,
            ..nangate_metal5()
        };
        assert!(check_strap(
            &l,
            StrapDims { width: 140, spacing: 1000, pitch: 8000, offset: 0, min_spacing: 280,
                        snap: false, has_track_grid: true, grid_width: i32::MAX, net_count: 2 },
            Direction::Horizontal,
            "Core",
        )
        .is_none());
    }

    #[test]
    fn a_ring_checks_its_core_offsets_and_names_them_as_such() {
        let l = nangate_metal5();
        let d = check_ring_layer(&l, 4000, 4000, 280, &[4000, 4000, 8008, 4000]).unwrap();
        assert_eq!(d.code, 191);
        assert!(d.message.starts_with("Core offset of 4.0040 um"));
    }

    #[test]
    fn snapping_to_a_layer_with_no_routing_grid_is_refused() {
        // Upstream rule: `Straps::checkLayerSpecifications` populates the layer's grid when
        // `snap_` is set and raises PDN-0215 when the layer has none.
        let l = LayerRules { name: "metal1".into(), ..nangate_metal5() };
        let d = check_snap(&l, true, false).unwrap();
        assert_eq!(d.code, 215);
        assert_eq!(
            d.message,
            "Unable to snap strap on metal1 to grid, no routing grid defined for layer."
        );
    }

    #[test]
    fn a_layer_with_no_grid_is_only_a_problem_when_snapping_was_asked_for() {
        let l = nangate_metal5();
        assert!(check_snap(&l, false, false).is_none(), "no -snap_to_grid, no complaint");
        assert!(check_snap(&l, true, true).is_none(), "asked, and the tracks are there");
    }

    #[test]
    fn a_group_too_wide_for_its_grid_is_refused() {
        // The upstream `design_width` case: ASAP7 M8, width 2.0 spacing 2.0 offset 10.0 over two
        // nets, so the group is 2*2 + 1*2 = 6.0 um and the grid is 14.04 um.
        let m8 = LayerRules {
            name: "M8".into(),
            units_per_micron: 1000,
            manufacturing_grid: Some(1),
            ..nangate_metal5()
        };
        let group = strap_group_width(2, 2000, 2000);
        assert_eq!(group, 6000, "two straps and the one gap between them");
        let d = check_group_fits(&m8, "Core", 14040, 10000, group).unwrap();
        assert_eq!(d.code, 185);
        // ⚠️ Three precisions in one line: the grid width to TWO decimals, the widths to ONE.
        assert_eq!(
            d.message,
            "Insufficient width (14.04 um) to add straps on layer M8 in grid \"Core\" \
             with total strap width 6.0 um and offset 10.0 um."
        );
    }

    #[test]
    fn a_group_that_exactly_fills_its_grid_is_accepted() {
        // ⚠️ The test is `grid_width < offset + group`, so equality passes — a group filling the
        // grid exactly is legal, and an off-by-one here refuses a whole class of tight designs.
        let l = nangate_metal5();
        assert!(check_group_fits(&l, "Core", 16000, 10000, 6000).is_none());
        assert_eq!(check_group_fits(&l, "Core", 15999, 10000, 6000).unwrap().code, 185);
    }

    #[test]
    fn a_diagnostic_prints_as_the_reference_prints_it() {
        let d = Diag { code: 106, message: "x".into() };
        assert_eq!(d.to_string(), "[ERROR PDN-0106] x");
    }
}
