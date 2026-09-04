// SPDX-License-Identifier: Apache-2.0
//! Everything this binary reads out of the database, in one place.
//!
//! 🔑 **The library above is pure and stays that way** — every module in `vyges_pdn` states that
//! nothing in it touches a database, and that is what makes those rules testable without a
//! fixture. This module is the other side of that line: it is the only place a `Db` is queried,
//! and it holds no policy of its own beyond reading faithfully.
//!
//! It belongs to the binary, not the library: `lib.rs` does not declare it.

use vyges_opendb::Db;
use vyges_pdn::{followpins, validate, Direction, Rect};

/// Micron to database units. ⚠️ Rounded, not truncated: `0.93` at 2000 dbu is 1860 exactly, but a
/// value like `2.0005` truncates to one unit less and every shape built from it is off by one.
pub(crate) fn dbu(micron: &str, per_micron: f64) -> i32 {
    (micron.parse::<f64>().unwrap_or(0.0) * per_micron).round() as i32
}

/// Interior overlap, as the via/shape association uses it.
pub(crate) fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

pub(crate) fn direction_of(db: &Db, layer: &str) -> Direction {
    match db
        .layers_with_direction()
        .unwrap_or_default()
        .into_iter()
        .find(|(n, _)| n == layer)
        .map(|(_, d)| d)
        .unwrap_or_default()
        .as_str()
    {
        "HORIZONTAL" => Direction::Horizontal,
        "VERTICAL" => Direction::Vertical,
        _ => Direction::None,
    }
}

/// The width of a follow pin, from the standard cells — the reference's `determineWidth`.
pub(crate) fn followpin_width(db: &Db) -> Option<i32> {
    let mut boxes = Vec::new();
    for i in 0..db.num_masters().unwrap_or(0) {
        let Ok(master) = db.nth_master_name(i) else {
            continue;
        };
        let is_core = db.master_is_core(&master);
        for term in db.master_get_m_terms(&master) {
            let supply = matches!(
                db.mterm_get_sig_type(&master, &term).as_str(),
                "POWER" | "GROUND"
            );
            for (layer, _, y0, _, y1) in db.mterm_pin_boxes(&master, &term).unwrap_or_default() {
                let name = db.layer_name_by_number(layer);
                let routing = db.layer_get_type(&name).unwrap_or_default() == "ROUTING";
                boxes.push((is_core, supply, routing, y1 - y0));
            }
        }
    }
    followpins::determine_width(&boxes)
}

/// Every enclosure one side of a via could take.
///
/// ⚠️ **The rules' enclosures AND the generate rule's**, deduplicated — a technology may declare
/// both, and the choice is made across all of them rather than by preferring one source. Where
/// neither says anything the only candidate is zero, which is why some vias legitimately carry no
/// overhang at all.
pub(crate) fn enclosure_candidates_with_swap(
    db: &Db,
    cut_layer: &str,
    cut: (i32, i32),
    area: Rect,
    layer: &str,
    above: bool,
    // ⚠️ **`None` means the rules alone.** A generate rule states its own enclosure and that is a
    // candidate like any other; a technology-declared via has none, and `getMinimumEnclosures`
    // asks for the rule-derived set only — the via's own margins are a floor applied afterwards,
    // never a candidate. Seeded either way, that seed fits more cuts than any real rule and so
    // always wins the selection.
    from_rule: Option<(i32, i32)>,
    // 🔑 **A split array reports a width of ZERO.** `ViaGenerator::getRectSize` opens with
    // `if (!only_real && isSplitCutArray()) return 0;`, so `getLowerWidth`/`getUpperWidth` — the
    // widths the enclosure rules are bucketed against — do not describe the shape at all. The
    // narrowest bucket therefore wins, which is the point: a split array is a single cut placed
    // many times and has no business claiming the enclosure a full-width array would earn.
    split: bool,
) -> Vec<(vyges_pdn::viagen::Enclosure, bool)> {
    use vyges_pdn::viagen::{enclosure_from_rule, rect_direction, EncType, Enclosure};
    let dir = direction_of(db, layer);
    let rect_dir = rect_direction(area);
    // ⚠️ A generate rule's own enclosure is built by `Enclosure(dbTechViaLayerRule*, layer)`,
    // which calls `swap` — so it may be met in either orientation.
    let mut out: Vec<(Enclosure, bool)> = from_rule
        .map(|(x, y)| vec![(Enclosure { x, y }, true)])
        .unwrap_or_default();

    // 🔑 **A rule for another cut class is not this via's rule.** `getCutMinimumEnclosureRules`
    // opens with `if (!isCutClass(enc_rule->getCutClass())) continue;`, and ASAP7's cut layers
    // carry one rule per class — so ignoring the class admits other classes' ENDSIDE rules, which
    // state 0/0, fit more cuts than any real rule, and therefore win every selection.
    let classes = via_cut_classes(db, cut_layer);
    let mine = vyges_pdn::viagen::cut_class(&classes, cut).map(|c| c.name.clone());

    // ⚠️ **Rules are bucketed by minimum width and only ONE bucket is used** — the largest whose
    // width the shape meets. Taking every applicable rule mixes buckets that were never meant to
    // apply together.
    let mut buckets: std::collections::BTreeMap<i32, Vec<(Enclosure, bool)>> = Default::default();
    for i in 0..db.num_layer_get_tech_layer_cut_enclosure_rules(cut_layer) {
        if let Some(mine) = &mine {
            let rule_class = db.cutenclosurerule_get_cut_class(cut_layer, i);
            if !rule_class.is_empty() && &rule_class != mine {
                continue;
            }
        }
        let (is_above, is_below) = (
            db.cutenclosurerule_is_above(cut_layer, i),
            db.cutenclosurerule_is_below(cut_layer, i),
        );
        let (top, bot) = if !is_above && !is_below {
            (true, true)
        } else {
            (is_above, is_below)
        };
        if !((above && top) || (!above && bot)) {
            continue;
        }
        let kind = match db.cutenclosurerule_get_type(cut_layer, i).as_str() {
            "EOL" => EncType::Eol,
            "ENDSIDE" => EncType::EndSide,
            "HORZ_AND_VERT" => EncType::HorzAndVert,
            _ => EncType::Default,
        };
        let min_width = if db.cutenclosurerule_is_width_valid(cut_layer, i) {
            db.cutenclosurerule_get_min_width(cut_layer, i)
        } else {
            0
        };
        buckets
            .entry(min_width)
            .or_default()
            .push((
                enclosure_from_rule(
                    kind,
                    db.cutenclosurerule_get_first_overhang(cut_layer, i),
                    db.cutenclosurerule_get_second_overhang(cut_layer, i),
                    cut,
                    dir,
                    rect_dir,
                ),
                // Only a DEFAULT rule goes through `Enclosure::swap`; the others fix their axes.
                matches!(kind, EncType::Default),
            ));
    }
    // The widest bucket the shape qualifies for, walking ascending as the reference does.
    let shape_width = if split {
        0
    } else {
        (area.2 - area.0).min(area.3 - area.1)
    };
    if let Some((_, chosen)) = buckets
        .iter()
        .filter(|(w, _)| **w <= shape_width)
        .next_back()
    {
        out.extend(chosen.iter().copied());
    }
    let mut seen: Vec<(Enclosure, bool)> = Vec::new();
    for e in out {
        if !seen.iter().any(|(s, _)| *s == e.0) {
            seen.push(e);
        }
    }
    seen
}

/// The enclosure candidates alone, which is what the selection compares.
pub(crate) fn enclosure_candidates(
    db: &Db,
    cut_layer: &str,
    cut: (i32, i32),
    area: Rect,
    layer: &str,
    above: bool,
    from_rule: Option<(i32, i32)>,
    split: bool,
) -> Vec<vyges_pdn::viagen::Enclosure> {
    enclosure_candidates_with_swap(db, cut_layer, cut, area, layer, above, from_rule, split)
        .into_iter()
        .map(|(e, _)| e)
        .collect()
}

/// The routing layers a via between two layers passes through, ends included.
///
/// ⚠️ Built from the technology's layer numbering, which interleaves routing and cut layers, so
/// both are walked and only the routing ones kept.
/// The cut layer between two adjacent routing layers.
///
/// ⚠️ **Derived from the technology, not named by the caller.** A connect statement names the two
/// routing layers and says nothing about the cut between them; requiring it on the command line
/// makes every caller restate what the layer numbering already knows — and a caller that omits it
/// builds no via at all, silently, because there is nothing to report.
pub(crate) fn cut_layer_between(db: &Db, lower: &str, upper: &str) -> Option<String> {
    let all: Vec<String> = db
        .layers_with_direction()
        .unwrap_or_default()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let idx = |n: &str| all.iter().position(|x| x == n);
    let (a, b) = (idx(lower)?, idx(upper)?);
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    all[lo + 1..hi]
        .iter()
        .find(|n| db.layer_get_type(n).unwrap_or_default() == "CUT")
        .cloned()
}

/// A layer's routing level: its 1-based position among the ROUTING layers alone.
///
/// Not its position among all layers — cut layers interleave with routing ones, so the two
/// numbers diverge from metal2 upwards, and the reference names every via after this one.
pub(crate) fn routing_level(db: &Db, layer: &str) -> i32 {
    let mut level = 0;
    for (name, _) in db.layers_with_direction().unwrap_or_default() {
        if db.layer_get_type(&name).unwrap_or_default() == "ROUTING" {
            level += 1;
            if name == layer {
                return level;
            }
        }
    }
    0
}

/// Every layer as `(name, number, routing_level, has_lef58_type)`, in number order.
pub(crate) fn layers_with_numbers(db: &Db) -> Vec<(String, i32, i32, bool)> {
    db.layers_with_direction()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let routing = db.layer_get_type(&name).unwrap_or_default() == "ROUTING";
            (name, i as i32, if routing { 1 } else { 0 }, false)
        })
        .collect()
}

pub(crate) fn layer_number(layers: &[(String, i32, i32, bool)], name: &str) -> i32 {
    layers
        .iter()
        .find(|(n, ..)| n == name)
        .map(|(_, i, ..)| *i)
        .unwrap_or(0)
}

/// The cut classes a cut layer declares.
pub(crate) fn via_cut_classes(db: &Db, cut_layer: &str) -> Vec<vyges_pdn::viagen::CutClass> {
    (0..db.layer_get_tech_layer_cut_class_rules(cut_layer).len())
        .map(|i| vyges_pdn::viagen::CutClass {
            name: db.cutclassrule_get_name(cut_layer, i),
            width: db.cutclassrule_get_width(cut_layer, i),
            // ⚠️ A rule with no stated length is square, and `None` is what says so — reading
            // `get_length` regardless returns 0 and matches a cut of zero height.
            length: db
                .cutclassrule_is_length_valid(cut_layer, i)
                .then(|| db.cutclassrule_get_length(cut_layer, i)),
        })
        .collect()
}

/// The LEF58 cut-spacing-table rules a cut layer states, for one cut class.
///
/// ⚠️ **A cut layer stating no `SPACING` of its own is not a layer with no spacing** — every ASAP7
/// cut layer is in that position, and its table is the only place a pitch comes from.
pub(crate) fn spacing_table_rules(
    db: &Db,
    cut_layer: &str,
    class: &str,
) -> Vec<vyges_pdn::techvia::SpacingTableRule> {
    (0..db.num_cut_spacing_table_rules(cut_layer).unwrap_or(0))
        .map(|i| vyges_pdn::techvia::SpacingTableRule {
            has_second_layer: !db
                .cutspacingtablerule_get_second_layer(cut_layer, i)
                .is_empty(),
            same_net: db.cutspacingtablerule_is_same_net(cut_layer, i),
            centre_and_edge: db
                .cut_spacing_table_is_center_and_edge(cut_layer, i, class)
                .unwrap_or(false),
            centre_to_centre: db
                .cut_spacing_table_is_center_to_center(cut_layer, i, class)
                .unwrap_or(false),
            max_spacing: db
                .cut_spacing_table_max_spacing(cut_layer, i, class)
                .unwrap_or(0),
            // ⚠️ Crossed: the x pitch is asked with the y side flag, because a cut's dx is its
            // SIDE when dx > dy.
            sided: (
                db.cut_spacing_table_spacing(cut_layer, i, class, false, false)
                    .unwrap_or(0),
                db.cut_spacing_table_spacing(cut_layer, i, class, true, true)
                    .unwrap_or(0),
            ),
        })
        .collect()
}

/// A `-fixed_vias` name for this connect that spans exactly this level's two layers.
///
/// ⚠️ **The via must span the level, not the connect.** `M3 M6` names three vias, and each is
/// valid at one level of the stack only; matching on the connect would build VIA34 at every level.
#[allow(clippy::type_complexity)]
pub(crate) fn fixed_tech_via(
    db: &Db,
    fixed: &[(
        String,
        String,
        Vec<String>,
        Option<(i32, i32)>,
        Option<regex::Regex>,
    )],
    connect: (&str, &str),
    level: (&str, &str),
) -> Option<(String, vyges_pdn::techvia::TechVia, Option<(i32, i32)>)> {
    let (_, _, named, pitch, dont_use) = fixed
        .iter()
        .find(|(l, u, ..)| l == connect.0 && u == connect.1)?;
    // 🔑 **A connect naming no via still gets the technology's.** `populateTechVias` falls back to
    // every tech via spanning the pair, and a technology with no VIARULE GENERATE — ASAP7's
    // `via_dontuse` LEF is exactly that — has nothing else to offer. ⚠️ Gated on the caller
    // finding no generate rule for the level, so this never preempts a rule-built via.
    let all;
    let names: &[String] = if named.is_empty() {
        all = db.tech_get_vias();
        &all
    } else {
        named
    };
    for name in names {
        // ⚠️ `Connect::filterVias` erases from the TECH VIA list as well as the generate rules,
        // and it searches rather than anchors — a partial match rejects the via.
        if dont_use.as_ref().is_some_and(|d| d.is_match(name)) {
            continue;
        }
        if db.tech_via_layer(name, "bottom").unwrap_or_default() != level.0
            || db.tech_via_layer(name, "top").unwrap_or_default() != level.1
        {
            continue;
        }
        let boxes: Vec<(String, Rect)> = db
            .tech_via_boxes(name)
            .unwrap_or_default()
            .into_iter()
            .map(|(l, x0, y0, x1, y1)| (db.layer_name_by_number(l), (x0, y0, x1, y1)))
            .collect();
        let is_cut = |l: &str| db.layer_get_type(l).unwrap_or_default() == "CUT";
        if let Some(g) = vyges_pdn::techvia::geometry(&boxes, level.0, &is_cut) {
            return Some((name.clone(), g, *pitch));
        }
    }
    None
}

/// **`CoreGrid::cleanupShapes`** — the outline of every fixed instance, on each layer it occupies.
///
/// 🔑 **A core-grid shape lying wholly inside a macro is removed**, before any via is made.
///
/// ⚠️ **The INSTANCE'S OUTLINE, not the geometry that put it on the layer.** The reference keys a
/// layer off the master having an obstruction or a pin there, and then indexes the whole bounding
/// box. A macro with a hole in its metal — obstruction bands at top and bottom and nothing between
/// — still swallows a strap that threads the gap, because what is tested is the outline.
///
/// ⚠️ **Wholly, so an overhang saves a shape.** A strap one unit past the macro's edge survives
/// where its neighbour a pitch inside does not, which is exactly the difference the reference shows.
///
/// ⚠️ **Every FIXED instance**, with no filter on class: unlike `getInstanceObstructions` this does
/// not skip core masters or end caps.
pub(crate) fn macro_outlines(db: &Db) -> Vec<(String, Rect)> {
    let mut out: Vec<(String, Rect)> = Vec::new();
    for inst in db.block_get_insts() {
        if !db.inst_is_fixed(&inst) {
            continue;
        }
        let b = db.inst_bbox(&inst).unwrap_or_default();
        if b.len() != 4 {
            continue;
        }
        let outline = (b[0], b[1], b[2], b[3]);
        let master = db.inst_get_master(&inst);
        let mut layers: Vec<String> = Vec::new();
        let mut note = |layers: &mut Vec<String>, n: String| {
            if !layers.contains(&n) {
                layers.push(n);
            }
        };
        for (layer, ..) in db.master_obstruction_boxes(&master).unwrap_or_default() {
            note(&mut layers, db.layer_name_by_number(layer));
        }
        for term in db.master_get_m_terms(&master) {
            for (layer, ..) in db.mterm_pin_boxes(&master, &term).unwrap_or_default() {
                note(&mut layers, db.layer_name_by_number(layer));
            }
        }
        for l in layers {
            out.push((l, outline));
        }
    }
    out
}

/// Whether an instance is connected to any of the grid's supply nets — `InstanceGrid::isValid`.
///
/// 🔑 **An instance grid that connects to no power or ground net is not built at all.**
///
/// An invalid grid becomes a `DummyInstanceGrid`, and `getGrids(true)` — which is what
/// `buildGrids` walks — excludes `kDummy`. So it contributes no ring, no strap, and no
/// obstruction.
///
/// ⚠️ This matters most where a pattern matches far more than intended: `-instances bump_*` on a
/// flip-chip design matches every bump, signal bumps included, and only the supply ones survive.
/// Building the rest gives them rings and straps that do not exist, and blankets the design in
/// grid obstructions that cut everything they touch.
pub(crate) fn instance_has_supply(db: &Db, inst: &str, nets: &[String]) -> bool {
    let master = db.inst_get_master(inst);
    db.master_get_m_terms(&master).into_iter().any(|term| {
        let net = db.iterm_get_net(inst, &term);
        !net.is_empty() && nets.iter().any(|n| *n == net)
    })
}

/// ⛔ **MEASURED WRONG — not wired up. See the note at the end of this comment.**
///
/// The spacing the reference uses whenever it bloats a shape into an obstruction —
/// `TechLayer::getSpacing(width, length)`.
///
/// 🔑 **The table if the layer states one, the plain `SPACING` otherwise.** It is
/// `layer_->getSpacing(width, length)` under the hood, an ODB accessor that already falls back;
/// asking only for the V5.5 table returns nothing on a layer like Nangate45 metal1, which declares
/// `SPACING 0.065` and no `SPACINGTABLE`, and the shape is then not grown at all.
///
/// ⚠️ **Use this everywhere an obstruction is bloated OR a cut halo is taken.** Mixing the two
/// accessors is worse than picking either consistently: the cut extent is the bloated rect grown
/// by the cutting shape's halo, so a difference in one is indistinguishable from a difference in
/// the other, and they cancel on some designs and compound on others.
/// 🔑 **`TechLayer::getSpacing` is a MAXIMUM over two accessors, and the first of them is itself
/// four rules deep:**
///
/// where `dbTechLayer::getSpacing(width, length)` is itself the V5.4 RANGE rules, then the V5.5
/// table which overwrites them, then an over-range V5.4 rule, and only then the plain `SPACING`.
///
/// ⛔ **MEASURED AND LEFT UNWIRED, twice now, and the second measurement says where to look.**
/// An earlier version read the chain as "the V5.5 table, else the plain SPACING" and cost seven
/// designs their exact match. This one is the chain as the source states it, and across the suite
/// it **gains one design and costs three**: more than it gains.
///
/// 🔑 **All three losses are macro grids with HALOS, and the one it fixes has none.** That is the old note's own hypothesis confirmed rather than the rule being wrong:
/// `getInstanceObstructions(inst, halos_)` merges the halo rect with the spacing rect and takes the
/// LARGER per side, so where our equivalent adds one to the other instead, widening the spacing
/// term widens the error with it.
///
/// ⟹ **Fix `applyHalo` first, then measure this again.** Wiring it before that trades three cases
/// for one, in both directions, forever.
#[allow(dead_code)]
pub(crate) fn obstruction_spacing(db: &Db, layer: &str, width: i32, length: i32) -> i32 {
    let db_spacing = db.layer_get_spacing_for(layer, width, length).unwrap_or(0);
    let two_widths = db.layer_find_tw_spacing(layer, width, width, length).unwrap_or(0);
    db_spacing.max(two_widths)
}

/// What a layer permits a grid component to state, for [`vyges_pdn::validate`].
///
/// ℹ️ Read once per layer per run. Every field is a plain technology fact; the rules that read them
/// live in the library, where they can be tested without a LEF file.
pub(crate) fn layer_rules(db: &Db, layer: &str) -> Option<validate::LayerRules> {
    // ⚠️ **An unknown layer must answer `None`, never a rule set of zeroes.** Every generated
    // accessor reports a missing layer as `0`, and a maximum width of zero refuses every shape —
    // so a mistyped layer name would come back as a width violation on a layer that does not
    // exist. Naming a layer the technology does not have is its own diagnostic, raised elsewhere.
    if db.layer_get_type(layer).unwrap_or_default().is_empty() {
        return None;
    }
    Some(validate::LayerRules {
        name: layer.to_string(),
        min_width: db.layer_get_min_width(layer) as i32,
        // ⚠️ **An undeclared MAXWIDTH reads back as a very large number, not as zero** — odb's own
        // default. Clamping it here would refuse every wide strap in a technology that states no
        // maximum at all.
        max_width: db.layer_get_max_width(layer).min(i32::MAX as u32) as i32,
        direction: direction_of(db, layer),
        width_tables: db.layer_width_tables(layer).unwrap_or_default(),
        // ⚠️ The TECHNOLOGY's units, which is what the reference renders a diagnostic with
        // (`TechLayer::getLefUnits`), not the block's DEF units. They usually agree and the
        // message is wrong by a factor of the ratio when they do not.
        units_per_micron: db.tech_get_db_units_per_micron(),
        manufacturing_grid: db.manufacturing_grid().unwrap_or_default(),
    })
}

/// The minimum spacing beside a shape of `width`, as `TechLayer::getSpacing(width)` answers it.
///
/// Same rule as [`obstruction_spacing`] with a zero run length, named separately because this one
/// is a limit a stated spacing is CHECKED against rather than a keep-out geometry is built from.
pub(crate) fn min_spacing_for(db: &Db, layer: &str, width: i32) -> i32 {
    obstruction_spacing(db, layer, width, 0)
}

/// What a `kOverPads` connection needs in order to be REFINED after every component is built.
///
/// 🔑 **`PadDirectConnectionStraps::refineShapes` exists because an over-pad strap picks its slot
/// from the PAD's arithmetic and only afterwards discovers what stands between it and its target.**
/// If the via it would need is obstructed on an intermediate layer, the strap is removed and slid
/// along the pin, in manufacturing-grid steps, to the first place whose via is clear.
///
/// ⚠️ **A strap that finds nowhere legal is simply gone** — the removal happens first and there is
/// no path that puts it back.
///
/// ⚠️ **Only a shape that survived its cut UNCHANGED can be refined.** `replaceShape` carries a
/// shape's VIAS onto its pieces and nothing else, so a cut strap is absent from `target_shapes_`
/// and `strapViaIsObstructed` returns false for it without ever looking.
#[derive(Clone)]
pub(crate) struct OverPadStrap {
    pub net: String,
    pub layer: String,
    /// The strap as built, before any cut — the key that says whether it survived one.
    pub strap: Rect,
    pub target_layer: String,
    pub target: Rect,
    /// `org_pin_shape`: the terminal's merged pin rectangle in placed coordinates, which is the
    /// range the refined strap may slide within.
    pub pin: Rect,
    pub width: i32,
    pub horizontal: bool,
    /// The pad's own outline, for the over-pads half of `cutShapes` during a refine.
    pub inst: Rect,
}

/// One pad direct connection — a `PadDirectConnectionStraps` component.
///
/// 🔑 **One per (instance, terminal), and only where the terminal has pins FACING THE CORE.**
/// `setupDirectConnect` builds each component from `getPinsFacingCore`, so a pad whose supply pin
/// points outward gets none at all. That test needs the pin geometry, which is why this runs up
/// front rather than as a side effect of building straps — the reference does the same, at
/// `define_pdn_grid` time, because the obstruction set depends on the answer.
pub(crate) struct PadConnection {
    pub inst: String,
    /// The terminal this component belongs to. ⚠️ Part of its identity, not decoration: the
    /// reference names each component `<instance>/<terminal>`, and a pad with both a VDD and a VSS
    /// pin is two components.
    pub term: String,
    pub net: String,
    pub edge: vyges_pdn::pads::Edge,
    pub inst_rect: Rect,
    /// Every ROUTING pin of the terminal on a named layer. ⚠️ The strap's hold is taken from all
    /// of these, not only the ones facing the core.
    pub pins: Vec<vyges_pdn::pads::Pin>,
    /// The subset facing the core — the starting points, and the reason the component exists.
    pub facing: Vec<vyges_pdn::pads::Pin>,
    /// 🔑 **The key `getAssociatedStraps` sorts a pad's connections by**, and it is in MASTER
    /// coordinates, not placed ones: `getPinsByLayer` collects `mpin->getGeometry()` boxes and the
    /// sort compares those directly, while `makeShapesOverPads` applies the instance transform
    /// only afterwards — `transform.apply(pin_shape)`.
    ///
    /// ⚠️ Sorted on placed rects instead, a pad whose orientation reverses an axis orders its two
    /// connections the other way round: every slot coordinate correct, every one on the wrong net.
    pub sort_key: Rect,
    /// 🔑 **Set only for an OVER-PADS connection**, the fallback when nothing faces the core:
    /// `initialize` tries `getPinsFacingCore` and reaches for `getPinsFormingRing` only when that
    /// comes back empty. Carries the layer the strap runs on, over the pad rather than inward.
    pub over_pads: Option<String>,
}

/// The two refusals of `getPinsFormingRing`, against the master's own geometry.
///
/// ⚠️ Everything of the master counts — its obstructions AND every terminal's pins — since a
/// neighbouring pin above the strap's layer obstructs it just as an OBS box does.
fn may_run_over(
    db: &Db,
    master: &str,
    ring: &[vyges_pdn::pads::Pin],
    at: usize,
    routing: &[String],
) -> bool {
    let index_of = |layer: i64| -> Option<usize> {
        let name = db.layer_name_by_number(layer);
        routing.iter().position(|l| *l == name)
    };
    let obstructions: Vec<(usize, Rect)> = db
        .master_obstruction_boxes(master)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(l, x0, y0, x1, y1)| index_of(l).map(|i| (i, (x0, y0, x1, y1))))
        .collect();
    let geometry: Vec<(usize, Rect)> = db
        .master_get_m_terms(master)
        .iter()
        .flat_map(|t| db.mterm_pin_boxes(master, t).unwrap_or_default())
        .filter_map(|(l, x0, y0, x1, y1)| index_of(l).map(|i| (i, (x0, y0, x1, y1))))
        .collect();
    vyges_pdn::pads::may_run_over_pad(at, ring, &obstructions, &geometry)
}
/// The inner edge of the pad ring, which is what `-pad_offsets` is measured from.
///
/// 🔑 **`Rings::setPadOffset` walks the placed pads and pulls the die area inward**, one side at a
/// time, to the nearest pad edge facing the core:
///
/// - only masters whose type `isPad()` count, and ⚠️ **`PAD_AREAIO` is excluded** — an area-IO pad
///   sits over the core rather than around it and would collapse the boundary onto nothing;
/// - a pad belongs to a side only if it clears the core on that axis AND lies within the core's
///   extent on the other, so a corner pad — outside the core on both axes — is claimed by neither
///   and never sets a bound;
/// - the result starts as the DIE and only ever shrinks, so a side with no pad keeps the die edge.
///
/// ⚠️ **A design with no pads at all answers the core**, and the reference treats that as a
/// failure rather than a zero offset: `PDN-0105`, and the die is used instead.
pub(crate) fn pad_ring_inner(db: &Db, core: Rect, die: Rect) -> Rect {
    let mut inner = die;
    for inst in db.block_get_insts() {
        if !db.inst_is_placed(&inst) {
            continue;
        }
        let master = db.inst_get_master(&inst);
        if !db.master_is_pad(&master) {
            continue;
        }
        // ⚠️ **The string is not the enum name.** `dbMasterType::PAD_AREAIO` prints as
        // `"PAD AREAIO"` — `dbTypes.cpp` parses `"PAD AREAIO"` and returns the same with a space —
        // so a comparison against the underscored spelling silently matches nothing. This design's
        // 32 area-IO pads sit further in than the pad ring, so failing to exclude them pulls the
        // boundary onto them and every `-pad_offsets` ring lands tens of microns off.
        if db.master_get_type(&master).unwrap_or_default() == "PAD AREAIO" {
            continue;
        }
        let b = db.inst_bbox(&inst).unwrap_or_default();
        if b.len() != 4 {
            continue;
        }
        let (x0, y0, x1, y1) = (b[0], b[1], b[2], b[3]);
        let within_x = x0 >= core.0 && x1 <= core.2;
        let within_y = y0 >= core.1 && y1 <= core.3;
        if y0 > core.3 && within_x {
            inner.3 = inner.3.min(y0);
        } else if y1 < core.1 && within_x {
            inner.1 = inner.1.max(y1);
        } else if x1 < core.0 && within_y {
            inner.0 = inner.0.max(x1);
        } else if x0 > core.2 && within_y {
            inner.2 = inner.2.min(x0);
        }
    }
    if inner == core {
        eprintln!(
            "vyges-pdn: unable to determine the location of the pad offset, \
             using the die boundary instead"
        );
        return die;
    }
    inner
}


/// Every pad direct connection the grids will hold — `setupDirectConnect`, without building
/// anything.
///
/// ⚠️ **PLACED instances whose MASTER is a pad**, and nothing else: an unplaced pad has no
/// coordinates to reach from.
pub(crate) fn pad_connections(
    db: &Db,
    nets: &[String],
    layers: &[String],
    core: Rect,
) -> Vec<PadConnection> {
    let mut out = Vec::new();
    for inst in db.block_get_insts() {
        if !db.inst_is_placed(&inst) {
            continue;
        }
        let master = db.inst_get_master(&inst);
        if !db.master_is_pad(&master) {
            continue;
        }
        let b = db.inst_bbox(&inst).unwrap_or_default();
        if b.len() != 4 {
            continue;
        }
        let inst_rect = (b[0], b[1], b[2], b[3]);
        let Some(edge) = vyges_pdn::pads::pad_edge(inst_rect, core) else {
            continue; // no edge, or two of them: the reference refuses the pad
        };
        let orient = db.inst_get_orient(&inst);
        let master_size = (
            db.master_get_width(&master) as i32,
            db.master_get_height(&master) as i32,
        );
        for term in db.master_get_m_terms(&master) {
            let net = db.iterm_get_net(&inst, &term);
            if net.is_empty() || !nets.iter().any(|n| *n == net) {
                continue;
            }
            // ⚠️ **ROUTING layers only**, and filtered to the layers the flag named where it did.
            // ⚠️ The master-coordinate boxes, kept for the sort key alone.
            let raw: Vec<Rect> = db
                .mterm_pin_boxes(&master, &term)
                .unwrap_or_default()
                .into_iter()
                .filter(|(l, ..)| {
                    let name = db.layer_name_by_number(*l);
                    db.layer_get_type(&name).unwrap_or_default() == "ROUTING"
                        && (layers.is_empty() || layers.iter().any(|k| *k == name))
                })
                .map(|(_, x0, y0, x1, y1)| (x0, y0, x1, y1))
                .collect();
            let pins: Vec<vyges_pdn::pads::Pin> = db
                .mterm_pin_boxes(&master, &term)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(l, x0, y0, x1, y1)| {
                    let name = db.layer_name_by_number(l);
                    if db.layer_get_type(&name).unwrap_or_default() != "ROUTING" {
                        return None;
                    }
                    if !layers.is_empty() && !layers.iter().any(|k| *k == name) {
                        return None;
                    }
                    let direction = direction_of(db, &name);
                    Some(vyges_pdn::pads::Pin {
                        // 🔑 **`place_in_bbox`, not `transform_rect` with an origin.** Rotating a
                        // master about the origin sends its geometry negative, and the instance
                        // origin that would put it back is not the bbox corner for every
                        // orientation. Shifting the transformed OUTLINE onto the instance's bbox
                        // is orientation-independent and cannot land a pin outside its own pad.
                        //
                        // ⚠️ It looked right on the east pads by luck — their numbers were
                        // plausible without being inside the instance. On the west pads it put
                        // every pin at a NEGATIVE x, off the die, so none of them faced the core
                        // and that whole edge connected to nothing.
                        rect: vyges_pdn::orient::place_in_bbox(
                            (x0, y0, x1, y1),
                            &orient,
                            master_size,
                            (inst_rect.0, inst_rect.1),
                        ),
                        layer: name,
                        direction,
                    })
                })
                .collect();
            let facing = vyges_pdn::pads::pins_facing_core(&pins, inst_rect, edge);
            // ⚠️ **A FALLBACK, tried only when nothing faces the core.** Reaching for it first
            // would rebuild every ordinary pad connection as an over-pad one.
            let (facing, over_pads) = if facing.is_empty() {
                let routing: Vec<String> = db
                    .layers_with_direction()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(n, _)| n)
                    .filter(|n| db.layer_get_type(n).unwrap_or_default() == "ROUTING")
                    .collect();
                let master_size = (
                    db.master_get_width(&master) as i32,
                    db.master_get_height(&master) as i32,
                );
                match vyges_pdn::pads::pins_forming_ring(&pins, master_size, &routing) {
                    Some((ring, at)) if may_run_over(db, &master, &ring, at, &routing) => {
                        (ring, Some(routing[at].clone()))
                    }
                    _ => continue, // no component at all, which is what the reference makes
                }
            } else {
                (facing, None)
            };
            out.push(PadConnection {
                inst: inst.clone(),
                term: term.clone(),
                net,
                edge,
                inst_rect,
                pins,
                facing,
                over_pads,
                sort_key: raw
                    .iter()
                    .copied()
                    .min()
                    .unwrap_or((i32::MAX, i32::MAX, i32::MAX, i32::MAX)),
            });
        }
    }
    out
}

/// The pad instances a `-connect_to_pads` grid holds a direct connection for.
///
/// 🔑 **These contribute no obstruction at all.** `Grid::getInstances()` collects exactly the
/// instances carrying a `kPadConnect` component, `PdnGen::buildGrids` unions them across every
/// grid, and `makeInitialObstructions` takes that as `skip_insts`.
///
/// So a connected pad's OBS boxes and its pins are both invisible — a ring or a strap crossing the
/// pad is not cut by it. Treating a connected pad as an ordinary obstruction breaks a top-metal
/// ring into fragments at every pad it passes, and trimming then deletes the short ones.
///
/// ⚠️ **Membership is having the component, not having built anything.** A pad whose connection
/// produces no shape is still in the set, so this must not depend on the geometry succeeding.
/// ⚠️ But it DOES depend on a terminal having pins that face the core: testing merely for a supply
/// pin on a routing layer makes the set about twice too large, and a ring is then left UNDER-cut
/// instead of over-cut.
pub(crate) fn pad_connect_insts(
    db: &Db,
    nets: &[String],
    layers: &[String],
    core: Rect,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in pad_connections(db, nets, layers, core) {
        if !out.contains(&c.inst) {
            out.push(c.inst);
        }
    }
    out
}

/// **`CoreGrid::setupDirectConnect`** — a strap from every pad terminal to what it can reach.
///
/// One component per (net, pad instance, terminal): the pad's edge decides the direction, its
/// inner-edge pins are the starting points, and the nearest ring or stripe of the same net on a
/// connectable layer is the target.
///
/// ⚠️ **PLACED instances whose MASTER is a pad**, and nothing else — an unplaced pad has no
/// coordinates to reach from.
///
/// 🔑 **What counts as a target depends on WHERE `-connect_to_pads` was declared.** `isTargetShape`
/// answers STRIPE-or-RING by default, but `PdnGen::makeRing` overrides it for every pad strap it
/// creates.
///
/// ⚠️ So a flag on `add_pdn_ring` means **rings only** — a pad reaches the ring and stops. Allowing
/// stripes as well sends the strap past the ring to whatever lies further in, and the two overlapping
/// straps then merge into one that runs far too deep.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pad_strap_for(
    db: &Db,
    conn: &PadConnection,
    reaches: &dyn Fn(&str) -> Vec<String>,
    standing: &[(String, String, Rect, &'static str)],
    ring_only: bool,
    _core: Rect,
    die: Rect,
    // ⚠️ Which of this PAD's over-pad connections this is, and how many there are — they share
    // the pad's width, so one cannot be placed without knowing the others.
    // `(lane index, connections on this pad, explicit lane offset)`. The third is `Some` while
    // `buildOverPad` walks the pad looking for a lane that survives its cut.
    over_pad_slot: (usize, usize, Option<i32>),
    // Filled for an over-pads connection: what a later refine pass needs. See [`OverPadStrap`].
    refine: &mut Option<OverPadStrap>,
) -> Vec<(String, String, Rect, Rect)> {
    let mut out: Vec<(String, String, Rect, Rect)> = Vec::new();
    {
        let PadConnection {
            inst,
            term,
            net,
            edge,
            inst_rect,
            pins,
            facing,
            over_pads,
            ..
        } = conn;
        let (net, edge) = (net.clone(), *edge);
        {
            // 🔑 **One component per terminal.** `addShape` merges within a component, so two
            // pins of THIS terminal that line up on an axis become one strap — and two different
            // pads never join. Collected per layer here and collapsed below.
            let mut component: Vec<(String, Rect)> = Vec::new();
            let mut over_pins: Vec<vyges_pdn::pads::Pin> = Vec::new();
            // `org_pin_shape` — the merged pin rectangle before the slot replaces its across axis.
            let mut pin_extent: Rect = (0, 0, 0, 0);
            if let Some(layer) = over_pads {
                let ring = facing
                    .iter()
                    .map(|p| p.rect)
                    .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
                let (index, on_pad, offset_override) = over_pad_slot;
                if let Some(ring) = ring {
                    let horizontal =
                        matches!(edge, vyges_pdn::pads::Edge::East | vyges_pdn::pads::Edge::West);
                    let min_w = db.layer_get_min_width(layer) as i32;
                    let max_w = {
                        let m = db.layer_get_max_width(layer) as i32;
                        if m > 0 { m } else { i32::MAX }
                    };
                    let lay_sp = db.layer_get_spacing(layer);
                    let mfg = db.manufacturing_grid().unwrap_or_default().unwrap_or(1);
                    // ⛔ **An explicit offset still has to pass `computeOverPadLanes`.** The width
                    // is the pad's shared one either way, and a pad whose group is too large to
                    // fit builds nothing at any offset — `buildOverPad` is never reached, because
                    // `buildGroup` returns before it.
                    let slot_here = match offset_override {
                        Some(off) => vyges_pdn::pads::over_pad_lanes(
                            on_pad, *inst_rect, horizontal, min_w, max_w, lay_sp, mfg,
                        )
                        .map(|(w, _)| vyges_pdn::pads::slot_at_offset(ring, horizontal, off, w)),
                        None => vyges_pdn::pads::over_pad_strap(
                            ring, *inst_rect, horizontal, index, on_pad, min_w, max_w, lay_sp, mfg,
                        ),
                    };
                    if let Some(slot) = slot_here {
                        // The reference prints this as `Connecting using shape:`; keyed the way it
                        // names the connection so the two can be joined per connection.
                        if std::env::var_os("PDN_TRACE").is_some() {
                            eprintln!(
                                "[padslot] {inst}/{term}|{index}|{on_pad}|{},{},{},{}|{},{},{},{}",
                                slot.0, slot.1, slot.2, slot.3,
                                conn.sort_key.0, conn.sort_key.1,
                                conn.sort_key.2, conn.sort_key.3
                            );
                        }
                        // 🔑 **The slot is where the strap STARTS.** `makeShapesOverPads` ends in
                        // the same `getClosestShape` and `snapRectToClosestShape` an edge
                        // connection uses, so the slot reaches a target exactly as a pin does.
                        pin_extent = ring;
                        over_pins = vec![vyges_pdn::pads::Pin {
                            layer: layer.clone(),
                            rect: slot,
                            direction: direction_of(db, layer),
                        }];
                    }
                }
            }
            let starting: &[vyges_pdn::pads::Pin] =
                if over_pins.is_empty() { facing } else { &over_pins };
            // 🔑 **An over-pad connection makes ONE shape; an edge connection makes one per
            // layer it can reach.** The two paths differ here and nowhere else.
            //
            // ⚠️ **The over-pad loop keeps the LAST layer that answers, not the nearest of them.**
            // `Shape::ShapeTreeMap` is an `odb::PtrMap` ordered by database id, so the last is the
            // topmost layer carrying a target — and that choice decides where the strap ends.
            //
            // ⚠️ And it searches EVERY layer in the map. `connectableLayers` gates the edge path
            // alone; over the pad, a target on a layer no connect names is still taken.
            let candidates_on = |target_layer: &str| -> Vec<Rect> {
                standing
                    .iter()
                    .filter(|(n, l, _, kind)| {
                        *n == net
                            && l == target_layer
                            && if ring_only {
                                *kind == "RING"
                            } else {
                                matches!(*kind, "STRIPE" | "RING")
                            }
                    })
                    .map(|(_, _, r, _)| *r)
                    .collect()
            };
            let mut standing_layers: Vec<String> = Vec::new();
            for (_, l, _, _) in standing {
                if !standing_layers.contains(l) {
                    standing_layers.push(l.clone());
                }
            }
            standing_layers.sort_by_key(|l| routing_level(db, l));
            // ⚠️ **The over-pad rules follow the SLOT, not the flag.** `over_pads` says the pins
            // form a ring and a strap may run over them; `over_pins` says one was actually placed.
            // Where the slot came out narrower than the layer's minimum the reference builds
            // nothing at all, and this engine falls back to the edge path — so keying the
            // one-shape rule on the flag applied it to straps built from the pins facing the core,
            // and `pads_connect_from_non_pref_edge` grew ten Metal5 stubs that belong to neither
            // form. ℹ️ Whether that fallback should exist at all is a separate question, recorded
            // in the audit: the reference returns early and leaves the component empty.
            let over_pad_form = !over_pins.is_empty();
            for pin in starting {
                // Over the pad: the last layer that answers, and nothing else. Facing the core:
                // every connectable layer, each its own strap.
                let searched: Vec<String> = if over_pad_form {
                    standing_layers.clone()
                } else {
                    reaches(&pin.layer)
                };
                let mut chosen: Vec<(String, Rect)> = Vec::new();
                for target_layer in searched {
                    let Some(target) = vyges_pdn::pads::closest_target(
                        pin.rect,
                        edge,
                        die,
                        &candidates_on(&target_layer),
                    ) else {
                        continue;
                    };
                    if over_pad_form {
                        chosen.clear();
                    }
                    chosen.push((target_layer, target));
                }
                for (target_layer, target) in chosen {
                    // The reference's `Pad` group at 3 prints exactly this decision:
                    //   Connect iterm <inst>/<term> (<pin layer>/<box>) -> <net> (rect) on <layer>
                    // Emitted in the same shape so the chosen targets join on the iterm.
                    if std::env::var_os("PDN_TRACE").is_some() {
                        eprintln!(
                            "[padtarget] {inst}/{term}|{}|{target_layer}|{},{},{},{}",
                            pin.layer, target.0, target.1, target.2, target.3
                        );
                    }
                    // ⚠️ Zero means the layer states no maximum, not a maximum of nothing.
                    let max_width = Some(db.layer_get_max_width(&pin.layer) as i32).filter(|w| *w > 0);
                    let strap = vyges_pdn::pads::strap_to_shape(pin.rect, target, edge, max_width);
                    if over_pad_form {
                        *refine = Some(OverPadStrap {
                            net: net.clone(),
                            layer: pin.layer.clone(),
                            strap,
                            target_layer: target_layer.clone(),
                            target,
                            // 🔑 **The ORIGINAL pin rectangle, not the slot.** `target_pin_shape_`
                            // records `org_pin_shape`, taken before the across axis is replaced by
                            // the slot — so a refine may slide anywhere over the pin, not merely
                            // within the width the slot happened to get.
                            pin: pin_extent,
                            width: if edge.is_horizontal() {
                                pin.rect.3 - pin.rect.1
                            } else {
                                pin.rect.2 - pin.rect.0
                            },
                            horizontal: edge.is_horizontal(),
                            inst: *inst_rect,
                        });
                    }
                    // ℹ️ The terminal hold is taken after the merge below, from whichever pins
                    // end up inside the surviving strap.
                    component.push((pin.layer.clone(), strap));
                }
            }
            // ⚠️ Per LAYER: `shapes_` is keyed by layer, so straps on different layers never see
            // one another however they overlap.
            let mut layers_seen: Vec<String> = Vec::new();
            for (l, _) in &component {
                if !layers_seen.contains(l) {
                    layers_seen.push(l.clone());
                }
            }
            for l in layers_seen {
                let on_layer: Vec<(String, Rect)> = component
                    .iter()
                    .filter(|(cl, _)| *cl == l)
                    .map(|(_, r)| (net.clone(), *r))
                    .collect();
                for (_, rect) in vyges_pdn::shapes::add_shapes(&on_layer) {
                    // The strap is held wherever its own pin lies inside it.
                    let hold = pins
                        .iter()
                        .map(|p| p.rect)
                        .filter(|p| overlaps(*p, rect))
                        .fold(None::<Rect>, |acc, p| {
                            let clipped = (
                                p.0.max(rect.0),
                                p.1.max(rect.1),
                                p.2.min(rect.2),
                                p.3.min(rect.3),
                            );
                            Some(match acc {
                                None => clipped,
                                Some(a) => vyges_pdn::shapes::merge(a, clipped),
                            })
                        })
                        .unwrap_or(rect);
                    out.push((net.clone(), l.clone(), rect, hold));
                }
            }
        }
        // The per-component tally the reference's `Make` group prints as `cut=X->Y`, keyed the
        // way it names components, so the two lists diff directly.
        if std::env::var_os("PDN_TRACE").is_some() {
            eprintln!("[pad] {inst}/{term}|{}", out.len());
            // The shapes themselves, to join against the `+ <net> (rect) on <layer>` lines the
            // reference's `Shape` group prints under each component.
            for (_, layer, r, _) in &out {
                eprintln!(
                    "[padshape] {inst}/{term}|{layer}|{},{},{},{}",
                    r.0, r.1, r.2, r.3
                );
            }
        }
    }
    out
}

pub(crate) fn stack_layers(db: &Db, lower: &str, upper: &str) -> Vec<String> {
    let all: Vec<(String, i32, i32, bool)> = db
        .layers_with_direction()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let routing = db.layer_get_type(&name).unwrap_or_default() == "ROUTING";
            (name, i as i32, if routing { 1 } else { 0 }, false)
        })
        .collect();
    let num = |n: &str| {
        all.iter()
            .find(|(x, ..)| x == n)
            .map(|(_, i, ..)| *i)
            .unwrap_or(0)
    };
    let mut out = vec![lower.to_string()];
    out.extend(vyges_pdn::vias::intermediate_routing(
        &all,
        num(lower),
        num(upper),
    ));
    out.push(upper.to_string());
    out
}

/// One `VIARULE GENERATE`, reduced to what building a via from it needs.
pub(crate) struct ViaRule {
    /// ⚠️ Passed through when the via is created: with it odb writes a parameterised
    /// `+ VIARULE ... + CUTSIZE ... + ROWCOL ...`, and without it a list of explicit rectangles.
    /// The two describe the same metal and do not compare equal.
    pub(crate) name: String,
    pub(crate) lower: String,
    pub(crate) upper: String,
    pub(crate) cut: (i32, i32),
    /// Centre-to-centre, from the cut size and the rule's declared spacing.
    pub(crate) pitch: (i32, i32),
    pub(crate) bottom_enclosure: (i32, i32),
    pub(crate) top_enclosure: (i32, i32),
    /// ⚠️ **`None` means the rule states no range and applies at any width** — see
    /// [`vyges_pdn::viagen::rule_valid_for_width`].
    pub(crate) bottom_width: Option<(i32, i32)>,
    pub(crate) top_width: Option<(i32, i32)>,
    /// The cut layer this rule builds on.
    pub(crate) cut_layer: String,
}

/// The instances a `-macro` grid claims — `define_pdn_grid`'s `-instances`, `-cells` and `-default`.
///
/// ⚠️ **Both are unanchored REGEX searches**, not names and not globs: `get_insts` and
/// `get_masters` call Tcl's `regexp`, which searches. `-default` is `-cells ".*"`.
///
/// Two selections with different rules:
///
/// - **by instance**: every instance whose name matches, ⚠️ **fixed ones only** — a placed but
///   unfixed instance is warned about and skipped — then sorted unique by name;
/// - **by cell**: every MASTER whose name matches and ⚠️ **which is a BLOCK** — a standard cell
///   matching the pattern is passed over, which is what makes `-default`'s `.*` mean "every macro"
///   rather than "everything" — then every instance of those masters, cells in name order.
///
/// 🔑 **The order is the grid order**, and grids are built in order with each seeing what the ones
/// before it made. Two macros swapped here are two different answers.
pub(crate) fn select_instances(
    db: &Db,
    by_cell: bool,
    patterns: &[String],
    orients: &[String],
) -> Vec<String> {
    // ⚠️ An empty list matches everything; otherwise the match is EXACT, against odb's spelling.
    let oriented = |i: &String| {
        orients.is_empty()
            || orients
                .iter()
                .any(|o| vyges_pdn::orient::canonical(o) == db.inst_get_orient(i))
    };
    let matches = |pat: &str, name: &str| {
        regex::Regex::new(pat).map(|r| r.is_match(name)).unwrap_or(false)
    };
    let insts = db.block_get_insts();
    if !by_cell {
        let mut out: Vec<String> = insts
            .into_iter()
            .filter(|i| {
                db.inst_is_fixed(i) && patterns.iter().any(|p| matches(p, i)) && oriented(i)
            })
            .collect();
        out.sort();
        out.dedup();
        return out;
    }
    // Masters first, in name order, then their instances.
    let mut masters: Vec<String> = insts
        .iter()
        .map(|i| db.inst_get_master(i))
        .filter(|m| db.master_is_block(m) && patterns.iter().any(|p| matches(p, m)))
        .collect();
    masters.sort();
    masters.dedup();
    let mut out = Vec::new();
    for m in &masters {
        for i in &insts {
            if &db.inst_get_master(i) == m && oriented(i) {
                out.push(i.clone());
            }
        }
    }
    out
}

/// **Stage 3, in part** — the obstructions a fixed non-core instance puts on the grid.
///
/// `Grid::makeInitialObstructions` walks every FIXED instance and keeps the ones that are not core
/// cells. ⚠️ **An end cap is skipped unless it is a pad CORNER** — a standard-cell endcap sits in
/// the rows and obstructs nothing, and treating the two alike blocks every row end in the design.
///
/// Each of the master's obstruction boxes is then bloated by the layer's **flat** spacing —
/// `layer->getSpacing()`, not the width-dependent lookup used elsewhere — and transformed to where
/// the instance sits.
///
/// 🔑 **What this is for is not only cutting.** A macro obstruction is what refuses `extendTo` in
/// `repairVias`: without it a follow pin whose via reaches a strap beyond the macro is extended
/// straight through the macro, and the row it belongs to says nothing about it because the rows are
/// already split around the macro — so a rail runs far past where it belongs, at whichever macro
/// edge a strap sits close enough to pull on.
///
/// ℹ️ The master's PIN geometry is a second source the reference also collects here. Not built:
/// no case in the suite has turned on it yet, and it needs `generateObstruction` per pin shape.
/// 🔑 **Each obstruction is tagged with the instance it came from**, because an instance with a
/// grid of its own must not obstruct THAT grid. `buildGrids` hands `makeInitialObstructions` the
/// set of gridded instances as `skip_insts` and then puts their obstructions back through
/// `getGridLevelObstructions` — as shapes belonging to that grid, which its own cuts and extensions
/// ignore while every other grid still sees them.
///
/// ⚠️ Applied to everyone, a macro obstructs ITSELF: its metal blanket covers its own supply pins,
/// every via onto them is refused, and the straps laid across it are trimmed back to whatever
/// crosses at the edges. Skipped for everyone instead, the macro stops cutting its neighbours'
/// straps, which is just as wrong the other way.
/// The obstructions an instance contributes — `getInstanceObstructions`, which is its OBS boxes
/// and its PINS both.
///
/// 🔑 **Only the pin-derived ones carry a net**, and that is what decides whether a same-net shape
/// crossing them is cut. `getInstancePins` builds them as `Shape(layer, net, rect)` and
/// `getInstanceObstructions` then flips them to `kBlockObs`, which is exactly the pair
/// `Shape::cut`'s exemption tests for: same net, and a type other than `kShape`. A macro's OBS
/// boxes belong to no net and always cut.
pub(crate) fn instance_obstructions(
    db: &Db,
    halos: &[(String, [i32; 4])],
    // `(instance, layer, bloated rect, net, RAW rect)`.
    //
    // ⚠️ **The raw rect is carried for INSPECTION, not yet used.** `Shape::cut` measures its extent
    // from an obstruction's raw rect grown by the larger of the two halos, so an obstruction whose
    // two rects are the same reads as having no halo of its own — and every instance obstruction
    // here is pushed into `blockages` with the bloated rect in both fields. Whether that is wrong
    // is the open question; printing the pair is how it gets answered.
) -> Vec<(String, String, Rect, Option<String>, Rect)> {
    let mut out = Vec::new();
    for inst in db.block_get_insts() {
        if !db.inst_is_fixed(&inst) {
            continue;
        }
        let master = db.inst_get_master(&inst);
        if db.master_is_core(&master) {
            continue;
        }
        // 🔑 **A pad CORNER is kept; a standard-cell endcap is not.** The reference switches on
        // the four corner types and `continue`s on everything else — "Master is a pad corner"
        // against "Master is a std cell endcap".
        //
        // ⚠️ **The strings carry a SPACE, not an underscore.** `dbMasterType::getString()` returns
        // the LEF `CLASS` spelling — `"ENDCAP TOPLEFT"`, `"PAD AREAIO"`, `"CORE SPACER"` — and the
        // underscore belongs to the C++ enum identifier alone. Compared against the enum spelling
        // this matched nothing, so every endcap was skipped and a pad corner's obstructions were
        // never collected. Found by making the same mistake a second time, on `PAD AREAIO`.
        if db.master_is_end_cap(&master)
            && !matches!(
                db.master_get_type(&master).unwrap_or_default().as_str(),
                "ENDCAP TOPLEFT" | "ENDCAP TOPRIGHT" | "ENDCAP BOTTOMLEFT" | "ENDCAP BOTTOMRIGHT"
            )
        {
            continue;
        }
        let orient = db.inst_get_orient(&inst);
        let offset = (db.inst_get_origin_x(&inst), db.inst_get_origin_y(&inst));
        // 🔑 **The halo belongs to the GRID that claims the instance**, and reaches the obstruction
        // only through it: `makeInitialObstructions` calls `getInstanceObstructions(inst)` with a
        // zero halo, while `InstanceGrid::getGridLevelObstructions` calls it with `halos_`. An
        // instance nobody grids is therefore bloated by its spacing alone.
        //
        // ⚠️ **The larger of the two per side, not their sum**: `applyHalo` is merged with the
        // spacing rect, so a halo smaller than the spacing changes nothing.
        //
        // ⚠️ And it is what makes a macro obstruct a strap it does not touch — a 2 um halo reaches
        // 4000 units past the macro, and the strap 2080 away from it is cut after all.
        let halo = halos
            .iter()
            .find(|(i, _)| *i == inst)
            .map(|(_, h)| *h)
            .unwrap_or([0; 4]);
        for (layer, x0, y0, x1, y1) in db.master_obstruction_boxes(&master).unwrap_or_default() {
            let name = db.layer_name_by_number(layer);
            // ◐ **The layer's plain spacing, NOT its table.** `generateObstruction` bloats by
            // the spacing the technology states, which for a layer declaring only a
            // `SPACINGTABLE PARALLELRUNLENGTH` — Nangate45 metal9 — is more than this gives.
            //
            // ⚠️ **Measured: indexing the table here instead costs four designs.** So the
            // reference is not simply applying the table to a master's OBS boxes, whatever
            // `generateObstruction` reads like, and the difference is not the small under-bloat it
            // appears to be. Left alone until what it does apply is known rather than guessed.
            let s = db.layer_get_spacing(&name);
            let bloated = (
                x0 - s.max(halo[0]),
                y0 - s.max(halo[1]),
                x1 + s.max(halo[2]),
                y1 + s.max(halo[3]),
            );
            out.push((
                inst.clone(),
                name,
                vyges_pdn::orient::transform_rect(bloated, &orient, offset),
                None,
                vyges_pdn::orient::transform_rect((x0, y0, x1, y1), &orient, offset),
            ));
        }
        // 🔑 **An instance obstructs through its PINS as well as its OBS**, and the second half is
        // the larger one on a macro that declares no obstruction layer at all.
        //
        // ⚠️ **Every terminal, not only the supply ones.** `getInstancePins` walks all the
        // instance's iterms; a signal pin keeps a strap off just as a power pin does.
        //
        // ⚠️ **The halo goes on the axes matching the LAYER'S OWN direction**, not on both — a
        // horizontal layer takes it in x and a vertical one in y. The obstruction rect is bloated
        // by the pin's own spacing first, and the halo may only grow it (`rect_is_min`).
        for term in db.master_get_m_terms(&master) {
            for (layer, x0, y0, x1, y1) in db.mterm_pin_boxes(&master, &term).unwrap_or_default() {
                let name = db.layer_name_by_number(layer);
                if db.layer_get_type(&name).unwrap_or_default() != "ROUTING" {
                    continue;
                }
                let raw = (x0, y0, x1, y1);
                let (w, len) = (
                    (raw.2 - raw.0).min(raw.3 - raw.1),
                    (raw.2 - raw.0).max(raw.3 - raw.1),
                );
                let s = obstruction_spacing(db, &name, w, len);
                let obs = (raw.0 - s, raw.1 - s, raw.2 + s, raw.3 + s);
                let dir = direction_of(db, &name);
                let haloed = (
                    if dir == Direction::Horizontal { obs.0 - halo[0] } else { obs.0 },
                    if dir == Direction::Vertical { obs.1 - halo[1] } else { obs.1 },
                    if dir == Direction::Horizontal { obs.2 + halo[2] } else { obs.2 },
                    if dir == Direction::Vertical { obs.3 + halo[3] } else { obs.3 },
                );
                out.push((
                    inst.clone(),
                    name,
                    vyges_pdn::orient::transform_rect(haloed, &orient, offset),
                    // ⚠️ An unconnected terminal answers with an empty name, which must not be
                    // read as a net — every such pin would then share one and exempt each other.
                    Some(db.iterm_get_net(&inst, &term)).filter(|n| !n.is_empty()),
                    vyges_pdn::orient::transform_rect(raw, &orient, offset),
                ));
            }
        }
    }
    out
}

/// **The width a via NEEDS on a layer** — `Connect::getMinWidth`.
///
/// 🔑 **Not the layer's minimum width.** It is that plus **twice the worst enclosure** the cut
/// layers on either side ask for, so that a via actually fits rather than merely the metal.
///
/// The worst enclosure is the largest overhang any of that cut layer's enclosure rules states, and
/// the largest any generate rule touching it declares.
///
/// ℹ️ The reference also consults single-box tech vias — `(max_size - max_via_size - min_width) / 2`
/// — which is not built here; every technology in the suite that tapers states generate rules.
pub(crate) fn via_min_width(db: &Db, layer: &str, rules: &[ViaRule]) -> i32 {
    let min_width = db.layer_get_min_width(layer) as i32;
    let all: Vec<String> = db
        .layers_with_direction()
        .unwrap_or_default()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let idx = all.iter().position(|n| n == layer);
    let cut_next = |from: usize, up: bool| -> Option<String> {
        let mut i = from;
        loop {
            i = if up { i.checked_add(1)? } else { i.checked_sub(1)? };
            let n = all.get(i)?;
            if db.layer_get_type(n).unwrap_or_default() == "CUT" {
                return Some(n.clone());
            }
        }
    };
    let mut worst = 0;
    if let Some(i) = idx {
        for cut in [cut_next(i, false), cut_next(i, true)].into_iter().flatten() {
            for k in 0..db.num_layer_get_tech_layer_cut_enclosure_rules(&cut) {
                worst = worst
                    .max(db.cutenclosurerule_get_first_overhang(&cut, k))
                    .max(db.cutenclosurerule_get_second_overhang(&cut, k));
            }
            // ⚠️ A generate rule counts where it TOUCHES the cut layer, whichever of its three
            // entries names it — the enclosures taken are the rule's, not that entry's.
            for r in rules.iter().filter(|r| r.cut_layer == cut) {
                for (x, y) in [r.bottom_enclosure, r.top_enclosure] {
                    worst = worst.max(x).max(y);
                }
            }
        }
    }
    min_width + 2 * worst
}

/// Every generate rule the technology declares.
///
/// ⚠️ **The three layer entries of a rule are not in a guaranteed order**, so the cut layer is
/// found by which entry carries a RECTANGLE rather than by position: the lower and upper entries
/// carry enclosures, the cut entry carries the cut's shape. Indexing them 0/1/2 works on the
/// technologies where the order happens to hold and silently swaps enclosures on the others.
pub(crate) fn via_rules(db: &Db) -> Vec<ViaRule> {
    let mut out = Vec::new();
    for g in 0..db.tech_get_via_generate_rules().len() {
        let n = db.techviagenrule_get_via_layer_rule_count(g) as usize;
        let (mut cut, mut pitch) = ((0, 0), (0, 0));
        let mut cut_layer = String::new();
        let (mut layers, mut encs) = (Vec::new(), Vec::new());
        let mut widths: Vec<Option<(i32, i32)>> = Vec::new();
        for l in 0..n {
            let layer = db.techvialayerrule_get_layer(g, l);
            match db.via_layer_rule_rect(g, l) {
                Some(r) => {
                    cut_layer = layer.clone();
                    cut = (r.2 - r.0, r.3 - r.1);
                    let sp = (
                        db.techvialayerrule_get_spacing_x_spacing(g, l),
                        db.techvialayerrule_get_spacing_y_spacing(g, l),
                    );
                    // The rule states spacing; the generator works in pitch. A rule with no
                    // spacing gives a pitch of exactly one cut, which is a single-cut via.
                    pitch = (sp.0.max(cut.0), sp.1.max(cut.1));
                }
                None => {
                    // ⚠️ **Oriented for the layer.** The reference builds its enclosure from the
                    // rule's two overhangs and then swaps them so the smaller sits on the layer's
                    // constrained axis. Taking them as written leaves a horizontal layer carrying
                    // the vertical layer's enclosure and vice versa.
                    let raw = vyges_pdn::viagen::Enclosure {
                        x: db.techvialayerrule_get_enclosure_overhang1(g, l),
                        y: db.techvialayerrule_get_enclosure_overhang2(g, l),
                    };
                    let e = vyges_pdn::viagen::swap_for_layer(raw, direction_of(db, &layer));
                    layers.push(layer);
                    encs.push((e.x, e.y));
                    widths.push(db.techvialayerrule_has_width(g, l).then(|| {
                        (
                            db.techvialayerrule_get_width_min_width(g, l),
                            db.techvialayerrule_get_width_max_width(g, l),
                        )
                    }));
                }
            }
        }
        if layers.len() == 2 && cut.0 > 0 {
            // 🔑 **The rule's layers are sorted by LAYER NUMBER, never taken in the order the LEF
            // declares them.** `GenerateViaGenerator`'s constructor collects the three, sorts them
            // with `l->getNumber() < r->getNumber()` and indexes bottom/cut/top from that.
            //
            // ⚠️ **And a technology really does write one the other way round.** Nangate45 declares
            // `VIARULE Via9Array-0 GENERATE` as `metal10` then `metal9`, alone among its rules.
            // Read in order it becomes a metal10 -> metal9 rule, no rule is found for the
            // metal9/metal10 pair, and every via on that pair falls through to the technology's
            // own — which is a fallback the reference reaches only when no rule was buildable.
            let (lo, hi) = vyges_pdn::viagen::rule_layer_order(
                routing_level(db, &layers[0]),
                routing_level(db, &layers[1]),
            );
            out.push(ViaRule {
                name: db.techviagenrule_get_name(g),
                lower: layers[lo].clone(),
                upper: layers[hi].clone(),
                cut,
                pitch,
                bottom_enclosure: encs[lo],
                top_enclosure: encs[hi],
                bottom_width: widths[lo],
                top_width: widths[hi],
                cut_layer,
            });
        }
    }
    out
}

pub(crate) fn rows_of(db: &Db) -> Vec<followpins::Row> {
    let mut out = Vec::new();
    for i in 0.. {
        match db.nth_row(i) {
            Ok(Some((bbox, site, orient))) if bbox.len() == 4 => {
                // ⚠️ The SITE's height, not the row's — `determinePitch` picks the minimum by
                // site and only then measures that row's bbox. See `followpins::row_height`.
                let site_height = db.site_get_height(&site);
                // ⚠️ A hybrid site's row is skipped entirely by `makeShapes` — its row pattern
                // already holds the rails.
                let has_row_pattern = db.site_has_row_pattern(&site);
                out.push(followpins::Row {
                    bbox: (bbox[0], bbox[1], bbox[2], bbox[3]),
                    site,
                    orient,
                    site_height,
                    has_row_pattern,
                })
            }
            _ => break,
        }
    }
    out
}

pub(crate) fn area(db: &Db, core: bool) -> Rect {
    if core {
        (
            db.block_get_core_area_x_min(),
            db.block_get_core_area_y_min(),
            db.block_get_core_area_x_max(),
            db.block_get_core_area_y_max(),
        )
    } else {
        (
            db.block_get_die_area_x_min(),
            db.block_get_die_area_y_min(),
            db.block_get_die_area_x_max(),
            db.block_get_die_area_y_max(),
        )
    }
}

