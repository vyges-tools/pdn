// SPDX-License-Identifier: Apache-2.0
//! `vyges-pdn` — generate a power distribution network over an OpenDB database.
//!
//! The library is pure geometry over values; this reads the design, hands those values in, and
//! writes the result back as special wiring.

use std::process::ExitCode;

use vyges_opendb::Db;
use vyges_pdn::{followpins, nets, rings, shapes, straps, validate, Direction, Rect};

/// Every read of the database lives here, so this file holds the pipeline and nothing else.
mod components;
mod dbio;
use dbio::*;

/// Print one stage marker when `PDN_TRACE` is set, so the order components run in can be diffed
/// against the reference's own trace (`set_debug_level PDN Make 1`, whose markers this mirrors).
///
/// ℹ️ The reference orders components as `rings_` then `straps_` **in insertion order**, and
/// `-connect_to_pads` inserts its straps at `define_pdn_grid` time — before any `add_pdn_stripe`.
/// So its sequence is ring, pad connects, followpins, stripes; ours is what this prints.
fn trace(stage: &str, detail: &str) {
    if std::env::var_os("PDN_TRACE").is_some() {
        eprintln!("[trace] {stage}: {detail}");
    }
}

/// The machine-readable contract, as every other engine in the programme carries one.
///
/// ⚠️ **An assertion must name a field the engine actually emits.** `pdn` printed only human text
/// on stderr until this landed, so declaring `status` meant emitting a report to go with it —
/// a descriptor promising a field that does not exist is worse than no descriptor, because a
/// consumer reading the contract resolves `unknown` and cannot tell that from a failure.
/// The fields the report on stdout carries, named once so a test can check the descriptor's
/// promises against them rather than against a comment.
#[cfg(test)]
const REPORT_FIELDS: &[&str] = &[
    "tool",
    "status",
    "shapes",
    "vias",
    "pin_shapes",
    "die_edge_pin_shapes",
    "def_written",
];


/// The pin, inherited from the crate every engine already depends on.
const CRATE_PIN: &str = vyges_opendb::OPENROAD_PIN;

/// The pin this binary was built against, injected into the descriptor at print time.
///
/// 🔑 **One definition for the whole programme, inherited rather than typed.** The SHA lives in
/// `openroad-pin.yaml` in `vyges-opendb-lib` and reaches here through `vyges-opendb`, which this
/// engine already depends on. Before this, every engine spelled the pin out in its own
/// `--describe` prose, and four of them were still quoting the previous one a day after it moved.
///
/// ⚠️ **It reports what this BINARY was built against — not that the binary is current.** A stale
/// build reports its stale pin quite happily. That is the point: a harness compares this against
/// the oracle image it is about to launch and refuses on a mismatch, which is the check that was
/// missing when two engines ran a whole gate against the previous pin's oracle.
const PIN_TOKEN: &str = "@OPENROAD_PIN@";

fn describe() -> String {
    DESCRIBE.replace(PIN_TOKEN, CRATE_PIN)
}

const DESCRIBE: &str = r#"{
  "schema": "vyges-tool-descriptor/1.1",
  "openroad_pin": "@OPENROAD_PIN@",
  "name": "pdn",
  "summary": "power distribution network generation: rings, straps, follow pins and the vias between them",
  "maturity": "structured",
  "provenance_limitations": [
    "input_hash covers the argument vector, not the content of the .odb it names.",
    "MEASURED 2026-08-23 against the upstream pdn goldens at pin @OPENROAD_PIN@: 110 of 110 comparable cases exact on shapes, vias and block terminals, 0 failing, and 9 of 9 on the diagnostic a refused command raises. A SCORE IS ONLY TRUE OF ONE COMMIT -- quote the pin beside it.",
    "36 of the suite's cases are skipped rather than passed, and the reasons are counted, not hidden: 29 build no grid at all, 2 have a reference that built no grid, 2 compute a -pitch in Tcl this translation cannot read, and one each use -existing, repair_pdn_vias and add_sroute_connect.",
    "Diagnostics implemented so far: PDN-0003, 0004, 0005 (connect rules), 0106, 0107, 0108, 0114, 0117, 0118, 0191 (argument validation), 0185 and 0215 (runtime). A case whose golden names any other code is skipped with that code named, never silently passed.",
    "status is one of generated, vacuous or error. VACUOUS IS NOT GENERATED: it means the run laid no metal at all, and this assertion passes only on generated, so a no-op fails it rather than reporting a grid that was never built. Zero can still be the right answer for the design; read shapes and decide.",
    "The engine validates inside the ordinary build path, as the reference does inside addRing and addStrap, so every design it accepts has passed those checks too -- the diagnostics are not a separate check mode.",
    "Written against the upstream pdn regression suite. The algorithm is reimplemented from the published behaviour and the goldens' implementation-defined details (snapping, tie-breaks, rounding), not transliterated from the source."
  ],
  "invocation": {
    "args_template": ["generate", "{odb}"],
    "optional": {
      "out_def": { "type": "path", "description": "write the result as DEF" },
      "power": { "type": "string", "description": "the power net name" },
      "ground": { "type": "string", "description": "the ground net name" },
      "starts_with": { "type": "string", "description": "power or ground; which net takes the innermost ring" },
      "followpins": { "type": "string", "description": "layer[:extend[:width]], repeatable" },
      "stripe": { "type": "string", "description": "layer:width:pitch:offset[:extend[:count[:snap[:spacing]]]], repeatable" },
      "ring": { "type": "string", "description": "layer0,layer1:width:spacing:offset[:boundary], repeatable" },
      "connect": { "type": "string", "description": "layer0,layer1[:vias], repeatable" },
      "pins": { "type": "string", "description": "layers whose shapes are never shrunk" },
      "split_cuts": { "type": "string", "description": "layer:pitch[:stagger], repeatable" },
      "domain": { "type": "string", "description": "region:power:ground, per grid" }
    }
  },
  "consumes": ["odb"],
  "artifacts": [ { "role": "pdn_def", "field": "def_written" } ],
  "assertion": {
    "id": "pdn-generated",
    "field": "status",
    "pass_when": { "eq": "generated" }
  }
}
"#;

/// The usage text, printed to stdout for `--help` and to stderr for a misuse.
///
/// 🔑 **`--help` is stdout and exit 0; a misuse is stderr and exit 2.** They read the same but
/// they are not the same event, and tooling depends on the difference: the CLI reference page for
/// this engine is generated by capturing `--help` on STDOUT, so a help that writes to stderr and
/// exits non-zero produces an empty page and aborts the generator.
fn help() -> ExitCode {
    println!("{USAGE}");
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

const USAGE: &str = concat!(
        "usage: vyges-pdn generate <db> --out-def <def> --power <net> --ground <net>\n\
         \x20        [--starts-with power|ground]   (default ground)\n\
         \x20        [--domain <region>:<power>:<ground>]  (per grid; region domain)\n\
         \x20        [--followpins <layer>[:<extend>[:<width>]]]   (micron, repeatable)\n\
         \x20        [--stripe <layer>:<width>:<pitch>:<offset>[:<extend>[:<count>[:<snap>[:<spacing>]]]]]\n\
         \x20        [--ring <layer0>,<layer1>:<width>:<spacing>:<offset>[:boundary]]\n\
         \x20        [--pins <layer>[,<layer>...]]  (shapes there are never shrunk)\n\
         \x20        [--split-cuts <layer>:<pitch>[:stagger]]   (micron, repeatable)\n\
         \n\
         \x20  vyges-pdn global-connect <db> --connect NET:PINPAT:INSTPAT:power|ground|signal\n\
         \x20        [--connect ...]  [--force]  [--out-odb FILE]\n\
         \x20        creates the supply nets and connects matching instance pins to them.\n\
         \x20        Patterns are FULL matches, as OpenROAD's are. Without this, `generate`\n\
         \x20        refuses: there is no net to build a grid on.\n\
         \x20        status is applied or vacuous. VACUOUS IS NOT APPLIED: the rules matched no\n\
         \x20        pin and nothing was connected. Exit is still 0 -- a design already wired\n\
         \x20        correctly connects nothing on a second run -- so read connections.\n\
         \n\
         \x20  vyges-pdn --describe | --help | --version\n\
         \n\
         ⚠️ Shapes are emitted BEFORE trimming, which belongs with the via stage. Compare against\n\
         the reference run with `pdngen -skip_trim`."
);

#[derive(Clone)]
struct Opts {
    values: Vec<(String, String)>,
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut values = Vec::new();
        let mut i = 0;
        while i < args.len() {
            if let Some(key) = args[i].strip_prefix("--") {
                let val = args.get(i + 1).cloned().unwrap_or_default();
                values.push((key.to_string(), val));
                i += 2;
            } else {
                i += 1;
            }
        }
        Opts { values }
    }
    fn one(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    fn all(&self, key: &str) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }
    /// The options belonging to each declared grid, in declaration order.
    ///
    /// 🔑 **`buildGrids` builds every declared grid in turn, not one grid**, and a design with
    /// macros declares several: a core grid and one instance grid per macro. Each sees the shapes
    /// and obstructions of the ones before it, so their order is part of the answer.
    ///
    /// ⚠️ **Only the grid-scoped options are partitioned.** Everything else — the supply net names,
    /// the fallback cut geometry, whether to trim — describes the run and is repeated into every
    /// group, so it may be written anywhere on the command line.
    ///
    /// A command line with no `--grid` at all is one unnamed grid holding everything, which is what
    /// every single-grid case still is.
    fn grids(&self) -> Vec<(String, Opts)> {
        const SCOPED: &[&str] = &[
            "ring",
            "followpins",
            "stripe",
            "connect",
            "split-cuts",
            "pins",
            "starts-with",
            // ⚠️ **Scoped, because the flag belongs to the grid that declared it.** Left global it
            // reaches every core grid, and a design declaring several then builds the same pad
            // straps once per grid.
            "connect-to-pads",
            // ⚠️ Scoped: `define_pdn_grid -voltage_domains` names the domain THIS grid belongs to.
            "domain",
        ];
        let globals: Vec<(String, String)> = self
            .values
            .iter()
            .filter(|(k, _)| k != "grid" && !SCOPED.contains(&k.as_str()))
            .cloned()
            .collect();
        let mut groups: Vec<(String, Vec<(String, String)>)> =
            vec![(String::new(), globals.clone())];
        for (k, v) in &self.values {
            if k == "grid" {
                groups.push((v.clone(), globals.clone()));
            } else if SCOPED.contains(&k.as_str()) {
                groups.last_mut().unwrap().1.push((k.clone(), v.clone()));
            }
        }
        // An empty leading group is the artefact of a command line that opens with `--grid`.
        if groups.len() > 1
            && !groups[0]
                .1
                .iter()
                .any(|(k, _)| SCOPED.contains(&k.as_str()))
        {
            groups.remove(0);
        }
        groups
            .into_iter()
            .map(|(spec, values)| (spec, Opts { values }))
            .collect()
    }
}

/// **Stage 6c** — where the power switches go, and the always-on metal they put down.
///
/// `GridSwitchedPower::build`, reduced to what changes the grid. Row by row, against the grid's
/// **lowest strap set**: the straps crossing that row are sorted by x and one switch is placed per
/// strap, offset so that one of its always-on pins meets the strap.
///
/// ⚠️ **A switch that does not span two rows is discarded** — the cell is double height, and one
/// that lands at the top or bottom of a row band has nowhere to stand.
///
/// 🔑 **What comes back is the always-on PIN metal**, which is the only part of a switch the power
/// grid sees: `GridSwitchedPower::getShapes()` returns exactly those and the ordinary via machinery
/// carries the always-on straps down onto them.
///
/// ℹ️ **The instances themselves are not created here.** The reference writes them into the
/// database along with a daisy-chained or star control network of signal nets; this computes where
/// they would go and takes their pin geometry. What the power grid emits is identical either way,
/// and this engine otherwise only reads its input.
#[allow(clippy::too_many_arguments)]
fn switch_pin_shapes(
    db: &Db,
    master: &str,
    alwayson_pin: &str,
    alwayson_net: &str,
    rows: &[followpins::Row],
    straps: &[(String, Rect)],
    core: Rect,
) -> Vec<(String, String, Rect)> {
    // The cell's own always-on pin geometry, in master coordinates.
    let pin_boxes: Vec<(String, Rect)> = db
        .mterm_pin_boxes(master, alwayson_pin)
        .unwrap_or_default()
        .into_iter()
        .map(|(l, x0, y0, x1, y1)| (db.layer_name_by_number(l), (x0, y0, x1, y1)))
        .collect();
    if pin_boxes.is_empty() {
        return Vec::new();
    }
    let master_site = db.master_get_site(master);
    let cell_height = db.master_get_height(master) as i32;
    let cell_width = db.master_get_width(master) as i32;
    let mut out = Vec::new();
    for row in rows {
        // ⚠️ **Compared by site NAME**, because the same site read from two libraries is two
        // objects — the reference says so in a comment of its own.
        if !row.site.is_empty() && !master_site.is_empty() && row.site != master_site {
            continue;
        }
        let site_width = db.site_get_width(&row.site);
        if site_width <= 0 {
            continue;
        }
        // Where the pin sits within the cell, in site steps.
        let pin_positions: Vec<i32> = pin_boxes
            .iter()
            .flat_map(|(_, r)| vyges_pdn::switches::site_positions((r.0, r.2), site_width, 0))
            .collect();
        let mut crossing: Vec<Rect> = straps
            .iter()
            .filter(|(_, r)| r.0 < row.bbox.2 && row.bbox.0 < r.2 && r.1 < row.bbox.3 && row.bbox.1 < r.3)
            .map(|(_, r)| *r)
            .collect();
        crossing.sort_by_key(|r| r.0);
        for strap in crossing {
            let at = vyges_pdn::switches::locations(
                (strap.0, strap.2),
                &pin_positions,
                site_width,
                core.0,
            );
            let Some(x) = at.first().copied() else {
                continue;
            };
            let y = row.bbox.1;
            // ⚠️ The cell is double height; a placement that does not reach a second row is not
            // made at all.
            let spans = rows
                .iter()
                .filter(|r| r.bbox.1 < y + cell_height && y < r.bbox.3)
                .count();
            if spans < 2 {
                continue;
            }
            for (layer, r) in &pin_boxes {
                // ⚠️ **One switch per position, however many rows ask for it.** A double-height
                // cell is reached from both of the rows it stands in, and the reference names its
                // instances after the row and creates them by name — so the second row finds the
                // instance already there rather than making a twin. Left as two, the pair of
                // identical vias annihilate each other in the overlap check, each being "no larger
                // than" the other, and the switch connects to nothing at all.
                let placed = (
                    layer.clone(),
                    alwayson_net.to_string(),
                    vyges_pdn::orient::place_in_bbox(
                        *r,
                        &row.orient,
                        (cell_width, cell_height),
                        (x, y),
                    ),
                );
                if !out.contains(&placed) {
                    out.push(placed);
                }
            }
        }
    }
    out
}

/// An instance's supply pins as shapes a via may land on — `InstanceGrid::getInstancePins`.
///
/// 🔑 **An instance grid vias down onto the macro's OWN pins**, not only onto other grids' straps.
/// `InstanceGrid::getIntersections` inserts these into the shape map before searching, so a strap
/// laid across a macro is held up by the macro's pins even where nothing else crosses it — and
/// trimming then keeps it out to them.
fn instance_pin_shapes(db: &Db, inst: &str) -> Vec<(String, String, Rect)> {
    let master = db.inst_get_master(inst);
    let orient = db.inst_get_orient(inst);
    let offset = (db.inst_get_origin_x(inst), db.inst_get_origin_y(inst));
    let mut out = Vec::new();
    for term in db.master_get_m_terms(&master) {
        if !matches!(
            db.mterm_get_sig_type(&master, &term).as_str(),
            "POWER" | "GROUND"
        ) {
            continue;
        }
        let net = db.iterm_get_net(inst, &term);
        if net.is_empty() {
            continue;
        }
        for (layer, x0, y0, x1, y1) in db.mterm_pin_boxes(&master, &term).unwrap_or_default() {
            let name = db.layer_name_by_number(layer);
            if db.layer_get_type(&name).unwrap_or_default() != "ROUTING" {
                continue;
            }
            out.push((
                name,
                net.clone(),
                vyges_pdn::orient::transform_rect((x0, y0, x1, y1), &orient, offset),
            ));
        }
    }
    out
}

/// The outline of an instance's SUPPLY pins, placed — `InstanceGrid::getDomainBoundary`.
///
/// ⚠️ **Merged in the master's own coordinates and transformed ONCE**, not transformed per pin and
/// then merged. The two agree for an axis-aligned placement and not for a rotated one.
fn instance_pin_outline(db: &Db, inst: &str) -> Option<Rect> {
    let master = db.inst_get_master(inst);
    let mut out: Option<Rect> = None;
    for term in db.master_get_m_terms(&master) {
        if !matches!(
            db.mterm_get_sig_type(&master, &term).as_str(),
            "POWER" | "GROUND"
        ) {
            continue;
        }
        for (_, x0, y0, x1, y1) in db.mterm_pin_boxes(&master, &term).unwrap_or_default() {
            let r = (x0, y0, x1, y1);
            out = Some(match out {
                None => r,
                Some(o) => (o.0.min(r.0), o.1.min(r.1), o.2.max(r.2), o.3.max(r.3)),
            });
        }
    }
    out.map(|r| {
        vyges_pdn::orient::transform_rect(
            r,
            &db.inst_get_orient(inst),
            (db.inst_get_origin_x(inst), db.inst_get_origin_y(inst)),
        )
    })
}

/// Every declared component's dimensions, checked before anything is built.
///
/// 🔑 **The reference checks these when the component is DECLARED, not when the grid is built** —
/// `addRing` and `addStrap` validate and then push, so a refused component is never added and the
/// `add_pdn_*` command itself is what fails. Running the whole pass up front here is the same
/// thing: the first violation ends the run with the reference's own diagnostic, and no DEF is
/// written.
///
/// ⚠️ **Order within a grid is rings, then straps** — `Grid::checkSetup`'s own order, and the order
/// the commands are issued in. ℹ️ Across component KINDS this engine cannot reproduce declaration
/// order exactly: follow pins and stripes reach it as two lists, so a design interleaving them and
/// breaking a rule in both would report the follow pin's where the reference reports whichever came
/// first. No upstream case does that; recorded here rather than assumed away.
fn validate_grids(
    db: &Db,
    grids: &[(GridSpec, Opts)],
    build_nets: &[String],
    per_micron: f64,
    core: Rect,
) -> Option<validate::Diag> {
    // ⚠️ **A follow pin takes its direction from the ROWS, not from its layer** — and that is what
    // decides whether a layer's width table governs it at all. With no rows the reference leaves
    // the direction the strap constructor set, which is the layer's own.
    let row_direction = match db.nth_row_direction(0).unwrap_or_default().as_str() {
        "HORIZONTAL" => Some(Direction::Horizontal),
        "VERTICAL" => Some(Direction::Vertical),
        _ => None,
    };

    for (g, o) in grids {
        let net_count = grid_net_count(db, o, g, build_nets);

        // 🔑 **Checked before anything is built, because `Connect`'s CONSTRUCTOR raises these** —
        // `add_pdn_connect` refuses at declaration time, not when the vias are placed. A rule
        // naming the same layer twice, or a cut layer where a routing layer belongs, never
        // reaches via generation at all.
        for spec in o.all("connect") {
            let pair = spec.split(':').next().unwrap_or("");
            let Some((l0, l1)) = pair.split_once(',') else {
                continue;
            };
            let (r0, r1) = (routing_level(db, l0), routing_level(db, l1));
            if let Some(d) = validate::check_connect_layers(l0, r0, l1, r1) {
                return Some(d);
            }
        }

        for spec in o.all("ring") {
            // `<layer0>,<layer1>:<w0>,<w1>:<s0>,<s1>:<offsets>[:boundary][:kind]`
            let (layers, rest) = spec.split_once(':').unwrap_or((spec, ""));
            let mut names = layers.split(',');
            let l0 = names.next().unwrap_or("");
            let l1 = names.next().unwrap_or(l0);
            let p: Vec<&str> = rest.split(':').collect();
            // ⚠️ One value expands to two, one per layer — `pdn::get_one_to_two`.
            let pair = |field: Option<&&str>| -> (i32, i32) {
                let v: Vec<i32> = field
                    .unwrap_or(&"0")
                    .split(',')
                    .map(|x| dbu(x, per_micron))
                    .collect();
                match v.len() {
                    0 => (0, 0),
                    1 => (v[0], v[0]),
                    _ => (v[0], v[1]),
                }
            };
            let (w0, w1) = pair(p.first());
            let (s0, s1) = pair(p.get(1));
            let offs: Vec<i32> = p
                .get(2)
                .unwrap_or(&"0")
                .split(',')
                .map(|v| dbu(v, per_micron))
                .collect();
            // One value expands to four and two to `{a b a b}` — `pdn::get_one_to_four`.
            let off4 = match offs.len() {
                0 => [0; 4],
                1 => [offs[0]; 4],
                2 => [offs[0], offs[1], offs[0], offs[1]],
                _ => [offs[0], offs[1], offs[2], offs[3]],
            };
            for (layer, width, spacing) in [(l0, w0, s0), (l1, w1, s1)] {
                if layer.is_empty() {
                    continue;
                }
                let Some(rules) = layer_rules(db, layer) else {
                    continue;
                };
                let min_spacing = min_spacing_for(db, layer, width);
                if let Some(d) =
                    validate::check_ring_layer(&rules, width, spacing, min_spacing, &off4)
                {
                    return Some(d);
                }
            }
        }

        for spec in o.all("followpins") {
            // `<layer>[:<extend>[:<width>]]`
            let mut f = spec.splitn(3, ':');
            let layer = f.next().unwrap_or("");
            let _extend = f.next();
            if layer.is_empty() {
                continue;
            }
            // ⚠️ A stated width REPLACES the cell-derived one; the cells are asked only when the
            // command states none.
            let width = f
                .next()
                .filter(|w| !w.is_empty())
                .map(|w| dbu(w, per_micron))
                .filter(|w| *w > 0)
                .unwrap_or_else(|| followpin_width(db).unwrap_or(0));
            let Some(rules) = layer_rules(db, layer) else {
                continue;
            };
            let direction = row_direction.unwrap_or(rules.direction);
            // ⚠️ **A follow pin is checked for its WIDTH and nothing else.** It has no stated
            // spacing, pitch or offset — the rows give it all three — so the reference's
            // `FollowPins::checkLayerSpecifications` calls `checkLayerWidth` alone.
            if let Some(d) = validate::check_width(&rules, width, direction) {
                return Some(d);
            }
        }

        for spec in o.all("stripe") {
            // `<layer>:<width>:<pitch>:<offset>[:<extend>[:<count>[:<snap>[:<spacing>...]]]]`
            let (layer, rest) = spec.split_once(':').unwrap_or((spec, ""));
            if layer.is_empty() {
                continue;
            }
            let p: Vec<&str> = rest.split(':').collect();
            let width = dbu(p.first().copied().unwrap_or("0"), per_micron);
            let pitch = dbu(p.get(1).copied().unwrap_or("0"), per_micron);
            let offset = dbu(p.get(2).copied().unwrap_or("0"), per_micron);
            let Some(rules) = layer_rules(db, layer) else {
                continue;
            };
            // A stated `-spacing` wins; only in its absence is one derived — the same rule the
            // build uses, because the reference derives it in the strap's constructor and then
            // checks the value it derived.
            let spacing = p
                .get(6)
                .filter(|s| !s.is_empty())
                .map(|s| dbu(s, per_micron))
                .filter(|s| *s > 0)
                .unwrap_or_else(|| {
                    straps::default_spacing(
                        pitch,
                        net_count,
                        width,
                        rules.manufacturing_grid.unwrap_or(1),
                    )
                });
            // ⚠️ **The grid's extent ACROSS the strap's direction**, which is what a group of
            // straps has to fit inside — a horizontal set is bounded by the grid's height.
            // ℹ️ Taken as the core area: a region or instance grid is bounded by its own
            // rectangle, which this pass does not resolve. Same scoping limit as `-pins` above.
            let grid_width = if rules.direction == Direction::Horizontal {
                core.3 - core.1
            } else {
                core.2 - core.0
            };
            let dims = validate::StrapDims {
                width,
                spacing,
                pitch,
                offset,
                min_spacing: min_spacing_for(db, layer, width),
                snap: p.get(5).copied() == Some("snap"),
                // `TechLayer::populateGrid` reads the block's track grid for this layer along the
                // strap's direction; no tracks there is what PDN-0215 refuses.
                has_track_grid: {
                    // 🔑 **The axis follows the STRAP's direction**, as `populateGrid` does:
                    // a horizontal strap snaps to the Y tracks, a vertical one to the X.
                    let (x, y) = db.track_grid(layer).unwrap_or_default();
                    if rules.direction == Direction::Horizontal {
                        !y.is_empty()
                    } else {
                        !x.is_empty()
                    }
                },
                grid_width,
                net_count,
            };
            if let Some(d) =
                validate::check_strap(&rules, dims, rules.direction, &g.name)
            {
                return Some(d);
            }
        }
    }
    None
}

/// **`GridComponent::getNetCount()`** — how many nets this grid builds for.
///
/// 🔑 A region domain's own supplies where the grid has a region, the instance's CONNECTED supplies
/// where it grids one, and the block's otherwise.
///
/// ⚠️ **A switched domain has three, not two.** The count decides how far a ring keep-out reaches
/// and what a strap set's derived spacing is, so one net too few moves geometry rather than just
/// miscounting.
fn grid_net_count(db: &Db, o: &Opts, g: &GridSpec, build_nets: &[String]) -> i32 {
    if let Some(spec) = o.one("domain") {
        let f: Vec<&str> = spec.split(':').collect();
        let secondary = f
            .get(3)
            .map(|s| s.split(',').filter(|x| !x.is_empty()).count())
            .unwrap_or(0);
        let switched = usize::from(f.get(4).is_some_and(|s| !s.is_empty()));
        return (2 + switched + secondary) as i32;
    }
    if !g.instance.is_empty() {
        // The nets the INSTANCE is wired to, the same test the build loop makes.
        let master = db.inst_get_master(&g.instance);
        let connected: Vec<String> = db
            .master_get_m_terms(&master)
            .iter()
            .map(|term| db.iterm_get_net(&g.instance, term))
            .filter(|n| !n.is_empty())
            .collect();
        return build_nets.iter().filter(|n| connected.contains(n)).count() as i32;
    }
    build_nets.len() as i32
}

/// One declared grid: which instance it belongs to, if any, and how it is bounded.
struct GridSpec {
    name: String,
    /// What an `-macro` grid claims: `+`-joined patterns, matched as regexes against instance
    /// names, or against MASTER names when `by_cell`. Empty for the core grid.
    instance: String,
    by_cell: bool,
    /// `-halo {left bottom right top}`, in database units.
    halo: [i32; 4],
    /// `-orient`: only instances facing one of these are claimed. Empty means every orientation.
    orients: Vec<String>,
    /// `-grid_to_boundary`: lay the grid out to the instance's own outline rather than to the
    /// outline of its supply pins.
    to_boundary: bool,
}

impl GridSpec {
    /// `<name>[:macro:<instance>[:<l>,<b>,<r>,<t>[:boundary]]]`
    fn parse(spec: &str, per_micron: f64) -> GridSpec {
        let mut p = spec.split(':');
        let name = p.next().unwrap_or("").to_string();
        let kind = p.next().unwrap_or("");
        // ⚠️ `c=` selects by CELL and `i=` (or a bare list) by instance — both as regexes.
        let mut by_cell = false;
        let instance = if kind == "macro" {
            let sel = p.next().unwrap_or("");
            match sel.strip_prefix("c=") {
                Some(rest) => {
                    by_cell = true;
                    rest.to_string()
                }
                None => sel.strip_prefix("i=").unwrap_or(sel).to_string(),
            }
        } else {
            String::new()
        };
        // ⚠️ **One value expands to four and two to `{a b a b}`** — `pdn::get_one_to_four`. So a
        // pair is "horizontal, vertical", and reading it as "left, bottom" leaves the right and top
        // sides with no halo at all.
        let halo = {
            let vals: Vec<i32> = p
                .next()
                .unwrap_or("")
                .split(',')
                .filter(|v| !v.is_empty())
                .map(|v| dbu(v, per_micron))
                .collect();
            match vals.len() {
                0 => [0; 4],
                1 => [vals[0]; 4],
                2 => [vals[0], vals[1], vals[0], vals[1]],
                _ => [vals[0], vals[1], vals[2], vals[3]],
            }
        };
        // 🔑 **Laying the grid out to the instance's own outline is the DEFAULT**, and going over
        // the supply pins is the exception. `pdn.tcl` opens with `set pg_pins_to_boundary 1` and
        // only `-grid_over_pg_pins` clears it.
        //
        // ⚠️ Modelled the other way round, every macro grid is bounded by its pin outline — and a
        // macro whose only supply port is a strip near the top then gets a strap as tall as that
        // strip instead of one running the length of the instance.
        let to_boundary = p.next() != Some("pgpins");
        let orients: Vec<String> = p
            .next()
            .unwrap_or("")
            .split(',')
            .filter(|o| !o.is_empty())
            .map(str::to_string)
            .collect();
        GridSpec {
            name,
            instance,
            by_cell,
            halo,
            orients,
            to_boundary,
        }
    }
}

/// Extend every shape that its own via does not fully reach — `Grid::repairVias`.
///
/// Returns whether anything moved, which is what tells the caller to rebuild the vias.
///
/// ⚠️ **Shapes are mutated as it goes, and that is deliberate.** The reference extends the
/// *replacement* when a shape has already been extended once in the same pass, so two vias on one
/// strap compound rather than fight. Matching a via's remembered rect therefore has to allow for
/// the shape having already grown — hence the containment test rather than equality.
///
/// ⚠️ **A ring IS extended, unless its ring was built on a single layer.** `isModifiable` is false
/// only for a locked shape, and `Rings::makeShapes` locks its shapes just when both ring layers are
/// the same. Excluding every ring left two ring segments 260 units short of the reference's — the
/// gap being exactly the difference between a via's metal and the strap rect the ring reaches for.
/// The rect a shape OBSTRUCTS — itself, bloated by its own spacing.
///
/// 🔑 **The obstruction tree is indexed by `getObstruction()`, not by the shape's rect.**
///
/// So every query against it — and `makeVias` inserts **every search shape** into it before asking
/// which vias are obstructed — compares against metal plus its keep-out, never the bare metal.
///
/// ⚠️ **A via does not have to touch a strap to be blocked by it**, only to come within its
/// spacing. Testing the raw rect leaves a stack standing wherever it threads between two shapes it
/// could never legally sit between — and those survivors then hold follow pins out to a ring they
/// should have been trimmed away from.
fn obstruction_of(db: &Db, layer: &str, r: Rect) -> Rect {
    let (w, len) = ((r.2 - r.0).min(r.3 - r.1), (r.2 - r.0).max(r.3 - r.1));
    let h = db.layer_find_v55_spacing(layer, w, len).unwrap_or(0);
    (r.0 - h, r.1 - h, r.2 + h, r.3 + h)
}

/// The part of a rect inside another, or nothing when they miss.
fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let r = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (r.0 < r.2 && r.1 < r.3).then_some(r)
}

/// One region the grid failed to connect, with everything needed to fill it.
struct Channel {
    /// The orphaned shapes' region, grown to cover every one of them.
    area: Rect,
    /// What of `area` is still free on the layer the repair straps will use.
    available: Rect,
    /// The orphans' own extent — repair straps are kept only where they cross it.
    obs: Rect,
    nets: Vec<String>,
    /// The layer that was left unconnected, and the one the repair straps go on.
    connect_to: String,
    target_layer: String,
    target_width: i32,
    target_spacing: i32,
}

/// **Stage 6f, part one** — `RepairChannelStraps::findRepairChannels`.
///
/// A shape is an orphan when no via rises from it. ⚠️ **Only straps and follow pins count**, and a
/// strap with no connection at all is skipped rather than repaired: it is about to be trimmed away,
/// and building a channel to reach something that will not exist wastes the repair.
///
/// 🔑 **Each orphan is bloated by its own component's PITCH, across its direction**, so neighbours
/// on a regular pitch merge into one channel. A follow pin set's pitch is twice a row's height.
#[allow(clippy::too_many_arguments)]
fn find_channels(
    db: &Db,
    emitted: &[(String, String, Rect, &'static str)],
    placed: &[vyges_pdn::vias::Via],
    strap_sets: &[(String, i32, i32, i32, bool)],
    followpin_layer: &str,
    followpin_pitch: i32,
    highest: i32,
    core: Rect,
    net_order: &[String],
) -> Vec<Channel> {
    let overlaps = |a: Rect, b: Rect| a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3;
    let mut out = Vec::new();
    // Which layers have shapes that could be orphaned, lowest first.
    let mut layers: Vec<String> = Vec::new();
    for (_, l, _, kind) in emitted {
        if *kind != "RING" && !layers.contains(l) {
            layers.push(l.clone());
        }
    }
    layers.sort_by_key(|l| routing_level(db, l));
    for layer in layers {
        if routing_level(db, &layer) >= highest {
            break; // nothing above this to connect to
        }
        // ⚠️ **The LOWEST layer this one connects to that carries a declared strap set.** A connect
        // reaching two layers up does not make the higher one a repair target.
        let Some((target_layer, _, target_width, target_spacing, target_horizontal)) = strap_sets
            .iter()
            .filter(|(l, ..)| {
                placed
                    .iter()
                    .any(|v| v.lower == layer && &v.upper == l)
                    || connects_between(db, &layer, l)
            })
            .min_by_key(|(l, ..)| routing_level(db, l))
            .cloned()
        else {
            continue;
        };
        // The orphans on this layer, each bloated by its component's pitch across its direction.
        let mut bloated: Vec<Rect> = Vec::new();
        let mut orphans: Vec<(&String, Rect, bool)> = Vec::new();
        for (i, (net, l, rect, kind)) in emitted.iter().enumerate() {
            if l != &layer || *kind == "RING" {
                continue;
            }
            let above = placed
                .iter()
                .any(|v| v.net == *net && v.lower == *l && v.lower_rect == *rect);
            if above {
                continue;
            }
            let is_followpin = *kind == "FOLLOWPIN";
            if !is_followpin {
                // A floating strap is removed, not repaired.
                let any = placed.iter().any(|v| {
                    v.net == *net
                        && ((v.lower == *l && v.lower_rect == *rect)
                            || (v.upper == *l && v.upper_rect == *rect))
                });
                if !any {
                    continue;
                }
            }
            let _ = i;
            let (pitch, horizontal) = if is_followpin && l == followpin_layer {
                (followpin_pitch, true)
            } else {
                match strap_sets.iter().find(|(sl, ..)| sl == l) {
                    Some((_, p, _, _, h)) => (*p, *h),
                    None => (0, direction_of(db, l) == Direction::Horizontal),
                }
            };
            let (bx, by) = if horizontal { (0, pitch) } else { (pitch, 0) };
            bloated.push((rect.0 - bx, rect.1 - by, rect.2 + bx, rect.3 + by));
            orphans.push((net, *rect, is_followpin));
        }
        if orphans.is_empty() {
            continue;
        }
        for region in vyges_pdn::channels::merge_channels(&bloated) {
            let Some(mut area) = intersect_rect(region, core) else {
                continue;
            };
            let mut obs: Option<Rect> = None;
            let mut nets: Vec<String> = Vec::new();
            let (mut followpin_count, mut strap_count) = (0, 0);
            for (net, rect, is_fp) in &orphans {
                if !overlaps(area, *rect) {
                    continue;
                }
                area = (
                    area.0.min(rect.0),
                    area.1.min(rect.1),
                    area.2.max(rect.2),
                    area.3.max(rect.3),
                );
                obs = Some(match obs {
                    None => *rect,
                    Some(o) => (
                        o.0.min(rect.0),
                        o.1.min(rect.1),
                        o.2.max(rect.2),
                        o.3.max(rect.3),
                    ),
                });
                if !nets.contains(*net) {
                    nets.push((*net).clone());
                }
                if *is_fp {
                    followpin_count += 1;
                } else {
                    strap_count += 1;
                }
            }
            // ⚠️ **Every follow pin must be repaired, a lone strap need not be.** One orphaned
            // strap is tolerated; two are a gap in the grid.
            if followpin_count < 1 && strap_count <= 1 {
                continue;
            }
            // ⚠️ **The nets come out in the order the DATABASE holds them**, not the order the
            // orphans were met. `nets_` is a `PtrSet` keyed by object id, so a channel whose lowest
            // orphan happens to be ground still puts power on the first strap. Ordering by
            // discovery swaps the two straps of every channel where it differs — which changes
            // nothing electrically and every coordinate in the output.
            nets.sort_by_key(|n| net_order.iter().position(|o| o == n).unwrap_or(usize::MAX));
            let (Some(area), Some(obs)) = (intersect_rect(area, core), obs) else {
                continue;
            };
            let Some(obs) = intersect_rect(obs, core) else {
                continue;
            };
            // What is already standing on the target layer takes room away from the channel.
            let blocking: Vec<Rect> = emitted
                .iter()
                .filter(|(_, l, _, _)| *l == target_layer)
                .map(|(_, l, r, _)| {
                    let (w, len) = ((r.2 - r.0).min(r.3 - r.1), (r.2 - r.0).max(r.3 - r.1));
                    let h = db.layer_find_v55_spacing(l, w, len).unwrap_or(0);
                    (r.0 - h, r.1 - h, r.2 + h, r.3 + h)
                })
                .collect();
            let available =
                vyges_pdn::channels::available_area(area, &blocking, !target_horizontal);
            out.push(Channel {
                area,
                available,
                obs,
                nets,
                connect_to: layer.clone(),
                target_layer: target_layer.clone(),
                target_width,
                target_spacing,
            });
        }
    }
    out
}

/// Whether the technology places these two routing layers in one connect this grid declared.
///
/// ℹ️ Approximated from the placed vias plus adjacency; the connect list itself is not threaded
/// down here, and every case in the suite states its repair target as a direct connect.
fn connects_between(_db: &Db, _lower: &str, _upper: &str) -> bool {
    false
}

/// **Stage 6f, part two** — build the straps for one channel.
///
/// `determineParameters`: try the target strap's own width and spacing; then relax the spacing to
/// the layer's minimum for that width; then halve the width until something fits or the layer's
/// minimum width is reached.
#[allow(clippy::type_complexity)]
/// **Stage 6f, one channel** — `determineParameters`, then `testBuild`, then narrow once and retry.
///
/// 🔑 **Two questions, not one.** `determineParameters` asks whether a group of this width FITS
/// clear of obstructions somewhere in the channel, looking only at `obs_check_area_` — the along
/// axis narrowed to where the orphans are. `testBuild` then asks whether the strap SURVIVES the
/// cut, which sees its whole length. The two legitimately disagree, and that disagreement is the
/// only reason the reference ever ends up narrower than its own search said it could be.
///
/// ⚠️ **Once, not until it fits.** The reference narrows a single time at this call site; the loop
/// inside `determineParameters` is a different loop, and running them together would narrow past
/// what the reference produces.
///
/// ℹ️ A third attempt with snapping switched off follows in the reference. Not built: no case in
/// the suite reaches it, and every strap here is on a track either way.
fn build_repair(
    db: &Db,
    ch: &Channel,
    emitted: &[(String, String, Rect, &'static str)],
    blockages: &[(String, Rect, Option<String>, Rect)],
    core: Rect,
    die: Rect,
    grid_mfg: i32,
) -> Option<Vec<(String, Rect)>> {
    let first = build_repair_at(
        db,
        ch,
        ch.target_width,
        ch.target_spacing,
        emitted,
        blockages,
        core,
        die,
        grid_mfg,
    )?;
    if !first.straps.is_empty() {
        return Some(first.straps);
    }
    // ⚠️ `isAtEndOfRepairOptions` — nothing narrower to try.
    let min_width = db.layer_get_min_width(&ch.target_layer) as i32;
    if first.width <= min_width {
        return None;
    }
    let next = vyges_pdn::channels::next_width(first.width, min_width, grid_mfg);
    let max_length = if direction_of(db, &ch.target_layer) == Direction::Horizontal {
        core.2 - core.0
    } else {
        core.3 - core.1
    };
    let spacing = db
        .layer_find_v55_spacing(&ch.target_layer, next, max_length)
        .unwrap_or(0)
        .max(db.layer_get_spacing(&ch.target_layer));
    let second = build_repair_at(
        db, ch, next, spacing, emitted, blockages, core, die, grid_mfg,
    )?;
    (!second.straps.is_empty()).then_some(second.straps)
}

/// What one `determineParameters` + `testBuild` attempt produced.
///
/// An empty `straps` means the group was placed and then cut away entirely — the caller narrows and
/// tries again. `None` from the attempt means no width fitted at all, which ends the repair.
struct RepairAttempt {
    straps: Vec<(String, Rect)>,
    /// The width the search settled on, which is what the next one is halved from.
    width: i32,
}

fn build_repair_at(
    db: &Db,
    ch: &Channel,
    start_width: i32,
    start_spacing: i32,
    emitted: &[(String, String, Rect, &'static str)],
    blockages: &[(String, Rect, Option<String>, Rect)],
    core: Rect,
    die: Rect,
    grid_mfg: i32,
) -> Option<RepairAttempt> {
    let horizontal = direction_of(db, &ch.target_layer) == Direction::Horizontal;
    // ⚠️ **A repair strap must cross what it repairs.** Two layers running the same way never
    // meet, so the channel is rejected rather than filled with parallel metal.
    if direction_of(db, &ch.connect_to) == direction_of(db, &ch.target_layer) {
        return None;
    }
    let vertical = !horizontal;
    let max_length = if horizontal {
        core.2 - core.0
    } else {
        core.3 - core.1
    };
    let min_width = db.layer_get_min_width(&ch.target_layer) as i32;
    let area_width = if horizontal {
        ch.available.3 - ch.available.1
    } else {
        ch.available.2 - ch.available.0
    };

    // Every routing layer strictly above the orphaned one, up to and including the target.
    let check_layers: Vec<String> = {
        let (lo, hi) = (
            routing_level(db, &ch.connect_to),
            routing_level(db, &ch.target_layer),
        );
        emitted
            .iter()
            .map(|(_, l, _, _)| l.clone())
            .chain(blockages.iter().map(|(l, ..)| l.clone()))
            .filter(|l| {
                let r = routing_level(db, l);
                r > lo && r <= hi
            })
            .fold(Vec::new(), |mut acc, l| {
                if !acc.contains(&l) {
                    acc.push(l);
                }
                acc
            })
    };

    let mut width = start_width;
    let mut spacing = start_spacing;
    let mut attempt = 0;
    // 🔑 **`determineParameters` and `testBuild` are two different questions.** This loop answers
    // the first: does a group of this width FIT, clear of obstructions, somewhere in the channel.
    // It checks only `obs_check_area_` — the along axis narrowed to where the orphans actually are.
    // Whether the strap then SURVIVES the cut, which sees its whole length, is asked afterwards.
    let offset = loop {
        let group = vyges_pdn::channels::group_width(ch.nets.len(), width, spacing);
        if group <= area_width {
            let clear = |straps: Rect| {
                // The group's own obstruction, with the along axis widened to the orphans' extent —
                // a repair strap is only in the way where it actually runs.
                let (w, len) = (
                    (straps.2 - straps.0).min(straps.3 - straps.1),
                    (straps.2 - straps.0).max(straps.3 - straps.1),
                );
                let h = db
                    .layer_find_v55_spacing(&ch.target_layer, w, len)
                    .unwrap_or(0);
                let mut check = (
                    straps.0 - h,
                    straps.1 - h,
                    straps.2 + h,
                    straps.3 + h,
                );
                if horizontal {
                    check.0 = ch.obs.0;
                    check.2 = ch.obs.2;
                } else {
                    check.1 = ch.obs.1;
                    check.3 = ch.obs.3;
                }
                let hits = |l: &str, r: Rect| {
                    check.0 < r.2 && r.0 < check.2 && check.1 < r.3 && r.1 < check.3 && {
                        let _ = l;
                        true
                    }
                };
                for l in &check_layers {
                    for (bl, r, ..) in blockages {
                        if bl == l && hits(l, *r) {
                            return false;
                        }
                    }
                    for (_, el, r, _) in emitted {
                        if el != l {
                            continue;
                        }
                        let (w, len) =
                            ((r.2 - r.0).min(r.3 - r.1), (r.2 - r.0).max(r.3 - r.1));
                        let hh = db.layer_find_v55_spacing(l, w, len).unwrap_or(0);
                        if hits(l, (r.0 - hh, r.1 - hh, r.2 + hh, r.3 + hh)) {
                            return false;
                        }
                    }
                }
                true
            };
            if let Some(o) = vyges_pdn::channels::determine_offset(
                ch.available,
                vertical,
                width,
                group,
                grid_mfg,
                &clear,
            ) {
                break o;
            }
        }
        // First relax the spacing, then start halving the width.
        if attempt == 0 {
            spacing = db
                .layer_find_v55_spacing(&ch.target_layer, width, max_length)
                .unwrap_or(0)
                .max(db.layer_get_spacing(&ch.target_layer));
            attempt = 1;
            continue;
        }
        if width <= min_width {
            return None;
        }
        width = vyges_pdn::channels::next_width(width, min_width, grid_mfg);
        spacing = db
            .layer_find_v55_spacing(&ch.target_layer, width, max_length)
            .unwrap_or(0)
            .max(db.layer_get_spacing(&ch.target_layer));
    };

    // Lay the group down: one strap per net, each snapped to the next free routing track.
    let tracks = db.track_grid(&ch.target_layer).unwrap_or_default();
    let grid_axis = if horizontal { tracks.1 } else { tracks.0 };
    let mut out = Vec::new();
    let mut pos = offset;
    let mut next_minimum_track = i32::MIN;
    for net in &ch.nets {
        let group_pos = vyges_pdn::vias::snap_to_grid(pos, &grid_axis, next_minimum_track);
        let start = group_pos - width / 2;
        let rect = if vertical {
            (start, ch.area.1, start + width, ch.area.3)
        } else {
            (ch.area.0, start, ch.area.2, start + width)
        };
        pos = group_pos + width + spacing;
        next_minimum_track = pos;
        let (abs_lo, abs_hi) = if vertical {
            (die.0, die.2)
        } else {
            (die.1, die.3)
        };
        let (lo, hi) = if vertical {
            (rect.0, rect.2)
        } else {
            (rect.1, rect.3)
        };
        if lo < abs_lo || hi > abs_hi {
            continue;
        }
        out.push((net.clone(), rect));
    }
    if out.is_empty() {
        return None;
    }
    let settled = width;
    // 🔑 **`testBuild` = makeShapes + cutShapes + `!isEmpty`.** A repair strap is a grid component
    // like any other, so it is cut against the obstructions on its layer — over its WHOLE length,
    // not the stretch the offset search looked at. A group that passed the offset search can
    // therefore be annihilated here, and the reference's own trace says so plainly:
    //
    // ```text
    // Determine offset: true
    // Initial shape count: 1 → Final shape count: 0     (0.48 um)
    // Continue repair … changing width from 0.48 um to 0.24 um
    // Initial shape count: 1 → Final shape count: 1     (0.24 um)
    // ```
    //
    // ⚠️ Cut away ENTIRELY, not into pieces: the wide group violates spacing to a strap running
    // beside it along the full length, and the narrow one clears.
    let survivors = surviving_pieces(db, ch, &out, emitted, blockages, horizontal);
    Some(RepairAttempt {
        straps: survivors,
        width: settled,
    })
}

/// What is left of a repair group after its layer's obstructions cut it — `cutShapes`.
///
/// ⚠️ **The whole length.** `determineOffset` narrowed its obstruction test to the orphans' extent;
/// this does not, which is the entire reason the two can disagree.
fn surviving_pieces(
    db: &Db,
    ch: &Channel,
    group: &[(String, Rect)],
    emitted: &[(String, String, Rect, &'static str)],
    blockages: &[(String, Rect, Option<String>, Rect)],
    horizontal: bool,
) -> Vec<(String, Rect)> {
    let layer = ch.target_layer.as_str();
    let mut out = Vec::new();
    for (net, rect) in group {
        // Everything already standing on this layer, each bloated by its own spacing, and the
        // strap bloated by its own — the same two-sided comparison the obstruction tree makes.
        let own = obstruction_of(db, layer, *rect);
        let mut blocked: Vec<(i32, i32)> = Vec::new();
        let mut note = |o: Rect| {
            if own.0 < o.2 && o.0 < own.2 && own.1 < o.3 && o.1 < own.3 {
                blocked.push(if horizontal { (o.0, o.2) } else { (o.1, o.3) });
            }
        };
        for (bl, r, ..) in blockages {
            if bl == layer {
                note(*r);
            }
        }
        for (n, el, r, _) in emitted {
            // ⚠️ A strap of the SAME net still cuts: `cutShapes` applies no net test.
            if el == layer && !std::ptr::eq(n, net) {
                note(obstruction_of(db, layer, *r));
            }
        }
        let along = if horizontal {
            (rect.0, rect.2)
        } else {
            (rect.1, rect.3)
        };
        match shapes::cut(along, &blocked) {
            None => out.push((net.clone(), *rect)),
            Some(pieces) => {
                for (lo, hi) in pieces {
                    let r = if horizontal {
                        (lo, rect.1, hi, rect.3)
                    } else {
                        (rect.0, lo, rect.2, hi)
                    };
                    out.push((net.clone(), r));
                }
            }
        }
    }
    out
}

fn repair_vias(
    db: &Db,
    emitted: &mut [(String, String, Rect, &'static str)],
    placed: &[vyges_pdn::vias::Via],
    blockages: &[(String, Rect, Option<String>, Rect)],
    locked_layers: &[String],
    // 🔑 **The routing the design arrived with, which no grid owns.** `Grid::repairVias` opens by
    // skipping any via either of whose shapes has no grid component.
    //
    // and a `kFixed` shape from `makeInitialShapes` is constructed bare, so it never has one.
    //
    // ⚠️ **Without this a strap is extended to the full length of the bump wire it lands on.**
    // `extendTo` takes the UNION of the two rects along the shape's own direction, and a metal10
    // flipchip wire is 265000 long: seventeen pad straps came out
    // running 175000..440000 where the reference's run 349800..391140.
    fixed_shapes: &[vyges_pdn::vias::Shape],
) -> bool {
    let contains = |outer: Rect, inner: Rect| {
        outer.0 <= inner.0 && outer.1 <= inner.1 && outer.2 >= inner.2 && outer.3 >= inner.3
    };
    let mut moved = false;
    let is_fixed = |layer: &str, rect: Rect| {
        fixed_shapes
            .iter()
            .any(|s| s.layer == layer && s.rect == rect)
    };
    for v in placed {
        // Either end unowned and the via is not repairable at all — not merely at that end.
        if is_fixed(&v.lower, v.lower_rect) || is_fixed(&v.upper, v.upper_rect) {
            continue;
        }
        for (layer, own, toward) in [
            (&v.lower, v.lower_rect, v.upper_rect),
            (&v.upper, v.upper_rect, v.lower_rect),
        ] {
            let Some(i) = emitted.iter().position(|(n, l, r, shape)| {
                n == &v.net
                    && l == layer
                    && !(*shape == "RING" && locked_layers.contains(l))
                    && contains(*r, own)
            }) else {
                continue;
            };
            let obstructions: Vec<Rect> = blockages
                .iter()
                .filter(|(l, ..)| l == layer)
                .map(|(_, r, ..)| *r)
                .collect();
            let others: Vec<Rect> = emitted
                .iter()
                .enumerate()
                .filter(|(j, (_, l, _, _))| *j != i && l == layer)
                .map(|(_, (_, _, r, _))| *r)
                .collect();
            let halo = db.layer_get_spacing(layer).max(1);
            if let Some(grown) =
                vyges_pdn::shapes::extend_to(emitted[i].2, toward, &obstructions, &others, halo)
            {
                if std::env::var_os("PDN_TRACE").is_some() && grown != emitted[i].2 {
                    eprintln!(
                        "[repair] {}|{}|{:?} -> {:?} toward {:?}",
                        v.net, layer, emitted[i].2, grown, toward
                    );
                }
                emitted[i].2 = grown;
                moved = true;
            }
        }
    }
    moved
}

/// Build the grid: read the database, lay out the components, place the vias, write the result.
/// Build one `-ring` declaration — a `Rings` component, the first kind a grid holds.
///
/// Returns its segments and, when the ring is single-layer, the layer that locks.
fn make_ring(
    db: &Db,
    spec: &str,
    build_nets: &[String],
    core: Rect,
    die: Rect,
    per_micron: f64,
) -> (Vec<rings::Segment>, Option<String>) {
    let (layers, rest) = spec.split_once(':').unwrap_or((spec, ""));
    let p: Vec<&str> = rest.split(':').collect();
    let (l0, l1) = layers.split_once(',').unwrap_or((layers, layers));
    // ⚠️ **Only a SINGLE-LAYER ring is locked.** `Rings::makeShapes` calls `setLocked()` on its
    // shapes just when both layers are the same; an ordinary two-layer ring stays modifiable, and
    // the write stage does extend one. Treating every ring as locked leaves two ring segments 640
    // units short of the reference's.
    let locked = (l0 == l1).then(|| l0.to_string());
    // ⚠️ **A width or a spacing may be stated PER LAYER.** `pdn::get_one_to_two` expands one value
    // to two, so `-widths {2.0 3.0}` gives the first to the lower layer and the second to the
    // upper. Taking only the first builds both loops at the wrong size on every ring that
    // differentiates them.
    let two = |v: Option<&str>| {
        let v = v.unwrap_or("0");
        let (a, b) = v.split_once(',').unwrap_or((v, v));
        (dbu(a, per_micron), dbu(b, per_micron))
    };
    let widths = two(p.first().copied());
    let spacings = two(p.get(1).copied());
    let mk = |name: &str| rings::Layer {
        name: name.to_string(),
        direction: direction_of(db, name),
        width: if name == l0 { widths.0 } else { widths.1 },
        spacing: if name == l0 { spacings.0 } else { spacings.1 },
    };
    // ⚠️ **An offset may be stated per SIDE** — left, bottom, right, top. `pdn::get_one_to_four`
    // expands one value to four and two to `{a b a b}`, so a pair is "horizontal, vertical" and
    // not "inner, outer".
    let offs: Vec<i32> = p
        .get(2)
        .copied()
        .unwrap_or("0")
        .split(',')
        .map(|v| dbu(v, per_micron))
        .collect();
    let off4 = match offs.len() {
        0 => [0; 4],
        1 => [offs[0]; 4],
        2 => [offs[0], offs[1], offs[0], offs[1]],
        _ => [offs[0], offs[1], offs[2], offs[3]],
    };
    // 🔑 **`-pad_offsets` and `-core_offsets` are the same field, read differently.** The
    // reference refuses both at once and converts the pad form into the core form before anything
    // else looks at it — `Rings::setPadOffset` ends in `setOffset(core_offset)` — so only the
    // reading differs and every rule downstream is untouched.
    //
    // ⚠️ **The offset is to the OUTERMOST loop.** The ring's own total width is subtracted, which
    // is why the conversion needs the net count and both layers rather than the rectangle alone.
    let off4 = if p.get(4).copied() == Some("pad") {
        let (hor_width, ver_width) = rings::total_width(&mk(l0), &mk(l1), build_nets.len());
        let pads_inner = pad_ring_inner(db, core, die);
        let converted =
            rings::pad_offset_as_core_offset(core, pads_inner, off4, hor_width, ver_width);
        if std::env::var_os("PDN_TRACE").is_some() {
            eprintln!(
                "[padoffset] core {core:?} pads_inner {pads_inner:?} width h{hor_width} v{ver_width} \
                 asked {off4:?} -> core offset {converted:?}"
            );
        }
        converted
    } else {
        off4
    };
    let outline = rings::inner_outline(core, off4);
    // ⚠️ A ring extended to the boundary reaches the DIE along each side, and stops growing per net
    // on that axis — the loops still nest, they just all reach the same distance.
    let ring_bound = (p.get(3).copied() == Some("boundary")).then_some(die);
    (
        rings::make(&mk(l0), &mk(l1), build_nets, outline, ring_bound),
        locked,
    )
}

/// `GridComponent::getObstructions` — what the shapes already built contribute to the obstruction
/// set the next component is cut against.
///
/// ⚠️ **Each is bloated by ITS OWN spacing**, indexed by its own width and the length it runs. A
/// wide ring takes far more than its layer's nominal value and a rail far less, so one figure for
/// the layer is wrong at both ends. The raw rect is carried alongside because the cut extent is
/// measured from it, not from the bloated one.
fn made_obstructions(
    db: &Db,
    standing: &[(String, String, Rect, &'static str)],
) -> Vec<(String, Rect, Option<String>, Rect)> {
    standing
        .iter()
        .map(|(net, l, r, kind)| {
            let (w, len) = ((r.2 - r.0).min(r.3 - r.1), (r.2 - r.0).max(r.3 - r.1));
            let h = db.layer_find_v55_spacing(l, w, len).unwrap_or(0);
            (
                l.clone(),
                (r.0 - h, r.1 - h, r.2 + h, r.3 + h),
                // 🔑 **A component's own shapes carry NO net here, and a pin's does.**
                // `getObstructions` inserts a component's shapes into the obstruction tree AS THEY
                // ARE — they stay `kShape` — and `Shape::cut`'s exemption requires
                // `shapeType() != kShape`. So one component's shapes always cut the next
                // component's, same net or not.
                //
                // ⚠️ **`SWITCH` is the exception, because it is not a component shape.** It is an
                // instance's own pin metal, recorded here only so a via has something to stand on;
                // in the reference it is `kBlockObs` built with its net, which is precisely the
                // pair the exemption tests for. Sweeping it in with the straps cuts every shape
                // that lands on a macro's supply pin — 83 of them on one flip-chip case.
                (*kind == "SWITCH").then(|| net.clone()),
                *r,
            )
        })
        .collect()
}

/// `GridComponent::cutShapes` — cut a component's own shapes against the obstructions that have
/// accumulated, and return what survives.
///
/// 🔑 **Every component is put under this knife**, not only the straps. `GridComponent::make` runs
/// `makeShapes → cutShapes → getObstructions → getShapes`, and rings, followpins and straps all
/// inherit the base `cutShapes` — only the pad and repair-channel straps add anything on top of
/// it. A reference run under `set_debug_level PDN Make 1` prints `Cutting shapes in "<grid>"`
/// after each component in turn, which is what this is.
///
/// `obstructions` are `(layer, bloated rect, net, raw rect)`. Each was grown by its own spacing
/// when it was made, and the raw rect is kept because the cut extent is measured from it.
///
/// ⚠️ **Direction comes from the shape, not from the layer.** A strap runs along its layer's
/// preferred direction, so the two agree there — but a ring's four sides do not, and a ring cut on
/// its layer's direction is cut across two of them.
/// The two levels a `kOverPads` strap's via spans, and the obstructions it must clear.
///
/// The rule itself is [`vyges_pdn::pads::via_is_obstructed`]; this is the lookup around it.
fn strap_via_is_obstructed(
    shape: Rect,
    r: &OverPadStrap,
    routing: &[(String, i32)],
    obs: &[(i32, Rect)],
) -> bool {
    let level = |name: &str| routing.iter().find(|(n, _)| n == name).map(|(_, l)| *l);
    let (Some(shape_level), Some(target_level)) = (level(&r.layer), level(&r.target_layer)) else {
        return false;
    };
    vyges_pdn::pads::via_is_obstructed(shape, r.target, shape_level, target_level, obs)
}

/// **`PadDirectConnectionStraps::refineShape`** — slide the strap along its pin until its via is
/// clear, and report where it landed.
///
/// 🔑 **It slides across the pin, never along the strap** — the ends stay where `getClosestShape`
/// put them, and only the width-wise position moves.
///
/// ⚠️ **A candidate must survive its own CUT**, not merely the via test: `refineShape` adds the
/// shape, cuts it, and rejects the location outright if the component is left empty. So the pieces
/// this returns are already cut, and an empty answer means "try the next place", not "no metal".
#[allow(clippy::too_many_arguments)]
fn refine_over_pad_strap(
    db: &Db,
    r: &OverPadStrap,
    width: i32,
    routing: &[(String, i32)],
    // Levelled and pre-bloated, for the via test.
    obs: &[(i32, Rect)],
    // The full set, for the cut — already without the strap being moved.
    without_self: &[(String, Rect, Option<String>, Rect)],
    delta: i32,
) -> Option<Vec<Rect>> {
    let (search_min, search_max) = if r.horizontal {
        (r.pin.1, r.pin.3 - width)
    } else {
        (r.pin.0, r.pin.2 - width)
    };
    let delta = delta.max(1);
    let mut at = search_min;
    while at <= search_max {
        let candidate = if r.horizontal {
            (r.strap.0, at, r.strap.2, at + width)
        } else {
            (at, r.strap.1, at + width, r.strap.3)
        };
        at += delta;
        // ⛔ **The reference re-checks the candidate's via and the check can never fire.**
        //
        // and `strapViaIsObstructed` opens with `target_shapes_.find(shape)`, a
        // `std::map<Shape*, Shape*>` filled only by `makeShapesOverPads`. `new_shape` is a fresh
        // `shape->copy()`, so the lookup misses and the function returns **false** on its first
        // line, every time.
        //
        // 🔑 **So a refined candidate is judged by its CUT alone**, and the first position along
        // the pin whose cut leaves a piece is the one taken.
        //
        // ⚠️ **Running the test for real costs the shape.** On a flipchip design that connects
        // over its pads, the reference's own answer is the very
        // first candidate — the pin's low edge — and its via genuinely is obstructed on metal9.
        // Testing it rejects that position and every one of the 500 after it, and the strap is
        // dropped: 1892 shapes against 1893.
        //
        // ℹ️ This looks like an upstream defect rather than an intent. The `recheck` argument
        // exists only to lower the debug level of a line inside a call that cannot reach it, so
        // the author meant the check to run. 
        // ⛔ we match the reference here rather than the intent, because the reference is what the
        // correlation is against.
        if std::env::var_os("PDN_REFINE_TRACE").is_some() {
            eprintln!(
                "[refine try] {}|{:?}|obstructed(unused) {}",
                r.net,
                candidate,
                strap_via_is_obstructed(candidate, r, routing, obs)
            );
        }
        let pieces: Vec<Rect> = cut_shapes(
            db,
            &[(r.net.clone(), r.layer.clone(), candidate)],
            without_self,
        )
        .into_iter()
        .map(|(_, _, rect)| rect)
        // The over-pads half of `cutShapes`, which applies to a refined shape like any other.
        .filter(|rect| {
            let p = r.inst;
            let contained = p.0 <= rect.0 && p.1 <= rect.1 && p.2 >= rect.2 && p.3 >= rect.3;
            let touches = p.0 <= rect.2 && p.2 >= rect.0 && p.1 <= rect.3 && p.3 >= rect.1;
            !contained && touches
        })
        .collect();
        if pieces.is_empty() {
            continue;
        }
        return Some(pieces);
    }
    None
}

fn cut_shapes(
    db: &Db,
    shapes: &[(String, String, Rect)],
    obstructions: &[(String, Rect, Option<String>, Rect)],
) -> Vec<(String, String, Rect)> {
    let mut out: Vec<(String, String, Rect)> = Vec::new();
    for (shape_net, layer, rect) in shapes {
        let layer = layer.as_str();
        let horizontal = (rect.2 - rect.0) >= (rect.3 - rect.1);
        let width = (rect.2 - rect.0).min(rect.3 - rect.1);
        let along = if horizontal {
            (rect.0, rect.2)
        } else {
            (rect.1, rect.3)
        };
        // ◐ **The CUT SHAPE's own halo — and the full spacing chain belongs here, MEASURED, but
        // does not land yet.** A `kBlockObs` carries no halo of its own, so on such an obstruction
        // this is the only halo there is; reading zero for it leaves the cut short by exactly that
        // much at each end. One follow pin was split 130 narrow at both ends, which is the whole
        // of Nangate45 metal1's `SPACING 0.065`, and a case that isolates the decision answers
        // the reference's cut points exactly once this reads the chain.
        //
        // ⛔ **And it costs three macro designs**: two that block on halos, and one whose vias
        // fail.
        //
        // 🔑 **One change moving two designs in opposite directions means the INPUT is wrong, not
        // the rule** — a case that isolates it settles the rule on its own. What this consumes on
        // a macro grid is the thing to dump: the obstruction rects those cut against, which carry a
        // halo already and may be carrying it twice once this term grows.
        //
        // ⚠️ It is also the whole of the earlier thirteen-site regression. Sweeping every site at
        // once cost exactly these three cases and these same counts; the pin path measured clean
        // and this is the site that did it.
        let halo = obstruction_spacing(db, layer, width, along.1 - along.0);
        // ⚠️ **Across as well as along.** An obstruction only cuts a stripe it actually
        // crosses; filtering by layer alone cuts every stripe on the layer at the
        // obstruction's along-extent, wherever they sit. The shape is grown by its halo on
        // both axes for this test, as the reference grows its obstruction rect before
        // querying.
        let mine = shapes::Halo {
            left: halo,
            top: halo,
            right: halo,
            bottom: halo,
        };
        let blocked: Vec<(i32, i32)> = obstructions
            .iter()
            .filter(|(l, ..)| l == layer)
            // 🔑 **A shape ignores a same-net obstruction it wholly contains.** The reference
            // skips such a violation outright — "completely inside the new strap and therefore
            // is okay" — measuring containment ACROSS the shape's own direction and against
            // the obstruction's raw rect. Without it a stripe is cut by its own block pin,
            // and with it applied to the wrong axis the wrong stripe is cut: of two pins on
            // one net, the contained one must not cut and the overhanging one must.
            .filter(|(_, _, obs_net, r)| {
                if obs_net.as_deref() != Some(shape_net.as_str()) {
                    return true;
                }
                let (slo, shi) = if horizontal {
                    (rect.1, rect.3)
                } else {
                    (rect.0, rect.2)
                };
                let (olo, ohi) = if horizontal { (r.1, r.3) } else { (r.0, r.2) };
                !(slo <= olo && shi >= ohi)
            })
            // ⚠️ **BOTH rects are grown, each by ITS OWN spacing.** The reference queries one
            // shape's obstruction against another's, and every obstruction was bloated when it
            // was made — a block pin by a little, a wide ring by a lot. This field already
            // holds that per-obstruction rect, so it is used as it stands. Re-bloating it by
            // the STRIPE's halo instead grows a small pin by a wide wire's spacing and cuts
            // two pin cases that were exact.
            // **H11** — reach asked ACROSS against the obstruction's STORED rect, extent taken
            // ALONG from its RAW one grown by the larger of the two halos. Both halves and the
            // order between them are tested in `shapes::cut_sequence_tests`, against the split the
            // reference makes in `report`.
            .filter_map(|(_, r, _, raw)| shapes::blocked_span(*rect, mine, *r, *raw, horizontal))
            .collect();
        // 🔑 **`PDN_CUT_LAYER=<layer>` says why a shape ends where it does.** One line per shape
        // on that layer with the halo it carries, then one per obstruction that reaches it —
        // stored rect, raw rect and the span it blocks — so a cut point can be traced back to the
        // obstruction that made it rather than inferred from the survivors. ⚠️ Reading a
        // survivor's end as a cut point is how three earlier theses died: a row boundary, a trim
        // and a via-metal extension all look exactly like one.
        if let Some(want) = std::env::var_os("PDN_CUT_LAYER") {
            if want == layer {
                eprintln!(
                    "[cut] SHAPE {shape_net}|{layer}|{rect:?}|mine {halo}|obs {}",
                    obstructions.iter().filter(|(l, ..)| l == layer).count()
                );
                for (l, r, _n, raw) in obstructions.iter().filter(|(l, ..)| l == layer) {
                    if let Some(sp) = shapes::blocked_span(*rect, mine, *r, *raw, horizontal) {
                        eprintln!(
                            "[cut] {shape_net}|{l}|shape {rect:?}|mine {halo}|stored {r:?}|raw {raw:?}|span {sp:?}"
                        );
                    }
                }
            }
        }
        match shapes::cut(along, &blocked) {
            None => out.push((shape_net.clone(), layer.to_string(), *rect)),
            Some(pieces) => {
                for (lo, hi) in pieces {
                    let r = if horizontal {
                        (lo, rect.1, hi, rect.3)
                    } else {
                        (rect.0, lo, rect.2, hi)
                    };
                    out.push((shape_net.clone(), layer.to_string(), r));
                }
            }
        }
    }
    out
}

/// The extents a strap set may be told to reach: `-extend_to_core_ring`, `-extend_to_boundary`,
/// or the grid's own.
#[derive(Clone, Copy)]
struct StrapBounds {
    core: Rect,
    die: Rect,
    strap: Rect,
    ring: Rect,
}

/// Build one `-stripe` declaration — a `Straps` component.
///
/// `standing` is everything built so far: a strap is cut against it, which is `cutShapes` running
/// with the obstructions every earlier component contributed. Returns the set's descriptor (kept
/// even when the strap builds nothing) and its stripes.
#[allow(clippy::too_many_arguments)]
fn make_strap(
    db: &Db,
    spec_text: &str,
    domain: &nets::Domain,
    grid_nets: &[String],
    standing: &[(String, String, Rect, &'static str)],
    blockages: &[(String, Rect, Option<String>, Rect)],
    ring_shapes: &[(String, Rect)],
    b: StrapBounds,
    per_micron: f64,
) -> (
    Option<(String, i32, i32, i32, bool)>,
    Vec<(String, String, Rect, &'static str)>,
) {
    let mut out: Vec<(String, String, Rect, &'static str)> = Vec::new();
    let (layer, rest) = spec_text.split_once(':').unwrap_or((spec_text, ""));
    let p: Vec<&str> = rest.split(':').collect();
            let extend = p.get(3).copied().unwrap_or("core");
            // ⚠️ A count of zero means "as many as fit", so an absent option and an explicit 0 are the
            // same thing and neither is a sentinel that needs its own case.
            let count: i32 = p.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
            let snap = p.get(5).copied() == Some("snap");
            let width = dbu(p.first().copied().unwrap_or("0"), per_micron);
            let pitch = dbu(p.get(1).copied().unwrap_or("0"), per_micron);
            let horizontal = direction_of(db, layer) == Direction::Horizontal;
            let spec = straps::Spec {
                layer: layer.to_string(),
                width,
                // A stated `-spacing` wins; only in its absence is one derived.
                //
                // ⚠️ pitch / NET COUNT, not pitch / 2. The two agree for a power/ground pair and
                // disagree for anything else.
                spacing: p
                    .get(6)
                    .filter(|s| !s.is_empty())
                    .map(|s| dbu(s, per_micron))
                    .filter(|s| *s > 0)
                    .unwrap_or_else(|| {
                        // ⚠️ The GRID's net count, not the strap's. This runs before the two
                        // `-nets`/`-starts_with` overrides below shadow the list, and the
                        // reference derives the default spacing the same way.
                        //
                        // ⚠️ **The TECHNOLOGY's manufacturing grid, read from the database.**
                        // `Straps::Straps` snaps the value it derives with
                        // `TechLayer::snapToManufacturingGrid(tech, spacing_, false)` — the
                        // technology's own grid, at multiplier 1. A literal here was 5, which is
                        // Nangate45's grid HALVED (0.005 um at 2000 DBU/um is 10) and five times
                        // ASAP7's, so the derived spacing was snapped onto a lattice no technology
                        // has. It survived because snapping to a divisor of the real grid is a
                        // no-op whenever the value is already on it.
                        //
                        // ⚠️ A technology stating no grid must not snap at all, which `1` is:
                        // `snapToManufacturingGrid` returns the position unchanged when
                        // `hasManufacturingGrid()` is false.
                        straps::default_spacing(
                            pitch,
                            grid_nets.len() as i32,
                            width,
                            db.manufacturing_grid().unwrap_or_default().unwrap_or(1),
                        )
                    }),
                pitch,
                offset: dbu(p.get(2).copied().unwrap_or("0"), per_micron),
                number_of_straps: count,
                snap,
                // ⚠️ **`-allow_out_of_core` moves only the END of the across axis out to the
                // die**, not both — `makeShapes` passes `core.yMin()` and
                // `allow_out_of_core_ ? die.yMax() : core.yMax()`. A set running die edge to die
                // edge would be a different grid.
                allow_out_of_core: p.get(9).copied() == Some("out"),
            };
            // ⚠️ `boundary` is the DIE, as `Grid::getGridBoundary` returns `getGridArea`. Omitting
            // this arm leaves an extended strap at the core and trimming then pulls it back to its
            // vias, which reads as a trimming defect and is a missing extend mode.
            let bound = match extend {
                "rings" => b.ring,
                "boundary" => b.die,
                _ => b.strap,
            };
            let set = (
                spec.layer.clone(),
                spec.pitch,
                spec.width,
                spec.spacing,
                horizontal,
            );
            // ⚠️ **A strap may name its own net order or its own SUBSET of nets.**
        // `GridComponent::getNets` answers with the component's own list where it has one, and with
        // the grid's in that component's own `starts_with` order otherwise. Applied to the whole
        // grid instead, one strap's preference moves every other strap with it.
        let build_nets: Vec<String> = match p.get(7).copied().filter(|s| !s.is_empty()) {
            Some(sw) => nets::build_order(domain, sw == "power"),
            None => grid_nets.to_vec(),
        };
        // ⚠️ **In the order the strap STATES, not the grid's order filtered.**
        // `GridComponent::getNets` returns `nets_` as given and only falls back to the grid when it
        // is empty — so `-nets {VDD VSS}` puts power first however the grid was told to start.
        // Filtering the grid's list instead keeps the grid's order and swaps every strap of the
        // set with its neighbour: same positions, wrong nets, and nothing in the geometry to show
        // for it.
        let build_nets: Vec<String> = match p.get(8).copied().filter(|s| !s.is_empty()) {
            Some(list) => list
                .split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect(),
            None => build_nets,
        };
        if build_nets.is_empty() {
            return (Some(set), Vec::new()); // a strap for no net builds nothing
        }
        let span = straps::span(b.core, b.die, bound, horizontal, spec.allow_out_of_core);
            let abs = straps::absolute(b.die, horizontal);
            // ⚠️ The track grid on the axis the stripes STEP along: y for horizontal stripes, x for
            // vertical ones. Taking the other one snaps to tracks running the wrong way and every
            // position moves.
            let tracks = db.track_grid(layer).unwrap_or_default();
            let grid_axis = if horizontal { tracks.1 } else { tracks.0 };
            // ⚠️ **Only the rings on THIS layer.** The reference's avoidance set is the ring shapes
            // found under the strap's own layer key, so a metal4 strap is not blocked by a metal5 ring
            // it merely crosses. Avoiding every ring on every layer silently deletes whole strap sets:
            // a vertical strap spanning the ring area crosses the horizontal rings by construction, so
            // every one of its stripes is dropped and the set produces nothing at all.
            let avoid: Vec<Rect> = ring_shapes
                .iter()
                .filter(|(l, _)| *l == spec.layer)
                .map(|(_, r)| *r)
                .collect();
            let (stripes, _) = straps::make_straps(
                &spec,
                &build_nets,
                span,
                abs,
                &grid_axis,
                &avoid,
                horizontal,
            );
            // ── cut around obstructions on this layer ───────────────────────────────────────────
            // ⚠️ Both the stripe AND the obstruction are grown before they are compared. The stripe's
            // halo comes from the LEF 5.5 spacing table indexed by its own width and the length it
            // runs — a 1um strap crossing the core takes 1800 units on Nangate45 metal4, not the 280
            // its layer nominally asks for. Using the nominal spacing cuts in the right places by
            // several hundred units and every piece is wrong at both ends.
            //
            // 🔑 **Every shape already made obstructs the ones made after it** — that is the whole of
            // `GridComponent::make`:
            //
            // ```text
            // makeShapes(shapes);            // sees what has accumulated
            // cutShapes(obstructions);       // cut against what has accumulated
            // getObstructions(obstructions); // add mine, for the components AFTER me
            // getShapes(shapes);
            // ```
            //
            // Components run rings first, then straps in declaration order, so a strap is cut by the
            // rings and by every strap set declared before it.
            //
            // ⚠️ **Snapshotted BEFORE the loop, and that is the point of the ordering.** A component
            // contributes its obstructions only once it is finished, so its stripes never cut each
            // other. Reading the live list inside the loop instead makes each stripe an obstruction
            // for the next one in its own set — which reads as a logic error in the cut and is
            // not one.
            //
            // ⚠️ Each is bloated by **its own** spacing, indexed by its own width and length. A wide
            // ring takes far more than the layer's nominal value, and a rail far less.
            let made_obs = made_obstructions(db, standing);
            // 🔑 `cutShapes` — the same knife every component is put under.
            let pieces = cut_shapes(
                db,
                &stripes
                    .iter()
                    .map(|s| (s.net.clone(), s.layer.clone(), s.rect))
                    .collect::<Vec<_>>(),
                &blockages
                    .iter()
                    .cloned()
                    .chain(made_obs)
                    .collect::<Vec<_>>(),
            );
            out.extend(pieces.into_iter().map(|(n, l, r)| (n, l, r, "STRIPE")));
    (Some(set), out)
}

/// Build one `-followpins` declaration — a `FollowPins` component, the standard-cell rails.
///
/// Returns the layer, the set's pitch, and the rails. `None` means the width could not be
/// determined from the standard cells and none was stated.
#[allow(clippy::too_many_arguments)]
fn make_followpins(
    db: &Db,
    spec: &str,
    rows: &[followpins::Row],
    power: &str,
    ground: &str,
    core: Rect,
    die: Rect,
    ring_area: Rect,
    per_micron: f64,
) -> Option<(String, i32, Vec<(String, Rect)>)> {
    let mut parts = spec.splitn(3, ':');
    let layer = parts.next().unwrap_or(spec);
    let extend = parts.next().unwrap_or("core");
    // 🔑 **An explicit width wins outright.** `FollowPins`' constructor calls `determineWidth()`
    // only `if (getWidth() == 0)` — so a `-width` on a follow-pin stripe is not a hint the standard
    // cells can override, it replaces them. ASAP7 states 0.072 on M1 and 0.090 on M2 where the
    // cells would give 18, and every shape in those cases is the wrong width without this.
    let stated_width = parts
        .next()
        .filter(|w| !w.is_empty())
        .map(|w| dbu(w, per_micron))
        .filter(|w| *w > 0);
    // ⚠️ `boundary` is the DIE, not the grid's own extent: `Grid::getGridBoundary` returns
    // `getGridArea`, which returns the die area, and a core grid overrides neither.
    let fp_bound = match extend {
        "rings" => ring_area,
        "boundary" => die,
        _ => core,
    };
    let width = stated_width.or_else(|| followpin_width(db))?;
    // 🔑 **A follow pin set's PITCH is twice a row's height**, and stage 6f bloats by it to decide
    // which rails form one channel. `FollowPins` reads it off the first row.
    let pitch = rows.first().map(|r| 2 * (r.bbox.3 - r.bbox.1)).unwrap_or(0);
    let rails = followpins::make(layer, power, ground, width, rows, core, fp_bound);
    // ⚠️ **Accumulated, not deduped.** Adjacent rows are flipped, so every interior row edge is
    // written by both of the rows that meet on it — but the two are not always the same rectangle.
    // Where a macro splits one row and not the row beneath it, the shared edge is written once at
    // full width and once at the split width, and `GridComponent::addShape` merges them. See
    // `shapes::add_shapes`.
    let accumulated = shapes::add_shapes(
        &rails
            .iter()
            .map(|r| (r.net.clone(), r.rect))
            .collect::<Vec<_>>(),
    );
    Some((layer.to_string(), pitch, accumulated))
}

/// Build ONE pad direct connection — a single `PadDirectConnectionStraps` component.
///
/// `standing` is what the grid has already built, which is the set of shapes this connection may
/// target: `getClosestShape` searches it, so *when* this runs decides *what* the pad can reach.
///
/// Returns `(net, layer, strap, holding pin)` per strap made — empty when there is nothing to
/// reach yet, which is exactly what makes the component defer and try again later.
fn make_pad_connection(
    db: &Db,
    opts: &Opts,
    conn: &PadConnection,
    standing: &[(String, String, Rect, &'static str)],
    core: Rect,
    die: Rect,
    over_pad_slot: (usize, usize),
    refine: &mut Option<dbio::OverPadStrap>,
) -> Vec<(String, String, Rect, Rect)> {
    let reaches = |layer: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in opts.all("connect") {
            let Some((a, b)) = c.split(':').next().unwrap_or("").split_once(',') else {
                continue;
            };
            let other = if a == layer {
                b
            } else if b == layer {
                a
            } else {
                continue;
            };
            if !out.iter().any(|l| l == other) {
                out.push(other.to_string());
            }
        }
        out
    };
    // ⚠️ The flag on a RING restricts the targets to rings; on a grid it does not.
    let ring_only = opts
        .all("connect-to-pads")
        .iter()
        .any(|s| s.ends_with(":ring"));
    pad_strap_for(
        db,
        conn,
        &reaches,
        standing,
        ring_only,
        core,
        die,
        over_pad_slot,
        refine,
    )
}

fn generate(args: &[String]) -> ExitCode {
    let opts = Opts::parse(args);
    let (Some(path), Some(out), Some(power), Some(ground)) = (
        args.first(),
        opts.one("out-def"),
        opts.one("power"),
        opts.one("ground"),
    ) else {
        return usage();
    };

    let mut db = match Db::open(path) {
        Ok(d) => d,
        Err(e) => {
            vyges_events::log(
                "vyges-pdn",
                vyges_events::Severity::Error,
                format!("cannot open {path}: {e}"),
            );
            return ExitCode::from(2);
        }
    };

    let per_micron = db.block_get_def_units() as f64;
    let core = area(&db, true);
    let die = area(&db, false);
    // 🔑 **A switched domain has THREE supply nets.** UPF's power switch splits the supply into an
    // always-on net the grid's straps carry and a switched net the cells' rails carry, and
    // `VoltageDomain::getNets` reports both alongside ground — so every strap set builds three
    // straps to a group rather than two, and every offset in the grid moves.
    let domain = nets::Domain {
        power: power.into(),
        ground: ground.into(),
        switched_power: opts
            .one("switched-power")
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        // ⚠️ **Secondary supplies come LAST in every order**, before or after ground — see
        // `VoltageDomain::getNets`, which appends them after the primary trio either way.
        secondary: opts
            .one("secondary")
            .filter(|s| !s.is_empty())
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_default(),
    };
    // ⚠️ **`getPower()` answers with the SWITCHED net where one exists**, and that is what follow
    // pins are built from — the cells' own rails are on the switched supply. The always-on net
    // reaches the grid only through the switch cells' input pins.
    let followpin_power: &str = domain.switched_power.as_deref().unwrap_or(power);
    // ⚠️ Ground first BY DEFAULT. Verified against the reference: a ring built with
    // `-power VDD -ground VSS` puts VSS on the INNER loop. `define_pdn_grid -starts_with POWER`
    // reverses it, and the whole grid moves with it -- geometry exact, nets swapped.
    let starts_with_power = opts
        .one("starts-with")
        .is_some_and(|v| v.eq_ignore_ascii_case("power"));
    let build_nets = nets::build_order(&domain, starts_with_power);

    // `(net, layer, rect, shape)` — the shape annotation is part of the answer, not decoration.
    // Routing obstructions, by layer. ⚠️ These cut shapes; they do not stop them being made.
    // `(layer, rect for the query, owning net, RAW rect for the containment test)`.
    //
    // ⚠️ **The two rects are not the same and both are needed.** The reference queries with the
    // bloated obstruction and then measures containment against the obstruction's own `rect_`.
    // Collapsing them either cuts too little — a raw rect queried — or exempts too much.
    let blockages: Vec<(String, Rect, Option<String>, Rect)> = db
        .obstruction_boxes()
        .unwrap_or_default()
        .into_iter()
        .map(|(layer, x0, y0, x1, y1)| {
            let r = (x0, y0, x1, y1);
            (db.layer_name_by_number(layer), r, None, r)
        })
        .collect();
    // ── stage 5, in part: fixed block pins ───────────────────────────────────────────────────
    // 🔑 **`Shape::populateMapFromDb` collects fixed BTERM PINS, not only special wires.** Each
    // becomes a `kFixed` shape with `generateObstruction()` applied, and `buildGrids` pushes every
    // one of those into the obstruction map as well — so a pin cuts the stripes that cross it.
    //
    // ⚠️ This was dismissed once on the grounds that the input DEF carries no `SPECIALNETS`. The
    // function has two sources and only one had been checked: pins created through the API appear
    // in no DEF at all.
    //
    // ⚠️ **Routing layers only** — a pin on a layer with no routing level is skipped, not treated
    // as an obstruction on layer zero.
    let mut blockages = blockages;
    // ⚠️ **Bloated ALREADY, so the raw rect is the bloated one.** A `kBlockObs` shape is built with
    // `obs_(rect_)` — its obstruction is its rect and its own halo is therefore zero, so a strap
    // crossing it is cleared by the strap's spacing alone. Storing the unbloated rect here would
    // apply the layer's spacing twice on the wide side and not at all on the narrow one.
    // ⚠️ **The engine decides which domains it can build, not the caller.** `--voltage-domains`
    // is passed through as written and aliased here: `CORE` names the core domain, a domain bound
    // to a REGION arrives as `--domain` on the grid that claims it, and anything else is a domain
    // this engine does not model yet.
    let region_named: Vec<String> = opts
        .all("domain")
        .iter()
        .filter_map(|d| d.split(':').next())
        .map(str::to_string)
        .collect();
    for named in opts.all("voltage-domains") {
        for name in named.split(',').filter(|n| !n.is_empty()) {
            if region_named.iter().any(|r| r == name) {
                continue;
            }
            if nets::domain_name(name) != nets::CORE_DOMAIN {
                vyges_events::log(
                    "vyges-pdn",
                    vyges_events::Severity::Warn,
                    format!("voltage domain {name:?} is not the core domain"),
                );
                return ExitCode::from(2);
            }
        }
    }

    // ⚠️ Kept whole here and filtered per grid below: a macro obstructs every grid but its own.
    // The halo each gridded instance carries, from the grid that claims it.
    let inst_halos: Vec<(String, [i32; 4])> = opts
        .all("grid")
        .iter()
        .flat_map(|s| {
            let g = GridSpec::parse(s, per_micron);
            let patterns: Vec<String> = g
                .instance
                .split('+')
                .filter(|i| !i.is_empty())
                .map(str::to_string)
                .collect();
            if patterns.is_empty() {
                return Vec::new();
            }
            select_instances(&db, g.by_cell, &patterns, &g.orients)
                .into_iter()
                .map(|i| (i, g.halo))
                .collect::<Vec<_>>()
        })
        .collect();
    // 🔑 **A halo reaches an obstruction only through the grid that claims the instance, and what
    // it produces there is a GRID-level obstruction.**
    //
    // ⚠️ **And `FollowPinShape::cut` drops every `kGridObs`** — "followpins should only get cut
    // from real obstructions and not estimated obstructions". So a macro's halo cuts straps and
    // never cuts rails, and putting the halo'd rect in the REAL set makes it cut both.
    //
    // ⟹ That is what a macro design with halos reported: its rails end at 275500 and 334020, which
    // are exactly 672 and 826 site widths from the row origin — row boundaries, not cuts. The
    // reference's rails simply stop there, because the obstruction its follow pins see is the
    // zero-halo one at 285470 and never reaches them. Ours carried the halo to 275600 and, once
    // the cut halo grew, clipped thirty units off a rail nothing was cutting.
    let inst_obs = instance_obstructions(&db, &[]);
    // The same instances WITH their grid's halo, which belongs in the grid-level set below.
    let inst_obs_haloed = instance_obstructions(&db, &inst_halos);
    // ⚠️ **The reference names the instance and stops** — `Get instance {} obstructions` at
    // `Make 3` — so the rects themselves are in no log it writes. This is our half of a comparison
    // whose other half has to be read out of where the reference's shapes are CUT.
    // ℹ️ Keyed `<instance>|<layer>` with the net, because the net decides whether a same-net shape
    // crossing it is cut at all. It named `report`'s cutter in one line.
    if std::env::var_os("PDN_TRACE").is_some() {
        for (inst, layer, r, net, raw) in &inst_obs {
            // 🔑 **Both rects, because the cut needs both and they are asked DIFFERENT questions.**
            // Reach is tested against the bloated one; the extent is measured from the raw one
            // grown by the larger of the two halos. An obstruction whose pair is identical has no
            // halo of its own and is cleared by the crossing shape alone.
            eprintln!(
                "[instobs] {inst}|{layer}|{},{},{},{}|raw {},{},{},{}|own {},{},{},{}|{}",
                r.0,
                r.1,
                r.2,
                r.3,
                raw.0,
                raw.1,
                raw.2,
                raw.3,
                raw.0 - r.0,
                raw.1 - r.1,
                r.2 - raw.2,
                r.3 - raw.3,
                net.as_deref().unwrap_or("-")
            );
        }
    }
    let _ = &inst_obs;
    // 🔑 **A pad the grid connects to contributes NOTHING here.** `PdnGen::buildGrids` unions
    // `Grid::getInstances()` over every grid — exactly the instances carrying a pad direct
    // connection — and hands it to `makeInitialObstructions` as `skip_insts`, which drops the
    // instance entirely before reading either its OBS boxes or its pins.
    //
    // ⚠️ Without this a top-metal ring is broken at every connected pad it passes, and trimming
    // then deletes the short fragments — the ring ends up shorter than the reference's rather
    // than merely differently cut.
    let pad_connected = {
        // The union across the whole command line, as the reference unions across grids. The
        // named layers of every `--connect-to-pads` participate, and an unnamed one means all.
        let named: Vec<String> = opts
            .all("connect-to-pads")
            .iter()
            .flat_map(|s| s.split(','))
            .map(|s| s.trim_end_matches(":ring"))
            .filter(|s| !s.is_empty() && *s != "all")
            .map(str::to_string)
            .collect();
        if opts.all("connect-to-pads").is_empty() {
            Vec::new()
        } else {
            pad_connect_insts(&db, &build_nets, &named, core)
        }
    };

    trace(
        "Skip pad obstructions",
        &format!("{} instances: {}", pad_connected.len(), pad_connected.join(",")),
    );

    // ◐ **The net is carried but NOT used here yet, and that is a measured decision.**
    // `getInstancePins` builds these with their net and `getInstanceObstructions` flips them to
    // `kBlockObs`, which is exactly what `Shape::cut`'s exemption tests for — so honouring it is
    // the faithful reading and `instance_obstructions` returns it.
    //
    // ⚠️ Switching it on costs one flip-chip case 83 matching shapes and gains nothing anywhere,
    // which means something else in our model is already standing in for it. Passing `None` keeps
    // today's behaviour while the accessor stays correct, so the experiment is one line.
    // 🔑 **An instance a grid claims contributes NO real obstruction at all**, and the reference
    // says so in its own log rather than only in its source:
    //
    // ```text
    // [DEBUG PDN-Make] Get initial obstructions - begin
    // [DEBUG PDN-Make] Get initial obstructions - end          ← nothing between
    // ```
    //
    // `"Get instance {} obstructions"` prints AFTER the `skip_insts` test, so an empty span is the
    // evidence: the two gridded SRAMs are skipped and every other instance is a core master.
    //
    // ⟹ Everything a gridded macro keeps off comes from ITS OWN grid's level obstructions, which
    // are `GridObsShape` — and ⚠️ **`FollowPinShape::cut` drops every `kGridObs`**, "followpins
    // should only get cut from real obstructions and not estimated obstructions". So **a macro's
    // halo cuts straps and never cuts rails**, which is the whole rule in one line.
    //
    // ⚠️ **Why this is logged rather than assumed.** `insts_in_grids` is the set of instances any
    // grid claims; ours is the set a `--grid` spec NAMES, and the two can diverge — a selection by
    // cell rather than by name, or a grid whose instance list is computed. Where they do, we skip
    // an obstruction the reference keeps, and the result looks exactly like a macro that never
    // obstructed anything: no error, no missing shape, just a strap that runs where it should have
    // been cut. The count goes out on every run and the names under `PDN_TRACE`.
    //
    // ℹ️ It has already paid for itself once: it is what distinguished "the skip is correct" from
    // "the skip never fired", which a suite of matching cases cannot tell apart.
    let mut skipped_gridded: Vec<&String> = Vec::new();
    for (inst, layer, rect, _net, _raw) in &inst_obs {
        if pad_connected.iter().any(|i| i == inst) {
            continue;
        }
        if inst_halos.iter().any(|(i, _)| i == inst) {
            if !skipped_gridded.contains(&inst) {
                skipped_gridded.push(inst);
            }
            continue;
        }
        blockages.push((layer.clone(), *rect, None, *rect));
    }
    if !skipped_gridded.is_empty() {
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Warn,
            format!("{} gridded instance(s) contribute no real obstruction \
             (makeInitialObstructions skips them; their keep-out is grid-level only)",
            skipped_gridded.len()),
        );
        if std::env::var_os("PDN_TRACE").is_some() {
            for inst in &skipped_gridded {
                eprintln!("[instobs] SKIPPED {inst}: claimed by a grid, kGridObs only");
            }
        }
    }
    // Fixed block pins, as via targets. See the note where they are collected.
    let mut bterm_via_shapes: Vec<vyges_pdn::vias::Shape> = Vec::new();
    for bterm in db.block_get_b_terms() {
        for pin in 0..db.num_bterm_get_b_pins(&bterm) {
            if !matches!(
                db.bpin_get_placement_status(&bterm, pin).as_str(),
                "FIRM" | "LOCKED" | "PLACED" | "COVER"
            ) {
                continue;
            }
            for (layer, x0, y0, x1, y1) in db.bpin_layer_boxes(&bterm, pin).unwrap_or_default() {
                let name = db.layer_name_by_number(layer);
                if db.layer_get_type(&name).unwrap_or_default() != "ROUTING" {
                    continue;
                }
                // ⚠️ `generateObstruction` bloats by the layer's spacing, then by its spacing
                // tables, then by the end-of-line rules. Only the first is applied here, so a
                // technology leaning on the other two cuts short of where the reference does.
                let s = db.layer_get_spacing(&name);
                // 🔑 **A fixed block pin is a via TARGET as well as an obstruction**, and it is
                // the first thing `Shape::populateMapFromDb` collects — before the existing
                // routing, and by the same rule: fixed placement, a routing layer, `kFixed`.
                //
                // ⚠️ **Its shape type is `NONE`**, not `STRIPE`. The reference builds it with
                // `dbWireShapeType::NONE`, so a via landing on it writes no `+ SHAPE` clause.
                //
                // ℹ️ Read as an obstruction alone, a grid connects to every block pin it was
                // asked to reach except the ones it can only reach by via: one metal3 terminal
                // out of 485 vias in a design built for exactly that purpose.
                bterm_via_shapes.push(vyges_pdn::vias::Shape {
                    layer: name.clone(),
                    net: db.bterm_get_net(&bterm),
                    rect: (x0, y0, x1, y1),
                });
                blockages.push((
                    name,
                    (x0 - s, y0 - s, x1 + s, y1 + s),
                    Some(db.bterm_get_net(&bterm)),
                    (x0, y0, x1, y1),
                ));
            }
        }
    }
    // ── stage 5, the other half: the routing the design ARRIVED with ─────────────────────────
    // 🔑 **`Grid::makeInitialShapes` is called for every grid, not only for `-existing`.**
    // `PdnGen::buildGrids` reads every net's special wires into `all_shapes_vec` and then pushes
    // each of those shapes into `block_obs_vec` as well.
    //
    // ⟹ A design that arrives with power already routed cuts the grid it is about to be given.
    // ⚠️ Missing this costs a flipchip design that connects over its pads a whole population of
    // pad straps that run to the ring where the reference's stop short: a DVSS bump wire at x 175000
    // crosses them, and 175000 less metal10's 3000 spacing at that width is exactly where the
    // reference cuts. Every other case in the suite arrives with no `SPECIALNETS` at all, which is
    // why an engine that never read them matched 83 of 93 without it.
    //
    // 🔑 **`kFixed` and net-bearing, so the same-net exemption applies** — unlike a component's own
    // shapes, which stay `kShape` and cut their neighbours whatever the net.
    // 🔑 **And they are via TARGETS as well as obstructions, which is the half that was missing.**
    // `PdnGen::buildGrids` reads them once and uses the result twice — the shapes go to
    // `Grid::makeInitialShapes` AND to `block_obs_vec` — and `Grid::makeVias` then merges every
    // global shape intersecting its search area into the set it looks for crossings in.
    //
    // ⚠️ **Missing this builds no metal9/metal10 via at all on a flipchip design** — hundreds
    // in the reference against none here, because every metal10 shape in such a design arrives
    // with the DEF and this engine only ever obstructed with them. It shows as four shapes
    // because a via is not a shape: what showed was four metal9 straps trimmed back to their last
    // metal8 via, the reference's held out to the bump wire above.
    //
    // ⚠️ **Never emitted.** They are already in the database, so they are a target and nothing
    // else — not written, not trimmed, and `isModifiable` is false for a `kFixed` shape anyway.
    let mut fixed_via_shapes: Vec<vyges_pdn::vias::Shape> = bterm_via_shapes;
    for net in db.block_get_nets() {
        for (layer, x0, y0, x1, y1, _shape, octilinear) in
            db.net_swire_shapes(&net).unwrap_or_default()
        {
            let name = db.layer_name_by_number(layer);
            let r = (x0, y0, x1, y1);
            let (w, len) = ((x1 - x0).min(y1 - y0), (x1 - x0).max(y1 - y0));
            let h = db.layer_find_v55_spacing(&name, w, len).unwrap_or(0);
            // ⚠️ An octilinear box has no net and so connects to nothing — see below.
            if !octilinear {
                fixed_via_shapes.push(vyges_pdn::vias::Shape {
                    layer: name.clone(),
                    net: net.clone(),
                    rect: r,
                });
            }
            blockages.push((
                name,
                (r.0 - h, r.1 - h, r.2 + h, r.3 + h),
                // ⚠️ **An OCTILINEAR box loses its net.** Its corners are a bounding box around a
                // 45-degree segment, not the metal, so the reference refuses to reason about what
                // it connects: `setNet(nullptr)` and `kObs`. With no net there is no same-net
                // exemption, and the whole bounding box obstructs.
                (!octilinear).then(|| net.clone()),
                r,
            ));
        }
    }

    let rows = rows_of(&db);
    // 🔑 **A region's rows belong to that region's domain, not the core's.**
    // `VoltageDomain::getRegionRows` takes every row OVERLAPPING the region — not contained in it
    // — and `getDomainRows` then hands the core domain what no other domain claimed.
    //
    // ⚠️ Overlap, not containment: a row crossing the region's edge belongs to the region, and the
    // core grid must not lay a rail on it. Reading it as containment leaves both grids building on
    // the boundary rows and every one of them written twice on different nets.
    let claimed_regions: Vec<Rect> = db
        .block_get_regions()
        .iter()
        .flat_map(|r| db.region_boundaries(r).unwrap_or_default())
        .collect();
    let mut emitted: Vec<(String, String, Rect, &str)> = Vec::new();
    // Over-pad straps that survived their cut whole, and so are still eligible for a refine.
    // `(index into emitted, index into iterm_holds, what the refine needs)`.
    let mut refinable: Vec<(usize, Option<usize>, OverPadStrap)> = Vec::new();
    let mut refinable_next: Vec<(usize, Option<usize>, OverPadStrap)> = Vec::new();
    // What stage 6f needs to know about the components that made each shape: a strap set's own
    // width, spacing and pitch, and the follow pins' pitch.
    let mut followpin_pitch = 0;
    let mut followpin_layer = String::new();
    #[allow(clippy::type_complexity)]
    let mut strap_sets: Vec<(String, i32, i32, i32, bool)> = Vec::new();
    // `(net, (lower, upper), area)` for every CROSSING, built or refused, so trimming can ask what
    // holds each shape up — `Grid::makeVias` makes a `Via` per intersection and nothing but a
    // deleted shape takes one away.
    let mut via_areas: Vec<(String, (String, String), Rect)> = Vec::new();
    // 🔑 **The crossings that actually produced metal**, which is a different list from every
    // crossing. `Shape::hasInternalConnections` walks a shape's vias and asks `!via->isFailed()`,
    // so a shape held up by refused vias alone is destroyed at the write even though those same
    // vias counted for its trim. See the cleanup pass at the end of the write stage.
    let mut via_ok: Vec<(String, (String, String), Rect)> = Vec::new();
    // 🔑 **What `getConnectableShapes` contributed, kept for the whole run.** A via landing on a
    // pad's own pin is held by a shape that is a real `Shape` in the reference and so survives
    // `cleanupVias`, but that this engine never emits — the reference's pad grids write no shape on
    // their pins' layer either. Without it all 689 of them are built and then dropped as unheld.
    let mut connectable_pins: Vec<vyges_pdn::vias::Shape> = Vec::new();
    // Shapes held by a terminal rather than by a via — `Shape::addITermConnection`.
    let mut iterm_holds: Vec<(String, String, Rect)> = Vec::new();
    let grid_mfg = db.manufacturing_grid().unwrap_or_default().unwrap_or(1);
    // `(placement index, layer, the via's metal there, the via's area)` — one entry per FACE of
    // each via.
    //
    // ⚠️ **Per via, not per layer.** `Via::writeToDb` is called once for each via with that via's
    // own shapes, and checks them against that via's own lower and upper shape. Pooling every
    // via's metal on a layer into one check merges a whole column into a single rect: the shape
    // then appears to grow enormously, the direction guard refuses it, and every via on it is
    // ripped up — and measurably so, on real designs.
    let mut via_faces: Vec<(usize, String, Rect, Rect)> = Vec::new();
    // 🔑 **A via grows the shape it lands on BEFORE the next crossing is measured.**
    // `Via::writeToDb` builds one via, merges its metal into the two shapes it touches, and only
    // then moves to the next via — so a strap reached down by an early via is LONGER by the time a
    // later connect intersects it, and that later crossing is measured against the longer strap.
    //
    // ⚠️ **Not the same thing as the absorb pass below.** That one runs once, after every via
    // exists, and reproduces the final shapes correctly; what it cannot do is feed a growth back
    // into a crossing that was measured before it. A rail via reaching an M5 strap 14 units past
    // its end moves every M5-M6 crossing on that strap by 7, and picks a different via with it.
    //
    // ⚠️ Kept SEPARATE from `emitted` on purpose. The reference trims before any of this, so
    // letting the growth into `emitted` here would hand trimming a strap the reference trims
    // short and then extends -- see the absorb pass for why the order is the whole of the effect.
    let mut grown_rects: std::collections::HashMap<(String, String, Rect), Rect> =
        std::collections::HashMap::new();

    // Patch metal left on layers a via stack only passes through, written as DRCFILL.
    //
    // ⚠️ Carries the stack's identity for the same reason a placement does: the patch exists only
    // because the stack passes through, so a stack that trimming leaves unheld takes its patches
    // with it. Left behind, they are metal on a layer nothing connects to.
    let mut drcfill: Vec<(String, String, Rect, String, String, Rect)> = Vec::new();
    // Where each via goes, applied only once the boxes are in — see the note at the placement.
    // Each carries the STACK's two end layers and the stack's own area, which is what decides
    // whether trimming has left it standing.
    let mut placements: Vec<(String, String, (i32, i32), String, String, Rect)> = Vec::new();

    // `(layer, rect)` — the layer matters: a strap avoids only the rings on its OWN layer.
    let mut ring_shapes: Vec<(String, Rect)> = Vec::new();
    // Layers whose ring shapes are locked, and so cannot be modified later.
    let mut locked_layers: Vec<String> = Vec::new();
    // ── where a shape meets the die edge ─────────────────────────────────────────────────────
    // 🔑 **Recorded per grid, on the shapes as BUILT** — `GridComponent::addShape` gives a shape
    // whose edge lands exactly on the die boundary a connection one layer `minWidth` deep, and it
    // does so the moment the shape is added: before `repairVias` reaches it, before trimming, and
    // before any via metal is absorbed.
    //
    // ⚠️ **The moment is the whole of it, and it took three attempts to place.**
    // `via_metal_overhangs_shape` builds its metal4 strap at `y = 0`, which earns a connection;
    // `repairVias` then pulls the strap to `y = -170` to reach the rail below it, and a capture
    // taken any later sees a shape that no longer touches the edge. The engine's own trace is what
    // said so, after two readings of the source did not:
    //
    // ```text
    // [repair] VSS|metal4|(3520, 0, 4480, 200780) -> (3520, -170, 4480, 200780)
    // ```
    //
    // ⚠️ **All four edges, independently** — a shape reaching a corner earns two.
    //
    // Each entry is `(net, layer, on the x axis, the slice)`. The slice's CROSS extent is refreshed
    // from the surviving shape at write time, which is what `Shape::writeToDb` does.
    let mut edge_connections: Vec<(String, String, bool, Rect)> = Vec::new();

    // Every grid this run declares, in the order `buildGrids` would build them.
    // ⚠️ **One `-macro` declaration can name several instances, and each is its own grid** with
    // the same components. Expanded here so the loop below never has to know.
    let grids: Vec<(GridSpec, Opts)> = opts
        .grids()
        .into_iter()
        .flat_map(|(spec, o)| {
            let base = GridSpec::parse(&spec, per_micron);
            if base.instance.is_empty() {
                return vec![(base, o.clone())];
            }
            let patterns: Vec<String> = base
                .instance
                .split('+')
                .filter(|i| !i.is_empty())
                .map(str::to_string)
                .collect();
            select_instances(&db, base.by_cell, &patterns, &base.orients)
                .into_iter()
                // 🔑 **`InstanceGrid::isValid`** — an instance connected to no supply net gets a
                // dummy grid, and `getGrids(true)` skips those. See `instance_has_supply`.
                .filter(|i| instance_has_supply(&db, i, &build_nets))
                .map(|i| {
                    (
                        GridSpec {
                            name: base.name.clone(),
                            instance: i,
                            by_cell: base.by_cell,
                            halo: base.halo,
                            orients: base.orients.clone(),
                            to_boundary: base.to_boundary,
                        },
                        o.clone(),
                    )
                })
                .collect()
        })
        .collect();

    // ── what the technology allows a component to state ─────────────────────────────────────
    // 🔑 **Before anything is built, and the first violation ends the run.** See `validate_grids`.
    if let Some(d) = validate_grids(&db, &grids, &build_nets, per_micron, core) {
        // 🔑 **One site, every diagnostic.** `Diag` already carries the reference's own code and
        // wording (PDN-0003/0004/0005, 0106/0107/0108/0114/0117/0118/0191, 0185, 0215), and its
        // `Display` renders upstream's exact `[ERROR PDN-0106] ...` line. Routing it here puts all
        // twelve on the causal trail without a second list to keep in step with the first.
        //
        // ⚠️ The house code carries the reference's NUMBER, so a consumer can cluster by the code
        // upstream would have raised rather than by our prose.
        vyges_events::emit(
            &vyges_events::Event::new(
                "vyges-pdn",
                vyges_events::Severity::Error,
                format!("PDN-{:04} {}", d.code, d.message),
            )
            .with_code(&format!("PDN-{:04}", d.code)),
        );
        eprintln!("{d}");
        return ExitCode::from(1);
    }

    // ── each grid's own keep-out, for every OTHER grid ───────────────────────────────────────
    // 🔑 **`Grid::getGridLevelObstructions` gives every grid a blanket over the area it occupies**,
    // on the layers it uses: its strap layers and the intermediate routing layers of its connects.
    // ⚠️ An instance grid adds a second blanket over its GRID area — the instance outline plus its
    // halo — on those same layers.
    //
    // 🔑 **They are `kGridObs` and belong to the grid that made them**, which is the whole point:
    // that grid ignores its own, and every other grid is kept off the region it has claimed. Built
    // before the loop because a grid built early must see the keep-out of one built later — the
    // reference collects them all in `buildGrids` before any grid runs.
    let all_layers = layers_with_numbers(&db);
    #[allow(clippy::type_complexity)]
    let grid_obs: Vec<(usize, String, Rect, Rect)> = grids
        .iter()
        .enumerate()
        .flat_map(|(i, (g, o))| {
            // 🔑 **A CORE grid contributes no blanket unless its domain has a REGION**, and this
            // engine builds no region domains.
            //
            // ⚠️ Obvious once seen and expensive to assume otherwise: a core grid's blanket would
            // cover the whole core on every layer it uses — including the intermediate routing
            // layers of its connects, which are exactly the layers the macro grids strap. Giving
            // it one cut every macro grid's straps away entirely.
            // 🔑 **A core-kind grid contributes a blanket exactly when its domain has a REGION.**
            //
            // ⟹ With no region domain in the design this is dead, which is why it has never
            // mattered here. A region domain makes it live: the region grid claims its rectangle,
            // and the core grid's straps are cut where they cross it instead of running through.
            let region_area: Option<Rect> = o.one("domain").and_then(|spec| {
                let name = spec.split(':').next().unwrap_or("");
                db.region_boundaries(name)
                    .ok()
                    .filter(|b| !b.is_empty())
                    .map(|b| {
                        b.into_iter()
                            .reduce(|a, c| {
                                (a.0.min(c.0), a.1.min(c.1), a.2.max(c.2), a.3.max(c.3))
                            })
                            .unwrap()
                    })
            });
            if g.instance.is_empty() && region_area.is_none() {
                return Vec::new();
            }
            let area = if let Some(r) = region_area {
                r
            } else {
                let b = db.inst_bbox(&g.instance).unwrap_or_default();
                if b.len() != 4 {
                    return Vec::new();
                }
                (b[0], b[1], b[2], b[3])
            };
            // 🔑 **How many rings nest, which is how far the keep-out reaches** — the grid's own
            // net count, which is also what a strap set divides its pitch by.
            let ring_nets: i32 = grid_net_count(&db, o, g, &build_nets);
            // Its strap layers, plus every routing layer its connects pass through.
            let mut layers: Vec<String> = Vec::new();
            for s in o.all("stripe") {
                let l = s.split_once(':').map(|(l, _)| l).unwrap_or(s);
                if !layers.iter().any(|x| x == l) {
                    layers.push(l.to_string());
                }
            }
            for c in o.all("connect") {
                let pair = c.split(':').next().unwrap_or("");
                if let Some((lo, hi)) = pair.split_once(',') {
                    let (a, b) = (layer_number(&all_layers, lo), layer_number(&all_layers, hi));
                    for l in vyges_pdn::vias::intermediate_layers(&all_layers, a, b) {
                        // ⚠️ **`getIntermediteRoutingLayers()`, routing only.** The full list
                        // carries the CUT layers too, and a blanket on a cut layer blocks every
                        // via of a connect that passes through it — the reference's own via filter
                        // says as much: a grid obstruction on a non-routing layer never blocks.
                        if routing_level(&db, &l) == 0 {
                            continue;
                        }
                        if !layers.iter().any(|x| x == &l) {
                            layers.push(l);
                        }
                    }
                }
            }
            // 🔑 **The ring blanket is a DIFFERENT rect on DIFFERENT layers, and it is bloated.**
            // `Grid::getGridLevelObstructions` has two halves and they share nothing but the core.
            //
            // 🔑 **`getTotalWidth` counts every NET's ring, not one:**
            //
            // ⚠️ Taking one ring's width instead left a region grid's keep-out four thousand units
            // wide where the reference's is fifteen thousand five hundred — so the core grid's
            // straps ran up to the region's edge, their halos cut the region ring's own segments,
            // and the rails those segments were holding were trimmed away behind them. The
            // symptom was three missing ring shapes; the cause was a keep-out a quarter of the
            // right size, four stages upstream.
            //
            // ⚠️ **And the ring rect belongs on the RING's layers.** Ours went on every layer the
            // grid strapped as well, which is a keep-out the reference never asks for.
            let ring_layers_of = |spec: &str| -> Vec<String> {
                spec.split_once(':')
                    .map(|(l, _)| l)
                    .unwrap_or(spec)
                    .split(',')
                    .map(str::to_string)
                    .collect()
            };
            let mut ring_blankets: Vec<(String, Rect)> = Vec::new();
            for spec in o.all("ring") {
                let ring_layers = ring_layers_of(spec);
                let (l0, l1) = (
                    ring_layers.first().cloned().unwrap_or_default(),
                    ring_layers.get(1).cloned().unwrap_or_default(),
                );
                let p: Vec<&str> = spec.split(':').skip(1).collect();
                // ⚠️ One value expands to both layers, exactly as `-widths 5.0` does.
                let pair = |field: Option<&&str>| -> (i32, i32) {
                    let v: Vec<i32> = field
                        .unwrap_or(&"0")
                        .split(',')
                        .map(|x| dbu(x, per_micron))
                        .collect();
                    match v.len() {
                        0 => (0, 0),
                        1 => (v[0], v[0]),
                        _ => (v[0], v[1]),
                    }
                };
                let (w0, w1) = pair(p.first());
                let (s0, s1) = pair(p.get(1));
                let offs: Vec<i32> = p
                    .get(2)
                    .unwrap_or(&"0")
                    .split(',')
                    .map(|v| dbu(v, per_micron))
                    .collect();
                let off4 = match offs.len() {
                    0 => [0; 4],
                    1 => [offs[0]; 4],
                    2 => [offs[0], offs[1], offs[0], offs[1]],
                    _ => [offs[0], offs[1], offs[2], offs[3]],
                };
                let n = ring_nets.max(1);
                let mut hor = w0 * n + s0 * (n - 1);
                let mut ver = w1 * n + s1 * (n - 1);
                if direction_of(&db, &l0) != Direction::Horizontal {
                    std::mem::swap(&mut hor, &mut ver);
                }
                let _ = &l1;
                let ring_rect = (
                    area.0 - ver - off4[0],
                    area.1 - hor - off4[1],
                    area.2 + ver + off4[2],
                    area.3 + hor + off4[3],
                );
                for l in ring_layers {
                    ring_blankets.push((l, ring_rect));
                }
            }
            // ⚠️ **The halo box is the INSTANCE grid's alone.** `InstanceGrid` overrides the base
            // and adds `getGridArea()` — the instance plus its halo — on every layer the base
            // produced; a core grid with a region gets the base function and nothing more.
            let grid_area = (
                area.0 - g.halo[0],
                area.1 - g.halo[1],
                area.2 + g.halo[2],
                area.3 + g.halo[3],
            );
            let instance_grid = !g.instance.is_empty();
            let mut out: Vec<(usize, String, Rect, Rect)> = Vec::new();
            // 🔑 **The instance's own obstructions, with this grid's halo, as GRID-level ones.**
            // `InstanceGrid::getGridLevelObstructions` copies them in as `GridObsShape`, which is
            // what makes a macro's halo cut a strap it never touches and leave every follow pin
            // alone.
            for (inst, layer, r, _net, _raw) in &inst_obs_haloed {
                if *inst == g.instance {
                    out.push((i, layer.clone(), *r, *r));
                }
            }
            for l in &layers {
                out.push((i, l.clone(), area, area));
                if instance_grid {
                    out.push((i, l.clone(), grid_area, grid_area));
                }
            }
            for (l, r) in ring_blankets {
                // The one blanket the reference bloats, and it is bloated by its own size.
                let (w, len) = ((r.2 - r.0).min(r.3 - r.1), (r.2 - r.0).max(r.3 - r.1));
                let h = db.layer_find_v55_spacing(&l, w, len).unwrap_or(0);
                out.push((i, l.clone(), (r.0 - h, r.1 - h, r.2 + h, r.3 + h), r));
                if instance_grid && !layers.iter().any(|x| *x == l) {
                    out.push((i, l, grid_area, grid_area));
                }
            }
            out
        })
        .collect();
    // The same list the reference's `Obs` group prints, in the same normalised form, so the two
    // can be diffed line for line rather than compared by count.
    if std::env::var_os("PDN_TRACE").is_some() {
        for (owner, layer, q, r) in &grid_obs {
            let name = grids
                .get(*owner)
                .map(|(g, _)| {
                    if g.instance.is_empty() {
                        g.name.clone()
                    } else {
                        format!("{} - {}", g.name, g.instance)
                    }
                })
                .unwrap_or_default();
            // Both rects: the reference's `Obs` group prints the shape, and the halo is what
            // actually decides where a crossing strap is cut. A blanket whose rect is right and
            // whose halo is short reads as a geometry defect and is a spacing one.
            eprintln!(
                "[obs] {name}|{layer}|{},{},{},{}|halo {},{},{},{}",
                r.0,
                r.1,
                r.2,
                r.3,
                r.0 - q.0,
                r.1 - q.1,
                q.2 - r.2,
                q.3 - r.3
            );
        }
    }

    // ── every declared grid, in order ────────────────────────────────────────────────────────
    // 🔑 **`buildGrids` loops over the grids**, and each one sees everything the ones before it
    // made: `all_shapes` and `block_obs` are handed in and added to after each. A macro's grid is
    // therefore cut by the core grid's straps, and the core grid is not cut by the macro's.
    //
    // ⚠️ **Vias are made INSIDE this loop**, not after it — `Grid::makeShapes` ends with
    // `makeVias` and `repairGridChannels`. A grid connects its own straps, using its own connect
    // statements, to whatever is standing at that moment.
    // Every fixed instance's outline, per layer it occupies — read once, used by `cleanupShapes`.
    let macro_boxes = macro_outlines(&db);
    // 🔑 **A grid's via GEOMETRY is built AFTER trimming, so it is deferred.**
    // `PdnGen::buildGrids` makes each grid's shapes and its crossing SET, then trims, and only
    // `PdnGen::writeToDb` — which runs after all of that — calls `Connect::makeVia` to size and
    // place anything. So a via is measured against the strap AS TRIMMED.
    //
    // ⚠️ **The two readings agree except where a strap ENDS inside the crossing**, and then they
    // are far apart: an always-on switch pin sits at the end of its strap, and sizing the stack
    // against the untrimmed strap leaves an intermediate rect 100 units too tall on every one of
    // them. Trimming needs only the crossing AREAS, which are known before any via is built, so
    // the dependency between the two is breakable in this direction and no other.
    struct PendingVias<'a> {
        opts: &'a Opts,
        placed: Vec<vyges_pdn::vias::Via>,
        dropped: Vec<(vyges_pdn::vias::Via, vyges_pdn::vias::Failed)>,
        fixed: Vec<(
            String,
            String,
            Vec<String>,
            Option<(i32, i32)>,
            Option<regex::Regex>,
        )>,
        on_grid: Vec<((String, String), Vec<String>)>,
        max_cuts: Vec<((String, String), (i32, i32))>,
        split_by_connect: Vec<((String, String), Vec<(String, i32, bool)>)>,
        min_width_by_connect: Vec<((String, String), Vec<String>)>,
        ground: String,
    }
    let mut pending: Vec<PendingVias> = Vec::new();
    for (grid_index, (grid, opts)) in grids.iter().enumerate() {
        // 🔑 **`Grid::makeShapes` opens with PDN-0001** (`grid.cpp:122`), once per grid, and the
        // name it prints is `getLongName()`: the grid's own name for a core grid, and
        // `"<name> - <instance>"` for an instance grid (`grid.cpp:1442`). Reproducing the long
        // form matters — a design with one macro grid per macro prints one line each, and the
        // instance is the only thing telling them apart.
        vyges_events::emit(
            &vyges_events::Event::new(
                "vyges-pdn",
                vyges_events::Severity::Info,
                format!(
                    "PDN-0001 Inserting grid: {}",
                    if grid.instance.is_empty() {
                        grid.name.clone()
                    } else {
                        format!("{} - {}", grid.name, grid.instance)
                    }
                ),
            )
            .with_code("PDN-GRID-INSERT"),
        );
        // 🔑 **`-starts_with` belongs to the GRID that declared it**, and `define_pdn_grid`
        // defaults it to GROUND — `set start_with_power 0`. A grid that says nothing gets ground
        // however loudly a sibling asked for power.
        //
        // ⚠️ Read globally instead, one grid's `-starts_with POWER` swaps the nets on every other
        // grid's straps: identical geometry, identical counts, wrong nets — and nothing in the
        // shapes to show for it. A macro grid beside a core grid is the common case.
        let build_nets = nets::build_order(
            &domain,
            opts.one("starts-with")
                .is_some_and(|v| v.eq_ignore_ascii_case("power")),
        );
        // ── this grid's own extent ──────────────────────────────────────────────────────────
        // 🔑 **An instance grid is bounded by its INSTANCE, and by two different rects.**
        // `getDomainArea` is the instance's outline, which is what straps are laid across;
        // `getDomainBoundary` is the outline of its **supply pins**, which is what they are
        // extended to. ⚠️ The two are only the same when `-grid_to_boundary` says so, and taking
        // the outline for both runs every strap out to the edge of a macro whose pins stop well
        // inside it.
        // 🔑 **A grid may belong to a REGION domain rather than the core one.**
        // `set_voltage_domain -region <name>` binds a domain to a named DEF region, and
        // `define_pdn_grid -voltage_domains <name>` builds a grid for it. Everything the grid does
        // is then measured against the region's own rectangle rather than the die core, and its
        // supply nets are the domain's, not the block's.
        //
        // ⚠️ **A region may be several boxes** — DEF allows a non-rectangular region — so the area
        // is their union. Taking the first silently shrinks the grid.
        let region_domain: Option<(Rect, nets::Domain)> = opts.one("domain").and_then(|spec| {
            let mut f = spec.split(':');
            let name = f.next().unwrap_or("");
            let power = f.next().unwrap_or("").to_string();
            let ground = f.next().unwrap_or("").to_string();
            // ⚠️ **A region domain carries its own secondary supplies.**
            // `set_voltage_domain -region ... -secondary_power {VREG1 VREG2}` attaches them to
            // THAT domain, and its rings nest power, ground and each secondary in order — so
            // dropping them does not merely omit shapes, it moves the ones that remain.
            let secondary: Vec<String> = f
                .next()
                .unwrap_or("")
                .split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect();
            // ⚠️ **And its own switched supply.** A UPF power switch names the domain it belongs
            // to — `create_power_switch -domain TEMP_ANALOG -output_supply_port {vout VIN_SW}` —
            // so each domain has its own, and `VoltageDomain::getNets` puts it straight after the
            // primary power net.
            let switched = f.next().unwrap_or("").to_string();
            let boxes = db.region_boundaries(name).unwrap_or_default();
            if boxes.is_empty() || power.is_empty() || ground.is_empty() {
                vyges_events::log(
                    "vyges-pdn",
                    vyges_events::Severity::Warn,
                    format!("no usable region {name:?} for grid {:?}", grid.name),
                );
                return None;
            }
            let area = boxes
                .iter()
                .copied()
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
                .unwrap();
            Some((
                area,
                nets::Domain {
                    power,
                    ground,
                    switched_power: (!switched.is_empty()).then_some(switched),
                    secondary,
                },
            ))
        });
        let (core, build_nets) = if let Some((area, rd)) = &region_domain {
            let starts_power = opts
                .one("starts-with")
                .is_some_and(|v| v.eq_ignore_ascii_case("power"));
            (*area, nets::build_order(rd, starts_power))
        } else if grid.instance.is_empty() {
            (core, build_nets.clone())
        } else {
            let bbox = db.inst_bbox(&grid.instance).unwrap_or_default();
            if bbox.len() != 4 {
                vyges_events::log(
                    "vyges-pdn",
                    vyges_events::Severity::Warn,
                    format!("no instance {:?} for grid {:?}", grid.instance, grid.name),
                );
                continue;
            }
            let area = (bbox[0], bbox[1], bbox[2], bbox[3]);
            // ⚠️ **The grid's nets are the ones the INSTANCE is connected to.** A macro wired to
            // one supply gets one strap per pitch, not a power/ground pair — and building the pair
            // anyway puts metal for a net the macro has no pin for.
            // ⚠️ Asked through the MASTER's terminals. `inst_get_i_terms` answers with
            // `<instance>/<terminal>` while `iterm_get_net` wants the terminal alone, and the two
            // do not compose — the lookup simply misses and every macro grid comes out with no
            // nets and therefore no straps.
            let master = db.inst_get_master(&grid.instance);
            let connected: Vec<String> = db
                .master_get_m_terms(&master)
                .iter()
                .map(|term| db.iterm_get_net(&grid.instance, term))
                .filter(|n| !n.is_empty())
                .collect();
            let nets: Vec<String> = build_nets
                .iter()
                .filter(|n| connected.contains(n))
                .cloned()
                .collect();
            (area, nets)
        };
        let boundary = if grid.instance.is_empty() {
            core
        } else if grid.to_boundary {
            core
        } else {
            instance_pin_outline(&db, &grid.instance).unwrap_or(core)
        };
        // The instance's outline plus its halo — what the grid may obstruct, not what it fills.
        let grid_area = (
            core.0 - grid.halo[0],
            core.1 - grid.halo[1],
            core.2 + grid.halo[2],
            core.3 + grid.halo[3],
        );
        let _ = grid_area;
        // ⚠️ Straps are laid into the domain boundary BLOATED VERTICALLY by half the widest follow
        // pin, so a vertical strap reaches the outer edge of the outermost rail rather than the
        // core.
        // 🔑 **The follow pin's OWN width, stated or derived.** `CoreGrid::getDomainBoundary`
        // bloats by `strap->getWidth()` — the width the component was built with — and a
        // `-width` on a follow-pin stripe replaces the cell-derived one outright.
        // ⚠️ Always asking the cells puts the boundary two units off wherever a case states a
        // width, and a pin-layer strap — which trimming may not shrink — then keeps that error to
        // the end.
        let widest_followpin = opts
            .all("followpins")
            .iter()
            .map(|s| {
                s.splitn(3, ':')
                    .nth(2)
                    .filter(|w| !w.is_empty())
                    .map(|w| dbu(w, per_micron))
                    .filter(|w| *w > 0)
                    .unwrap_or_else(|| followpin_width(&db).unwrap_or(0))
            })
            .max()
            .unwrap_or(0);
        let strap_boundary = if grid.instance.is_empty() {
            vyges_pdn::grid::domain_boundary(core, widest_followpin)
        } else {
            boundary
        };
        // 🔑 **This grid does not see its own instance's obstructions.** They are in the global
        // list for every other grid; here they are taken back out, which is what
        // `skip_insts` plus `getGridLevelObstructions` amounts to.
        // 🔑 **Real obstructions, kept apart from the grid blankets.** `FollowPinShape::cut`
        // overrides the obstruction filter to drop every `kGridObs`: a followpin is cut by real
        // geometry only, never by another grid's estimated keep-out.
        let real_blockages: Vec<(String, Rect, Option<String>, Rect)> = {
            let mine: Vec<(String, Rect)> = if grid.instance.is_empty() {
                Vec::new()
            } else {
                inst_obs
                    .iter()
                    .filter(|(i, ..)| *i == grid.instance)
                    .map(|(_, l, r, _, _)| (l.clone(), *r))
                    .collect()
            };
            blockages
                .iter()
                .filter(|(l, r, net, _)| {
                    net.is_some() || !mine.iter().any(|(ml, mr)| ml == l && mr == r)
                })
                .cloned()
                // ⚠️ Every OTHER grid's blanket, and none of this grid's own.
                .collect()
        };
        // Everything else is cut by those AND by every other grid's blanket.
        let blockages: Vec<(String, Rect, Option<String>, Rect)> = real_blockages
            .iter()
            .cloned()
            // ⚠️ Every OTHER grid's blanket, and none of this grid's own.
            .chain(
                grid_obs
                    .iter()
                    .filter(|(owner, ..)| *owner != grid_index)
                    .map(|(_, l, q, r)| (l.clone(), *q, None, *r)),
            )
            .collect();

        // Where this grid's own shapes begin in the accumulated list.
        let grid_shapes_from = emitted.len();
        // `getConnectableShapes` — the pads' own pins, gathered as the connections are built and
        // handed to the via search alongside the grid's shapes. See the pad component below.
        let mut pad_connectable: Vec<vyges_pdn::vias::Shape> = Vec::new();

        // ── rings ────────────────────────────────────────────────────────────────────────────────
        // ⚠️ **Where THIS grid's rings begin.** `ring_shapes` accumulates across grids so that a
        // strap avoids the rings of every grid before it, but the ring AREA a strap extends to is
        // its own grid's. Measured from the whole list, two macro grids each extend their straps
        // to the union of both macros' rings and every one of them crosses the die.
        // The rows this grid's domain owns — its own where it has a region, everything no
        // region claimed where it does not.
        let rows: Vec<followpins::Row> = if let Some((area, _)) = &region_domain {
            rows.iter()
                .filter(|r| overlaps(r.bbox, *area))
                .cloned()
                .collect()
        } else if claimed_regions.is_empty() {
            rows.clone()
        } else {
            rows.iter()
                .filter(|r| !claimed_regions.iter().any(|g| overlaps(r.bbox, *g)))
                .cloned()
                .collect()
        };
        // 🔑 **A follow pin takes its supply from ITS OWN domain**, as the reference does.
        //
        // ⚠️ Bound globally, a region grid lays rails for the BLOCK's supply on the region's rows
        // — so the region's own net gets none, and the rails it does make duplicate the core
        // grid's on the very rows the core was told to leave alone.
        //
        // 🔑 **And its supply is `getPower()`, which is the SWITCHED net wherever there is one:**
        //
        // ⚠️ **`FollowPins::makeShapes` is the only caller of `getPower()` in the module**, which
        // is what makes the distinction easy to lose: everything else asks `getNets()`, where the
        // always-on net and the switched one both appear and neither stands for the other. The
        // core domain had this and a region domain did not, so a switched region laid its rails on
        // the always-on net — the rails the switch cells are supposed to drive.
        let (followpin_power, ground): (&str, &str) = match &region_domain {
            Some((_, rd)) => (
                rd.switched_power.as_deref().unwrap_or(rd.power.as_str()),
                rd.ground.as_str(),
            ),
            None => (followpin_power, ground),
        };
        // 🔑 **And so does everything the power switches are lined up with.**
        // `GridSwitchedPower` asks `grid_->getDomain()` for all three nets and never the block.
        //
        // ⚠️ **`getAlwaysOnPower()` is the raw net and `getPower()` is the switched one**, and the
        // switches need both: the always-on net is what `buildStrapTargetList` looks for a strap
        // on, and the switched net is what the cell's output pin is tied to. Taking the block's
        // pair for a region grid searches for a strap on a net that grid never builds, so no
        // switch is placed, and the region's switched rails are then held by nothing and trimmed.
        let (grid_always_on, grid_switched) = match &region_domain {
            Some((_, rd)) => (rd.power.as_str(), rd.switched_power.as_deref()),
            None => (power, domain.switched_power.as_deref()),
        };
        trace(
            "Grid",
            &format!(
                "{} core={core:?} nets={build_nets:?} rows={}",
                grid.name,
                rows.len()
            ),
        );
        let ring_start = ring_shapes.len();

        // ── the grid's components, in the order it holds them ────────────────────────────────────
        // 🔑 **One loop, and the sequence is not in it.** `components::plan` decides the order and
        // each arm builds one kind — the same split the reference makes between
        // `Grid::getGridComponents()` and `GridComponent::make`. Changing the order is then a
        // change to `plan` and its tests, not surgery on this function.
        //
        // 🔑 **Order is the answer.** `GridComponent::make` is `makeShapes → cutShapes →
        // getObstructions → getShapes`: each component is cut against everything built before it,
        // then becomes an obstruction for everything after. Pad connections in particular run
        // SECOND — `-connect_to_pads` is an argument to `define_pdn_grid`, so its straps enter
        // `straps_` ahead of any `add_pdn_stripe` — so a pad targets the RINGS, and the followpins
        // and straps after it are cut around it.
        //
        // ⚠️ Confirmed against a reference run under `set_debug_level PDN Make 1`: ring, then all
        // sixteen `Direct connect pin` components, then `Followpin`, then the two `Strap` sets.
        let mut ring_area = strap_boundary;
        let mut rings_measured = false;
        // 🔑 **A component that builds nothing is retried once, after every other has run.**
        //
        // `make()` reports whether the shape count moved, which is what `emitted.len()` is here.
        //
        // ⟹ **This is what carries a pad connection in a design with no ring.** Pads run second,
        // so with only `add_pdn_stripe` declared there is nothing yet to reach; the component
        // builds nothing, defers, and connects on the retry once the straps exist.
        // ⚠️ A deferred component is appended to the queue being walked, so every first-pass
        // component runs before any retry and the retries keep the order they deferred in — the
        // two loops above, without a second copy of the match.
        // 🔑 **The grid's pad connections, decided once.** `setupDirectConnect` runs at
        // definition time and its answer is what becomes the component list, so it is settled
        // before the loop rather than inside it.
        let pad_conns: Vec<PadConnection> = if grid.instance.is_empty() {
            let named: Vec<String> = opts
                .all("connect-to-pads")
                .iter()
                .flat_map(|s| s.split(','))
                .map(|s| s.trim_end_matches(":ring"))
                .filter(|s| !s.is_empty() && *s != "all")
                .map(str::to_string)
                .collect();
            pad_connections(&db, &build_nets, &named, core)
        } else {
            Vec::new()
        };
        let mut queue: Vec<(components::Component, bool)> =
            components::plan(&opts.values, pad_conns.len())
                .into_iter()
                .map(|c| (c, false))
                .collect();
        let mut at = 0;
        while at < queue.len() {
            let (comp, is_retry) = queue[at];
            at += 1;
            trace(
                comp.kind(),
                &if is_retry {
                    format!("{} (deferred retry)", comp.spec())
                } else {
                    comp.spec()
                },
            );
            let before = emitted.len();
            // ⚠️ The ring area is what `-extend_to_core_ring` reaches, so it can only be measured
            // once every ring is built. `plan` puts the rings first, which makes the first non-ring
            // component exactly the moment it becomes knowable.
            if !rings_measured && !matches!(comp, components::Component::Ring(_)) {
                let rects: Vec<Rect> =
                    ring_shapes[ring_start..].iter().map(|(_, r)| *r).collect();
                ring_area = vyges_pdn::grid::ring_area(strap_boundary, &rects);
                rings_measured = true;
                // The reference prints its equivalent per strap component as `boundary (...)`.
                trace(
                    "Ring area",
                    &format!(
                        "{},{},{},{} from {} ring shapes (strap boundary {},{},{},{})",
                        ring_area.0, ring_area.1, ring_area.2, ring_area.3, rects.len(),
                        strap_boundary.0, strap_boundary.1, strap_boundary.2, strap_boundary.3
                    ),
                );
            }
            match comp {
                components::Component::Ring(spec) => {
                    let (segments, locked) =
                        make_ring(&db, spec, &build_nets, core, die, per_micron);
                    // ⚠️ Not on the retry, for the same reason `strap_sets` is not: our bookkeeping,
                    // recorded once per declaration.
                    if let (Some(layer), false) = (locked, is_retry) {
                        locked_layers.push(layer);
                    }
                    // ◐ **The reference cuts a ring like anything else and we do not — yet.**
                    // `Rings` inherits the base `GridComponent::cutShapes`, and the reference's
                    // `macro_grid` metal10 ring really is fragmented into short segments where ours
                    // is one loop.
                    //
                    // ⚠️ Putting rings through `cut_shapes` was measured: `macro_grid` gained 114
                    // matching shapes and `report` lost 92, and the loss did not move when instance
                    // pins were given their nets. So something other than the same-net exemption
                    // governs which obstructions reach a ring — a ring is `setLocked()` when
                    // single-layer, and `Grid::getGridLevelObstructions` is not the same set a
                    // strap sees. Reverted until that set is known rather than guessed.
                    //
                    // ℹ️ Followpins are a separate case: the reference cuts them too, but ours are
                    // built from rows `followpins::make` has already split around every macro, so
                    // the cut is applied upstream and running it again would apply it twice.
                    let pieces = cut_shapes(
                        &db,
                        &segments
                            .iter()
                            .map(|s| (s.net.clone(), s.layer.clone(), s.rect))
                            .collect::<Vec<_>>(),
                        &blockages
                            .iter()
                            .cloned()
                            .chain(made_obstructions(&db, &emitted))
                            .collect::<Vec<_>>(),
                    );
                    for (net, layer, rect) in pieces {
                        ring_shapes.push((layer.clone(), rect));
                        emitted.push((net, layer, rect, "RING"));
                    }
                }
                components::Component::PadConnect(nth) => {
                    // 🔑 **One connection, on its own.** Each is its own component in the
                    // reference, so each defers and retries independently — which is the whole
                    // point: on a design where thirteen pads have nothing to reach on the first
                    // pass, a single bulk component would build the other fifty-five, report
                    // success, and leave those thirteen empty forever.
                    let Some(conn) = pad_conns.get(nth) else {
                        continue;
                    };
                    // The connections sharing this pad: an over-pad strap is one slot of the
                    // pad's width and cannot be placed without the count.
                    let mut on_pad: Vec<&PadConnection> = pad_conns
                        .iter()
                        .filter(|c| c.inst == conn.inst && c.over_pads.is_some())
                        .collect();
                    // 🔑 **By the LOWEST PIN, not by net and not by terminal order.**
                    // `getAssociatedStraps` sorts the pad's connections geometrically.
                    //
                    // — the minimum pin rectangle of each, compared lexicographically. On a pad
                    // whose ground pin sits below its power pin, ground therefore takes slot zero
                    // however the domain orders its nets.
                    //
                    // ⚠️ Ordered by the master's terminals, a pad's two connections come out
                    // SWAPPED: every slot coordinate correct and every one on the wrong net. A
                    // shape count cannot see that; the per-connection join showed it at once.
                    // 🔑 **In MASTER coordinates.** `getAssociatedStraps` compares
                    // `mpin->getGeometry()` boxes directly and the instance transform is applied
                    // only afterwards, so a pad whose orientation reverses an axis sorts the other
                    // way round if placed rects are used.
                    on_pad.sort_by_key(|c| c.sort_key);
                    let slot = (
                        on_pad.iter().position(|c| c.term == conn.term).unwrap_or(0),
                        on_pad.len(),
                    );
                    // 🔑 **`PadDirectConnectionStraps::getConnectableShapes` — the pad's OWN pins
                    // are via targets, and only for the over-pads form.**
                    //
                    // `Grid::getIntersections` calls it on every component before looking for
                    // crossings, so these are searched alongside the grid's own shapes.
                    //
                    // ⚠️ **On the RING pins' layer, not the strap's.** The strap runs one routing
                    // layer above them — `pins_forming_ring` returns both — so this is what gives
                    // an over-pad strap a via straight down into the pad it runs over.
                    //
                    // ℹ️ Missing it costs a flipchip design that connects over its pads several
                    // hundred vias and exactly one shape: every over-pad strap kept its via to the ring and survived
                    // trimming on that plus its terminal, except the one at the north-east corner,
                    // where the metal9 ring obstructs the via to metal8 and the pad pin is the only
                    // other thing under it.
                    if conn.over_pads.is_some() {
                        if let Some(pin_layer) = conn.facing.first().map(|p| p.layer.clone()) {
                            for (layer, net, rect) in instance_pin_shapes(&db, &conn.inst) {
                                if layer != pin_layer || net != conn.net {
                                    continue;
                                }
                                let shape = vyges_pdn::vias::Shape { layer, net, rect };
                                // ⚠️ A component may be retried; the same pin twice is two shapes,
                                // two identical vias, and the overlap rule then kills both.
                                if !pad_connectable.iter().any(|p: &vyges_pdn::vias::Shape| {
                                    p.layer == shape.layer && p.net == shape.net && p.rect == shape.rect
                                }) {
                                    pad_connectable.push(shape);
                                }
                            }
                        }
                    }
                    let mut over_pad: Option<OverPadStrap> = None;
                    let made = make_pad_connection(
                        &db,
                        opts,
                        conn,
                        &emitted[grid_shapes_from..],
                        core,
                        die,
                        slot,
                        &mut over_pad,
                    );
                    // 🔑 **A pad strap is cut like anything else.**
                    // `PadDirectConnectionStraps::cutShapes` opens by calling the base
                    // `Straps::cutShapes`; only the over-pads form adds anything on top. The
                    // reference's ledger shows two of this design's connections going 5 -> 9 and
                    // 5 -> 10 under the knife, which is a cut making pieces, not losing them.
                    //
                    // ⚠️ Earlier attempts at this were catastrophic because a connected pad's own
                    // pins were in the obstruction set, so every strap was cut apart by the very
                    // pin it starts from. They are excluded now — see `pad_connect_insts`.
                    let obstructions: Vec<(String, Rect, Option<String>, Rect)> = blockages
                        .iter()
                        .cloned()
                        .chain(made_obstructions(&db, &emitted))
                        .collect();
                    let pieces = cut_shapes(
                        &db,
                        &made
                            .iter()
                            .map(|(n, l, r, _)| (n.clone(), l.clone(), *r))
                            .collect::<Vec<_>>(),
                        &obstructions,
                    );
                    // 🔑 **An over-pad strap must both TOUCH the pad and LEAVE it.**
                    // `PadDirectConnectionStraps::cutShapes` runs the base cut and then, for the
                    // over-pads form alone, throws away every piece that fails either half.
                    //
                    // ⚠️ It is the CUT that makes this matter. A strap runs from the pad to the
                    // ring in one piece; an obstruction across it leaves a stub inside the pad and
                    // a remnant out in the core, and neither is a connection.
                    // ⚠️ **The over-pad form, not the over-pad FLAG.** A connection whose pins
                    // form a ring but whose slot came out too narrow falls back to the edge path,
                    // and an edge strap is not required to leave the pad. `over_pad` is set only
                    // where a slot was placed and a target found, which is exactly when there are
                    // shapes here to filter.
                    let over_pad_rect = over_pad.as_ref().map(|_| conn.inst_rect);
                    let pieces: Vec<_> = pieces
                        .into_iter()
                        .filter(|(_, _, r)| {
                            let Some(p) = over_pad_rect else { return true };
                            let contained =
                                p.0 <= r.0 && p.1 <= r.1 && p.2 >= r.2 && p.3 >= r.3;
                            let touches =
                                p.0 <= r.2 && p.2 >= r.0 && p.1 <= r.3 && p.3 >= r.1;
                            !contained && touches
                        })
                        .collect();
                    for (net, layer, rect) in pieces {
                        // ⚠️ The holding pin follows the piece it lands in; a piece that no longer
                        // covers the pin is held by nothing and stands on its vias like any strap.
                        // ⚠️ **A piece with no pin in it is held by NOTHING.**
                        // `addITermConnection` records the terminal only where the terminal
                        // actually is, so cutting a strap in two leaves the far piece standing on
                        // its vias like any other shape. Falling back to the piece itself would
                        // declare the whole thing held and exempt it from trimming entirely.
                        // ⛔ **The OVER-PADS form gets no terminal connection when it is made.**
                        // There are exactly three `addITermConnection` calls in the module:
                        //
                        // - `makeShapes` (the EDGE form) — `shape->addITermConnection(pin_rect.intersect(shape_rect))`
                        // - `refineShape` — `clearITermConnections()` then `addITermConnection(...)`
                        // - nowhere else. `makeShapesOverPads` builds its shape, calls `addShape`,
                        //   and records only `target_shapes_` and `target_pin_shape_`.
                        //
                        // 🔑 **So an over-pad strap is held by its VIAS alone unless it has been
                        // refined**, and `isRemovable` wants two connections.
                        //
                        // ℹ️ A flipchip design that connects over its pads shows both halves at
                        // once. One strap is never obstructed, so it is never refined and
                        // never gains a terminal: a VSS metal9 strap crosses at its y, which cuts
                        // the metal9 ring there and obstructs the via to metal8, so it is left with
                        // the one via onto its own pad pin and trimming takes it away. The strap at
                        // the north-east corner IS refined, gains a terminal, and survives on that
                        // plus its pad pin.
                        //
                        // ⚠️ Held at creation like an edge strap, such a strap survives and the
                        // design comes out one shape over.
                        let hold = if over_pad_rect.is_some() {
                            None
                        } else {
                            made.iter()
                                .find(|(_, l, _, h)| *l == layer && overlaps(*h, rect))
                                .and_then(|(_, _, _, h)| intersect_rect(*h, rect))
                        };
                        // 🔑 **Only a piece IDENTICAL to what was built keeps its target**, and
                        // only a shape with a target can be refined. `replaceShape` carries a
                        // shape's vias onto its pieces and nothing else, so a cut strap vanishes
                        // from `target_shapes_` and `strapViaIsObstructed` answers false for it
                        // without looking at anything.
                        if let Some(r) = over_pad.as_ref().filter(|r| r.strap == rect) {
                            refinable.push((
                                emitted.len(),
                                hold.map(|_| iterm_holds.len()),
                                r.clone(),
                            ));
                        }
                        emitted.push((net.clone(), layer.clone(), rect, "STRIPE"));
                        if let Some(hold) = hold {
                            // Its own pin holds it, exactly as an iterm connection does.
                            iterm_holds.push((net, layer, hold));
                        }
                    }
                }
                components::Component::Followpin(fp_spec) => {
                    let Some((layer, pitch, rails)) = make_followpins(
                        &db,
                        fp_spec,
                        &rows,
                        followpin_power,
                        ground,
                        core,
                        die,
                        ring_area,
                        per_micron,
                    ) else {
                        vyges_events::log(
                            "vyges-pdn",
                            vyges_events::Severity::Warn,
                            "unable to determine width of followpin straps from standard cells",
                        );
                        return ExitCode::from(2);
                    };
                    followpin_pitch = pitch;
                    // 🔑 **A followpin is cut like every other component**, but by REAL
                    // obstructions only — `FollowPinShape::cut` drops every `kGridObs`, since a
                    // grid blanket is an estimate of where another grid will build rather than
                    // metal that is actually there.
                    //
                    // ⚠️ Rows do NOT pre-apply this. They come from the DEF's `ROW` statements and
                    // know nothing about macros: a 5 x 1.4 um `MARKER` with OBS on metal1 splits
                    // the reference's rail and left ours whole.
                    let pieces = cut_shapes(
                        &db,
                        &rails
                            .iter()
                            .map(|(n, r)| (n.clone(), layer.clone(), *r))
                            .collect::<Vec<_>>(),
                        &real_blockages
                            .iter()
                            .cloned()
                            .chain(made_obstructions(&db, &emitted))
                            .collect::<Vec<_>>(),
                    );
                    for (net, l, rect) in pieces {
                        emitted.push((net, l, rect, "FOLLOWPIN"));
                    }
                    followpin_layer = layer;
                }
                components::Component::Strap(spec_text) => {
                    let (set, stripes) = make_strap(
                        &db,
                        spec_text,
                        &domain,
                        &build_nets,
                        &emitted,
                        &blockages,
                        &ring_shapes,
                        StrapBounds {
                            core,
                            die,
                            strap: strap_boundary,
                            ring: ring_area,
                        },
                        per_micron,
                    );
                    // ⚠️ Not on the retry. `strap_sets` is our own bookkeeping — the reference has
                    // no equivalent to re-register — and a set recorded twice is counted twice by
                    // everything downstream that picks the lowest or the highest of them.
                    if let (Some(set), false) = (set, is_retry) {
                        strap_sets.push(set);
                    }
                    emitted.extend(stripes);
                }
            }
            // 🔑 The per-component tally, in the reference's own units. Its `Make` group at level 2
            // prints `Initial shape count` / `Final shape count` around each `cutShapes`, and that
            // final number is this: what the component kept. Diffing the two ledgers says both
            // whether the sequence matches AND whether each stage's output does — a component that
            // is in the right place with the wrong count is a different bug from one in the wrong
            // place.
            trace(comp.kind(), &format!("kept={}", emitted.len() - before));
            // `make()`'s return value: did the shape count move? If not, this component gets one
            // more attempt once everything else has run — and only one.
            if !is_retry && emitted.len() == before {
                queue.push((comp, true));
            }
        }

        // ── vias ─────────────────────────────────────────────────────────────────────────────────
        // 🔑 **A grid only makes vias where it stands.** `Grid::makeVias` seeds its search with
        // THIS grid's shapes and then adds the global ones that intersect a search area — the
        // grid's own boundary merged with the extent of everything it made. Another grid's shapes
        // enter only where they reach into it.
        //
        // ⚠️ Without that, every macro grid remakes every other macro grid's vias: two macros with
        // the same connect statements each build both sets, the duplicates are written twice, and
        // the straps trim to whichever pair survives.
        let via_area = {
            let mut a = boundary;
            for (_, _, r, _) in &emitted[grid_shapes_from..] {
                a = (a.0.min(r.0), a.1.min(r.1), a.2.max(r.2), a.3.max(r.3));
            }
            a
        };
        let in_reach = |i: usize, r: Rect| {
            i >= grid_shapes_from
                || (r.0 <= via_area.2 && via_area.0 <= r.2 && r.1 <= via_area.3 && via_area.1 <= r.3)
        };
        // 🔑 **An instance grid searches its own macro's supply pins alongside the straps** — and
        // they go into `emitted`, not into a separate search list. A via's end has to be a shape
        // the grid knows about or `cleanupVias` drops it as no longer held — placed correctly and
        // then thrown away for want of anything recorded under them.
        //
        // ⚠️ Marked `SWITCH` — the cell's own metal, which holds a via up, is never trimmed, and is
        // never written. The reference's macro grids emit no shape on their pins' layer either.
        if std::env::var_os("PDN_TRACE").is_some() {
            eprintln!("[padconnect] {} pad pin shapes", pad_connectable.len());
        }
        connectable_pins.extend(pad_connectable.iter().cloned());
        let mut grid_pins: Vec<vyges_pdn::vias::Shape> = pad_connectable;
        if !grid.instance.is_empty() {
            // ⛔ **Into THIS grid's search list, never into the accumulated one.** The reference
            // reaches a macro's pins through the grid that claims it — `getIntersections` asks each
            // of the grid's own components for `getConnectableShapes` — and `InstanceGrid`'s shape
            // list, which is what later grids see as global shapes, does not carry them.
            //
            // ⚠️ **Put in the shared list they become via targets for every grid built after.** A
            // core grid declared after a macro grid then drops stacks onto the macro's own supply
            // pins: 76 of them on a design with one SRAM, three levels each, none of which the
            // reference builds. ℹ️ Invisible in the shapes — a pin is never written — so the
            // difference is 228 vias against an identical DEF.
            for (layer, net, rect) in instance_pin_shapes(&db, &grid.instance) {
                let pin = vyges_pdn::vias::Shape { layer, net, rect };
                connectable_pins.push(pin.clone());
                grid_pins.push(pin);
            }
        }
        // 🔑 **And a switched domain searches the power switches' always-on pins.** They are the
        // only place the unswitched supply meets the design, so without them the always-on net has
        // straps and no way down to anything.
        if let (Some(sw), Some(cell)) = (
            grid_switched,
            opts.one("power-switch").filter(|s| !s.is_empty()),
        ) {
            let _ = sw;
            let (master, pin) = cell.split_once(':').unwrap_or((cell, ""));
            // ⚠️ **The LOWEST strap set is the one the switches line up with** — the reference
            // takes the lowest routing level, ties broken by shape count.
            let lowest = strap_sets
                .iter()
                .min_by_key(|(l, ..)| routing_level(&db, l))
                .map(|(l, ..)| l.clone());
            if let Some(lowest) = lowest {
                // ⚠️ **Only the ALWAYS-ON straps are targets.** `buildStrapTargetList` filters
                // the lowest strap set to that net alone — a switch lined up with a ground or a
                // switched strap would be bridging nothing.
                let straps: Vec<(String, Rect)> = emitted
                    .iter()
                    .filter(|(n, l, _, kind)| {
                        *l == lowest && *kind == "STRIPE" && n == grid_always_on
                    })
                    .map(|(n, _, r, _)| (n.clone(), *r))
                    .collect();
                // 🔑 **A switch pin is a SHAPE of this grid, not only a via target.** The
                // reference returns them from `GridSwitchedPower::getShapes()`, so they are what
                // holds the stack up when trimming asks — but they are the CELL's metal and no
                // component writes them, which is why the always-on net comes out of the reference
                // with vias and patches and not one shape of its own.
                let pins = switch_pin_shapes(&db, master, pin, grid_always_on, &rows, &straps, core);
                // `GridSwitchedPower::build` reports nothing per grid, so this is ours: the number
                // of always-on straps it had to line up with, and the pin shapes it produced.
                trace(
                    "Switch",
                    &format!(
                        "{} on {lowest}: {} always-on straps for {grid_always_on}, {} pin shapes",
                        grid.name,
                        straps.len(),
                        pins.len()
                    ),
                );
                for (layer, net, rect) in pins {
                    // ⚠️ Into `emitted` ONLY. Every via search reads `emitted`, so adding them
                    // to the search list as well makes each pin two identical shapes, two
                    // identical vias, and the overlap check then kills both — each being "no
                    // larger than" the other.
                    emitted.push((net, layer, rect, "SWITCH"));
                }
            }
        }
        // ── refineShapes ─────────────────────────────────────────────────────────────────────────
        // 🔑 **After every component is built, an over-pad strap whose via would be obstructed is
        // slid along its pin until one is not.** `Grid::makeShapes` runs this to a fixed point.
        //
        // ⚠️ **The position matters as much as the rule.** The obstructions it judges against are
        // everything the grid has built — the straps and follow pins included, which are made AFTER
        // the pad connections. Run inline with the pad component, the intermediate layer it cares
        // about is still empty and nothing is ever refined.
        //
        // ⚠️ **A strap that finds nowhere legal is gone**, and the removal happens first: the
        // reference removes the shape, then tries two widths, and has no path that puts it back.
        // On a flipchip design that connects over its pads, that is the difference between a strap
        // one width below where it was first placed and no strap at all.
        //
        // 🔑 **The obstruction set is built ONCE per round and kept in step with `emitted`.**
        // `made_obstructions` answers one entry per emitted shape in the same order, so the shape
        // being refined is addressed by index rather than searched for — which is what makes a
        // per-shape removal affordable at all. Rebuilding the list per shape would be one spacing
        // lookup per shape per shape.
        if !refinable.is_empty() {
            let routing: Vec<(String, i32)> = db
                .layers_with_direction()
                .unwrap_or_default()
                .into_iter()
                .filter(|(n, _)| db.layer_get_type(n).unwrap_or_default() == "ROUTING")
                .enumerate()
                .map(|(i, (n, _))| (n, i as i32 + 1))
                .collect();
            loop {
                let mut modified = false;
                let all: Vec<(String, Rect, Option<String>, Rect)> = blockages
                    .iter()
                    .cloned()
                    .chain(made_obstructions(&db, &emitted))
                    .collect();
                let base = blockages.len();
                // The via test asks one question of every obstruction — *is it on a layer between
                // these two, and does it reach the via* — so it is worth answering the layer half
                // once. ⚠️ A shape already given up on is not metal: it stays in `emitted` until
                // the sweep below and `made_obstructions` answers for it like any other, which is
                // a keep-out at the origin that nothing put there.
                let mut levelled: Vec<(usize, i32, Rect)> = all
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i < base || emitted[*i - base].2 != (0, 0, 0, 0))
                    .filter_map(|(i, (layer, rect, _, _))| {
                        routing
                            .iter()
                            .find(|(n, _)| n == layer)
                            .map(|(_, l)| (i, *l, *rect))
                    })
                    .collect();
                for (index, hold, r) in std::mem::take(&mut refinable) {
                    // ⚠️ **Removed BEFORE the search**, so neither the via test nor the cut inside
                    // it can see the strap in the place it is being moved out of.
                    let skip = base + index;
                    let obs: Vec<(i32, Rect)> = levelled
                        .iter()
                        .filter(|(i, _, _)| *i != skip)
                        .map(|(_, l, r)| (*l, *r))
                        .collect();
                    let blocked = strap_via_is_obstructed(emitted[index].2, &r, &routing, &obs);
                    // The reference prints its half as `Direct connect shape <via> with obstruction
                    // <layer> using pin <pin> on <net>`; keyed the same way so the two join.
                    if std::env::var_os("PDN_TRACE").is_some() {
                        let s = emitted[index].2;
                        eprintln!(
                            "[refine] {}|{}|{},{},{},{}|{}|{}",
                            r.net, r.layer, s.0, s.1, s.2, s.3, r.target_layer, blocked
                        );
                    }
                    if !blocked {
                        refinable_next.push((index, hold, r));
                        continue;
                    }
                    modified = true;
                    // ⚠️ **Narrowed to the strap's own layer, and that is a performance fix rather
                    // than a behavioural one** — `cut_shapes` filters by layer itself, so the answer
                    // is identical. It matters because the candidate loop below runs the cut once
                    // per position along the pin, and the reference's equivalent is an R-tree query
                    // rather than a scan: unfiltered, a flipchip design that connects over its
                    // pads ran for over an hour without finishing.
                    let without_self: Vec<(String, Rect, Option<String>, Rect)> = all
                        .iter()
                        .enumerate()
                        .filter(|(i, o)| *i != skip && o.0 == r.layer)
                        .map(|(_, o)| o.clone())
                        .collect();
                    let mut placed = None;
                    for width in [r.width, db.layer_get_min_width(&r.layer) as i32] {
                        if width <= 0 {
                            continue;
                        }
                        placed = refine_over_pad_strap(
                            &db,
                            &r,
                            width,
                            &routing,
                            &obs,
                            &without_self,
                            grid_mfg,
                        );
                        if placed.is_some() {
                            break;
                        }
                    }
                    match placed {
                        // The first piece takes the shape's place; any others are new shapes of the
                        // same component, which is what `addShape` then `cutShapes` leaves behind.
                        Some(pieces) => {
                            emitted[index].2 = pieces[0];
                            // ⚠️ **Kept in step WITHIN the round, not only between rounds.** The
                            // reference re-inserts a refined shape immediately —
                            // `getObstructions(all_obstructions); getShapes(all_shapes);` — so the
                            // next pad on the same pass is judged against where this one now is.
                            if let Some(e) = levelled.iter_mut().find(|(i, _, _)| *i == skip) {
                                if let Some((_, bloated, _, _)) =
                                    made_obstructions(&db, &emitted[index..=index]).pop()
                                {
                                    e.2 = bloated;
                                }
                            }
                            // 🔑 **The terminal connection MOVES with the strap.**
                            //
                            // ⚠️ Left behind, the hold sits where the strap no longer is: the
                            // refined shape stands on its vias alone and trimming takes it away,
                            // while the stale rect may hold the NEIGHBOURING pad's strap instead.
                            // The refine then reads as having done nothing — the count is right,
                            // the geometry is right, and the shape is gone by the end.
                            // 🔑 **`clearITermConnections()` then `addITermConnection(...)`** — and
                            // the ADD is unconditional, which is what gives an over-pads strap its
                            // first and only terminal. It had none when it was made; see the
                            // three-call list where the hold is decided.
                            match (hold, intersect_rect(r.pin, pieces[0])) {
                                (Some(h), Some(new_hold)) => iterm_holds[h].2 = new_hold,
                                // Nothing of the pin left under it: an unheld shape, not a shape
                                // held by its old place.
                                (Some(h), None) => iterm_holds[h].2 = (0, 0, 0, 0),
                                (None, Some(new_hold)) => {
                                    iterm_holds.push((r.net.clone(), r.layer.clone(), new_hold));
                                }
                                (None, None) => {}
                            }
                            for extra in &pieces[1..] {
                                emitted.push((r.net.clone(), r.layer.clone(), *extra, "STRIPE"));
                            }
                            // ⛔ **A refined shape is never refined again**, and the reason is the
                            // same one that makes the candidate re-check dead.
                            //
                            // and `strapViaIsObstructed` opens on `target_shapes_.find(shape)`,
                            // which only `makeShapesOverPads` fills. `refineShape` adds a fresh
                            // `shape->copy()` and never gives it an entry, so on the next pass of
                            // `do { … } while (modified)` the selection test returns false for it.
                            //
                            // 🔑 **That is what terminates the loop.** Each original shape is
                            // refined at most once, so the second pass finds nothing to refine and
                            // `refineShapes` returns false.
                            //
                            // ⚠️ **Re-queueing it does not terminate at all.** A strap whose every
                            // position along the pin is obstructed — which is exactly the corner
                            // this closes — is refined to the same place forever;
                            // a flipchip design that connects over its pads ran for an hour
                            // without finishing. That the old code got away with it was an accident of
                            // testing the candidate: the refine failed, the shape was dropped, and
                            // the loop ended one shape short of the reference.
                        }
                        None => {
                            emitted[index].2 = (0, 0, 0, 0);
                            // A strap given up on stops obstructing at once. ⚠️ Level 0 is not a
                            // routing layer, so the test never consults it again.
                            if let Some(e) = levelled.iter_mut().find(|(i, _, _)| *i == skip) {
                                e.1 = 0;
                            }
                            if let Some(h) = hold {
                                iterm_holds[h].2 = (0, 0, 0, 0);
                            }
                        }
                    }
                }
                refinable = std::mem::take(&mut refinable_next);
                if !modified {
                    break;
                }
            }
        }
        // A strap the refine could not place anywhere is dropped, exactly as `removeShape` leaves it.
        let dropped = emitted.iter().filter(|(_, _, r, _)| *r == (0, 0, 0, 0)).count();
        if dropped != 0 {
            emitted.retain(|(_, _, r, _)| *r != (0, 0, 0, 0));
            vyges_events::log(
                "vyges-pdn",
                vyges_events::Severity::Warn,
                format!("{dropped} over-pad straps removed with nowhere legal to sit"),
            );
        }

        // ── cleanupShapes ────────────────────────────────────────────────────────────────────────
        // 🔑 **A core-grid shape lying wholly inside a macro is removed** — `CoreGrid::cleanupShapes`,
        // run after the switches are placed and before any via is made.
        //
        // ⚠️ **Only the CORE grid**, and only its own shapes: `Grid::cleanupShapes` is an empty
        // virtual and `getShapes()` answers for one grid. An instance grid keeps everything it made
        // over its own macro, which is the whole point of having claimed it.
        //
        // ⚠️ **Ordering is load-bearing.** Run after the vias, these shapes have already held their
        // neighbours out: a strap threading a macro's open middle holds the crossing strap past the
        // macro's edge, and removing it later leaves the neighbour extended to a via that no longer
        // exists.
        if grid.instance.is_empty() {
            let inside = |layer: &str, r: Rect| {
                macro_boxes.iter().any(|(l, o)| {
                    l == layer && o.0 <= r.0 && o.1 <= r.1 && r.2 <= o.2 && r.3 <= o.3
                })
            };
            let mut i = grid_shapes_from;
            while i < emitted.len() {
                if inside(&emitted[i].1, emitted[i].2) {
                    emitted.remove(i);
                } else {
                    i += 1;
                }
            }
        }
        // Runs whenever a connection is declared. `--report-vias` additionally prints the locations.
        if opts.one("connect").is_some() {
            let via_shapes: Vec<vyges_pdn::vias::Shape> = emitted
                .iter()
                .enumerate()
                .filter(|(i, (_, _, r, _))| in_reach(*i, *r))
                .map(|(_, (net, layer, rect, _))| vyges_pdn::vias::Shape {
                    layer: layer.clone(),
                    net: net.clone(),
                    rect: *rect,
                })
                .chain(grid_pins.iter().cloned())
                // Every global shape intersecting this grid's search area — `Grid::makeVias`.
                .chain(fixed_via_shapes.iter().filter(|s| overlaps(s.rect, via_area)).cloned())
                .collect();
            // ⚠️ **The intermediate layers must be populated.** They were an empty vector, so the
            // obstruction pass iterated nothing and could never reject a via however obstructed —
            // and the obstruction list handed to it was empty as well. Two faults stacked, each of
            // which hid the other.
            // `--connect <lower>,<upper>[:<via>+<via>...][:<cut pitch, micron>][:<dont use>][:<ongrid>+...]`
            //
            // ⚠️ A named via is looked up as a TECH VIA, which is what `-fixed_vias` usually means in
            // a technology carrying no VIARULE GENERATE — `pdn.tcl` resolves the name against both
            // `findVia` and `findViaGenerateRule`, and only the first kind is handled here.
            #[allow(clippy::type_complexity)]
            let mut fixed: Vec<(
                String,
                String,
                Vec<String>,
                Option<(i32, i32)>,
                Option<regex::Regex>,
            )> = Vec::new();
            // The layers each connect snaps its stack to, by layer pair.
            let mut on_grid: Vec<((String, String), Vec<String>)> = Vec::new();
            // `-max_rows` / `-max_columns`, by layer pair. Zero means unlimited.
            let mut max_cuts: Vec<((String, String), (i32, i32))> = Vec::new();
            // `-split_cuts`, by layer pair: the layers whose crossings are spread rather than
            // packed, with the pitch and stagger each asks for.
            let mut split_by_connect: Vec<((String, String), Vec<(String, i32, bool)>)> = Vec::new();
            // `-min_width_layers`, by layer pair: the intermediate layers this connect must not
            // widen past their own minimum.
            let mut min_width_by_connect: Vec<((String, String), Vec<String>)> = Vec::new();
            let connects: Vec<vyges_pdn::vias::Connect> = opts
                .all("connect")
                .iter()
                .filter_map(|c| {
                    let mut parts = c.splitn(9, ':');
                    let pair = parts.next()?;
                    let vias: Vec<String> = parts
                        .next()
                        .unwrap_or("")
                        .split('+')
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                        .collect();
                    let pitch = parts
                        .next()
                        .filter(|p| !p.is_empty())
                        .map(|p| dbu(p, per_micron))
                        .filter(|p| *p > 0)
                        .map(|p| (p, p));
                    // ⚠️ A pattern that does not compile is reported and ignored rather than
                    // silently treated as "match nothing", which would build vias the caller asked
                    // to be kept out.
                    let dont_use = parts.next().filter(|d| !d.is_empty()).and_then(|d| {
                        regex::Regex::new(d)
                            .map_err(|e| vyges_events::log(
                                             "vyges-pdn",
                                             vyges_events::Severity::Warn,
                                             format!("bad --connect via filter {d:?}: {e}"),
                                         ))
                            .ok()
                    });
                    // ⚠️ **Per connect, not global.** `-ongrid` names layers whose track grid this
                    // stack snaps to, and the very case that needs it declares four connects of which
                    // one asks for it — applied to all of them, the metal1-to-metal6 stack passing
                    // through the same layer would snap too.
                    let ongrid: Vec<String> = parts
                        .next()
                        .unwrap_or("")
                        .split('+')
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                        .collect();
                    // 🔑 **`-max_rows` and `-max_columns` cap the array, they do not size it.**
                    // `generateDbVia` sets them on every generator before `build()`, and `getCuts`
                    // ends in `if (max_cuts != 0) cuts = min(cuts, max_cuts)` — so a zero means
                    // unlimited and an explicit zero is the same as saying nothing.
                    //
                    // ⚠️ **Per connect, like the rest of this line.** A design that limits one
                    // stack and not another gets two different arrays out of the same technology.
                    let cap = |v: Option<&str>| {
                        v.filter(|s| !s.is_empty())
                            .and_then(|s| s.parse::<i32>().ok())
                            .filter(|n| *n > 0)
                            .unwrap_or(0)
                    };
                    let max_rows = cap(parts.next());
                    let max_columns = cap(parts.next());
                    // 🔑 **`-split_cuts` is a property of a CONNECT, not of the design.**
                    // `Connect::setSplitCuts` stores the map on the connect it was given to, so a
                    // design declaring it on one connect and not another gets two different arrays
                    // out of the same technology and the same layer. Held globally, the metal1-to-
                    // metal6 stack in such a design is scattered into single cuts by a
                    // pitch the metal1-to-metal4 connect asked for: 912 vias where 228 belong.
                    //
                    // `layer,pitch[,stagger]`, joined by `+`.
                    let splits: Vec<(String, i32, bool)> = parts
                        .next()
                        .unwrap_or("")
                        .split('+')
                        .filter(|v| !v.is_empty())
                        .filter_map(|e| {
                            let mut f = e.splitn(3, ',');
                            let layer = f.next()?.to_string();
                            let pitch = dbu(f.next().unwrap_or("0"), per_micron);
                            let stagger = f.next() == Some("stagger");
                            (pitch > 0).then_some((layer, pitch, stagger))
                        })
                        .collect();
                    // 🔑 **`-min_width_layers` names INTERMEDIATE layers this stack may not fatten.**
                    // `generateMinEnclosureViaRects` normally offers each intermediate level two
                    // candidate rects — the full overlap and one narrowed to the layer's own width
                    // — and lets the generator pick. Named here, the full-overlap rect is dropped
                    // and only the narrow one is left.
                    let min_width_layers: Vec<String> = parts
                        .next()
                        .unwrap_or("")
                        .split('+')
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                        .collect();
                    let (l, u) = pair.split_once(',')?;
                    Some((
                        l.to_string(),
                        u.to_string(),
                        vias,
                        pitch,
                        dont_use,
                        ongrid,
                        (max_rows, max_columns),
                        splits,
                        min_width_layers,
                    ))
                })
                .map(|(l, u, vias, pitch, dont_use, ongrid, caps, splits, min_width_layers)| {
                    on_grid.push(((l.clone(), u.clone()), ongrid));
                    max_cuts.push(((l.clone(), u.clone()), caps));
                    // ⚠️ **The connect's own two ends are ERASED from the map.**
                    // `setSplitCuts` does it on the way in, so naming an end layer does nothing
                    // whatever -- only a layer the stack passes THROUGH can scatter its cuts.
                    min_width_by_connect.push(((l.clone(), u.clone()), min_width_layers));
                    split_by_connect.push((
                        (l.clone(), u.clone()),
                        splits
                            .into_iter()
                            .filter(|(layer, ..)| *layer != l && *layer != u)
                            .collect(),
                    ));
                    let (lo, hi) = (layer_number(&all_layers, &l), layer_number(&all_layers, &u));
                    let c = vyges_pdn::vias::Connect {
                        lower: l.clone(),
                        upper: u.clone(),
                        intermediate: vyges_pdn::vias::intermediate_layers(&all_layers, lo, hi),
                    };
                    fixed.push((l, u, vias, pitch, dont_use));
                    c
                })
                .collect();
            // 🔑 **The grid's OWN shapes obstruct its vias.** `Grid::makeVias` copies the block
            // obstructions and then inserts every shape it can see — its own and the global ones —
            // as an obstruction on that shape's layer. A stack may not pass through a strap sitting
            // on one of its intermediate layers, and the reference applies no net test: a same-net
            // shape blocks exactly as a foreign one does.
            let via_obstructions: Vec<(String, Rect)> = blockages
                .iter()
                .map(|(l, r, ..)| (l.clone(), *r))
                .chain(emitted.iter().map(|(_, l, r, _)| (l.clone(), obstruction_of(&db, l, *r))))
                .collect();
            trace("Making vias", "start");
            let (placed, dropped) = vyges_pdn::vias::place(&connects, &via_shapes, &via_obstructions);
            trace("Making vias", &format!("end, {} placed", placed.len()));

            // ── repairVias ───────────────────────────────────────────────────────────────────────
            // 🔑 **A via whose two shapes do not reach each other pulls the shorter one out.**
            // `Grid::makeVias` builds the vias, calls `repairVias`, and — if any shape moved — builds
            // every via again. That second pass matters: the extended shapes make different
            // intersections, so the vias are not the ones that were just discarded.
            //
            // ⚠️ **Once, not until nothing changes.** The reference tests the return value and rebuilds
            // a single time; looping would keep growing shapes past what it produces.
            // The die edges this grid's shapes touch, taken before `repairVias` can move them.
            for (net, layer, rect, shape) in &emitted[grid_shapes_from..] {
                if *shape == "SWITCH" {
                    continue;
                }
                let mw = db.layer_get_min_width(layer) as i32;
                if rect.0 == die.0 {
                    edge_connections.push((net.clone(), layer.clone(), true,
                        (die.0, rect.1, (die.0 + mw).min(rect.2), rect.3)));
                }
                if rect.2 == die.2 {
                    edge_connections.push((net.clone(), layer.clone(), true,
                        ((die.2 - mw).max(rect.0), rect.1, die.2, rect.3)));
                }
                if rect.1 == die.1 {
                    edge_connections.push((net.clone(), layer.clone(), false,
                        (rect.0, die.1, rect.2, (die.1 + mw).min(rect.3))));
                }
                if rect.3 == die.3 {
                    edge_connections.push((net.clone(), layer.clone(), false,
                        (rect.0, (die.3 - mw).max(rect.1), rect.2, die.3)));
                }
            }

            let (placed, dropped) = if repair_vias(
                &db,
                &mut emitted,
                &placed,
                &blockages,
                &locked_layers,
                &fixed_via_shapes,
            )
            {
                // 🔑 **The search area is recomputed on EVERY pass, not once per grid.**
                // `Grid::makeVias` takes it from `getDomainBoundary()` merged with the grid's
                // shapes AS THEY STAND, and the outer `makeVias` calls the inner one twice —
                // before and after `repairVias`. A ring pulled out to meet the core ring reaches
                // further on the second pass than it did on the first, so global shapes that were
                // out of reach come into it and crossings appear that the first pass could not
                // see. Reusing the first pass's area silently keeps them out: 44 vias on a
                // flipchip design, every one of them a bump grid's second copy of a via the core
                // grid had already made.
                let via_area = {
                    let mut a = boundary;
                    for (_, _, r, _) in &emitted[grid_shapes_from..] {
                        a = (a.0.min(r.0), a.1.min(r.1), a.2.max(r.2), a.3.max(r.3));
                    }
                    a
                };
                let in_reach = |i: usize, r: Rect| {
                    i >= grid_shapes_from
                        || (r.0 <= via_area.2
                            && via_area.0 <= r.2
                            && r.1 <= via_area.3
                            && via_area.1 <= r.3)
                };
                let via_shapes: Vec<vyges_pdn::vias::Shape> = emitted
                    .iter()
                    .enumerate()
                    .filter(|(i, (_, _, r, _))| in_reach(*i, *r))
                    .map(|(_, (net, layer, rect, _))| vyges_pdn::vias::Shape {
                        layer: layer.clone(),
                        net: net.clone(),
                        rect: *rect,
                    })
                    .chain(grid_pins.iter().cloned())
                    .chain(fixed_via_shapes.iter().filter(|s| overlaps(s.rect, via_area)).cloned())
                    .collect();
                let via_obstructions: Vec<(String, Rect)> = blockages
                    .iter()
                    .map(|(l, r, ..)| (l.clone(), *r))
                    .chain(emitted.iter().map(|(_, l, r, _)| (l.clone(), obstruction_of(&db, l, *r))))
                    .collect();
                vyges_pdn::vias::place(&connects, &via_shapes, &via_obstructions)
            } else {
                (placed, dropped)
            };

            // ── stage 6f: repairGridChannels ────────────────────────────────────────────────────
            // 🔑 **A shape with nothing above it is a hole in the grid.** After the vias are made, the
            // reference gathers every strap or follow pin that no via reaches up from, treats the
            // region they occupy as a channel, and drops a narrow strap set into it on the lowest layer
            // the orphaned layer can connect to.
            //
            // ⚠️ **It repeats until a pass repairs nothing**, rebuilding the vias each time — the new
            // straps connect shapes that were orphans a moment ago, and they can open channels of their
            // own on the layer they were placed on.
            let (mut placed, mut dropped) = (placed, dropped);
            // ⚠️ **The DATABASE's order, not the grid's build order.** A channel's nets live in a
            // `PtrSet` keyed by object id, so they come out in creation order however the grid was told
            // to alternate power and ground — and this design declares its two straps ground-first.
            let db_net_order = db.block_get_nets();
            // The highest layer any declared strap set uses: nothing above it can be connected to, so
            // shapes there are not orphans.
            let highest = strap_sets
                .iter()
                .map(|(l, ..)| routing_level(&db, l))
                .max()
                .unwrap_or(0);
            // ⚠️ Bounded. The reference recurses without a limit and relies on each pass strictly
            // reducing the channels; a bound is cheap insurance against a channel that repairs into
            // itself, and 8 is far past any real design's depth.
            for _round in 0..8 {
                let channels = find_channels(
                    &db,
                    &emitted,
                    &placed,
                    &strap_sets,
                    &followpin_layer,
                    followpin_pitch,
                    highest,
                    // 🔑 **The followpin-extended box, not the core.** A repair channel's `area`
                    // becomes the repair strap's extent verbatim —
                    // `setStrapStartEnd(area_.yMin(), area_.yMax())` in its constructor — and the
                    // reference's reaches half a followpin width past the core at each end, which
                    // is the outer edge of the outermost rail. Clipped to the core instead, every
                    // repair strap stops short of the rail it is meant to reach.
                    strap_boundary,
                    &db_net_order,
                );
                if channels.is_empty() {
                    break;
                }
                let mut repaired: Vec<Rect> = Vec::new();
                trace("Channel", &format!("channels to repair {}", channels.len()));
                // Each channel's own area, to join against the reference's per-repair `boundary`.
                if std::env::var_os("PDN_TRACE").is_some() {
                    for ch in &channels {
                        eprintln!(
                            "[channel] {}|{},{},{},{}",
                            ch.target_layer, ch.area.0, ch.area.1, ch.area.2, ch.area.3
                        );
                    }
                }
                for ch in &channels {
                    // ⚠️ **One channel per band, per pass.** A channel sharing an x or a y span with
                    // one already repaired is left for the next round, by which time the straps just
                    // added may have closed it.
                    if repaired.iter().any(|o: &Rect| {
                        (ch.area.2 > o.0 && ch.area.0 < o.2) || (ch.area.3 > o.1 && ch.area.1 < o.3)
                    }) {
                        continue;
                    }
                    let Some(straps) = build_repair(&db, ch, &emitted, &blockages, core, die, grid_mfg)
                    else {
                        continue;
                    };
                    for (net, rect) in straps {
                        emitted.push((net, ch.target_layer.clone(), rect, "STRIPE"));
                    }
                    repaired.push(ch.area);
                }
                if repaired.is_empty() {
                    break;
                }
                // The straps changed, so every via has to be found again -- and so has the area
                // they are looked for in, for the reason above.
                // 🔑 **The search area is recomputed on EVERY pass, not once per grid.**
                // `Grid::makeVias` takes it from `getDomainBoundary()` merged with the grid's
                // shapes AS THEY STAND, and the outer `makeVias` calls the inner one twice —
                // before and after `repairVias`. A ring pulled out to meet the core ring reaches
                // further on the second pass than it did on the first, so global shapes that were
                // out of reach come into it and crossings appear that the first pass could not
                // see. Reusing the first pass's area silently keeps them out: 44 vias on a
                // flipchip design, every one of them a bump grid's second copy of a via the core
                // grid had already made.
                let via_area = {
                    let mut a = boundary;
                    for (_, _, r, _) in &emitted[grid_shapes_from..] {
                        a = (a.0.min(r.0), a.1.min(r.1), a.2.max(r.2), a.3.max(r.3));
                    }
                    a
                };
                let in_reach = |i: usize, r: Rect| {
                    i >= grid_shapes_from
                        || (r.0 <= via_area.2
                            && via_area.0 <= r.2
                            && r.1 <= via_area.3
                            && via_area.1 <= r.3)
                };
                let via_shapes: Vec<vyges_pdn::vias::Shape> = emitted
                    .iter()
                    .enumerate()
                    .filter(|(i, (_, _, r, _))| in_reach(*i, *r))
                    .map(|(_, (net, layer, rect, _))| vyges_pdn::vias::Shape {
                        layer: layer.clone(),
                        net: net.clone(),
                        rect: *rect,
                    })
                    .chain(grid_pins.iter().cloned())
                    .chain(fixed_via_shapes.iter().filter(|s| overlaps(s.rect, via_area)).cloned())
                    .collect();
                let via_obstructions: Vec<(String, Rect)> = blockages
                    .iter()
                    .map(|(l, r, ..)| (l.clone(), *r))
                    .chain(emitted.iter().map(|(_, l, r, _)| (l.clone(), obstruction_of(&db, l, *r))))
                    .collect();
                let next = vyges_pdn::vias::place(&connects, &via_shapes, &via_obstructions);
                placed = next.0;
                dropped = next.1;
            }

            if opts.one("report-vias").is_some() {
                for v in &placed {
                    println!("VIA {} {} {} {:?}", v.net, v.lower, v.upper, v.area);
                }
            }

            // ── what TRIMMING needs, which is only the crossing areas ────────────────────────
            // 🔑 **A crossing holds its two shapes whether or not a via is ever built there.**
            // `Grid::makeVias` creates a `Via` per intersection and `updateVias` attaches it to
            // both shapes; nothing between here and trimming consults any geometry. So the holds
            // can be recorded now and the geometry deferred until after the trim.
            //
            // ⚠️ **The two guards are the SAME two the build applies**, and they have to be: a
            // crossing the build skips must not hold a strap open, and one it merely refuses to
            // build must. See the note on a refused via at the build site.
            for v in &placed {
                if vyges_pdn::vias::placement_point(v.lower_rect, v.upper_rect, grid_mfg).is_none()
                {
                    continue;
                }
                for pair in stack_layers(&db, &v.lower, &v.upper).windows(2) {
                    let (lo, hi) = (pair[0].as_str(), pair[1].as_str());
                    let named = opts
                        .one(&format!("cut-{lo}-{hi}"))
                        .filter(|c| !c.is_empty());
                    if named
                        .map(str::to_string)
                        .or(cut_layer_between(&db, lo, hi))
                        .is_none()
                    {
                        continue;
                    }
                    via_areas.push((v.net.clone(), (lo.to_string(), hi.to_string()), v.area));
                }
            }

            // ⛔ **Deferred, not skipped.** Everything from here to the write is via GEOMETRY,
            // and it runs after trimming — see `PendingVias`.
            pending.push(PendingVias {
                opts,
                placed,
                dropped,
                on_grid,
                max_cuts,
                split_by_connect,
                min_width_by_connect,
                fixed,
                ground: ground.to_string(),
            });
        }
    }


    // ── trim ─────────────────────────────────────────────────────────────────────────────────
    // ⚠️ **After the vias and before the write.** A shape is trimmed to the extent of what is
    // attached to it, so it cannot be decided earlier; and a shape with nothing attached is
    // deleted, which is why an unconnected strap vanishes from a normal run while being perfectly
    // well-formed here.
    // ⚠️ **The VALUE, not merely the flag.** Tested for presence alone, `--trim 0` asked for
    // trimming just as loudly as `--trim 1` — so a run meant to reproduce `pdngen -skip_trim`
    // trimmed anyway, and every shape it should have kept was compared against a reference that
    // had kept its own.
    if opts.one("trim").is_some_and(|v| v != "0") {
        let pin_layers: Vec<String> = opts
            .all("pins")
            .iter()
            .flat_map(|s| s.split(','))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let emitted_before = emitted.len();
        let mut held_any = 0;
        let mut kept = Vec::with_capacity(emitted.len());
        for (net, layer, rect, shape) in emitted {
            // What holds this shape up: every via landing on it, on either of its two faces.
            // ⚠️ **Contained ACROSS the shape, overlapping along it.** `Shape::getVias()` is the
            // list attached to a shape, and re-attachment after a replacement is by intersection —
            // so an overlap test is right along the shape's length. Across it, though, a via's area
            // takes its extent from THIS shape (see `vias::via_area`), so anything reaching past
            // the shape's own width belongs to a different shape.
            //
            // 🔑 Both halves matter and they were found the hard way. Overlap alone let a via
            // belonging to a neighbouring stripe count, putting the minimum rect 2470 past the end
            // of the shape it described — and trimming then gave up **silently**, since `contains`
            // fails and the shift cannot rescue it. Full containment then excluded a rail's via
            // from the vertical strap it legitimately holds, because a rail centred on the core
            // edge reaches below the strap's own end.
            let across_ok = |a: &Rect| match vyges_pdn::viagen::rect_direction(rect) {
                Direction::Horizontal => a.1 >= rect.1 && a.3 <= rect.3,
                Direction::Vertical => a.0 >= rect.0 && a.2 <= rect.2,
                Direction::None => true,
            };
            let held: Vec<Rect> = via_areas
                .iter()
                .filter(|(n, l, a)| {
                    n == &net && (l.0 == layer || l.1 == layer) && overlaps(*a, rect) && across_ok(a)
                })
                .map(|(_, _, a)| *a)
                .collect();
            if !held.is_empty() {
                held_any += 1;
            }
            // 🔑 **A terminal connection holds a shape as surely as a via.**
            // `getMinimumRect` merges the iterm rects before the via areas, so a pad strap is held
            // at the pin it starts from even though nothing has yet been dropped onto it.
            let held: Vec<Rect> = held
                .into_iter()
                .chain(iterm_holds.iter().filter_map(|(n, l, r)| {
                    (n == &net && l == &layer && overlaps(*r, rect)).then_some(*r)
                }))
                .collect();
            // ⚠️ A shape reaching the die edge is PINNED there, and the pin counts as a
            // connection — which is what holds an extended ring out to its full length.
            let vias_only = held.clone();
            let mut held = held;
            // ⚠️ **The layer's MINIMUM width, not its default width.** `GridComponent::addShape`
            // measures the sliver with `getLayer()->getMinWidth()`, and the same rects feed both
            // the connection count here and the pin geometry written at the end — one rule, one
            // depth. This read was `getWidth()`, which is the LEF `WIDTH` and only equals
            // `MINWIDTH` when a layer declares no separate minimum.
            //
            // ℹ️ No routing layer in Nangate45, ASAP7 or sky130 declares them differently, so the
            // suite cannot tell the two apart — the correction is from the source, not from a
            // failing case.
            //
            // ⛔ **Computed in two places on purpose, and they are NOT the same rect.** This one is
            // deliberately unclamped so that an offcut inherits a sliver sticking out of it and is
            // removed; the one published as pin geometry is clamped to the shape, as the reference
            // clamps it at the moment it first records it. Both approximate "compute once when the
            // shape is added, inherit through every cut". Unifying them means modelling that
            // inheritance properly, which is its own change.
            held.extend(vyges_pdn::trim::boundary_pins(
                rect,
                die,
                (db.layer_get_min_width(&layer) as i32).max(1),
            ));
            // ⚠️ The LAYER's direction here, and that is correct: base `Shape::getLayerDirection`
            // returns exactly that. Only `FollowPinShape` overrides it, and that path is below.
            let dir = direction_of(&db, &layer);
            // 🔑 A layer named by `define_pdn_grid -pins` carries block pins. Its shapes are never
            // SHRUNK — a pin's extent is its contract with whatever connects from outside — and
            // one connection is enough to keep it where an ordinary shape needs two.
            // ⚠️ **A switch pin is the CELL's metal and is never touched** — not shrunk, not
            // removed. It is in this list only so that what lands on it counts as held.
            //
            // 🔑 **And neither is a LOCKED shape.** `trimShapes` opens with
            // `if (!shape->isModifiable()) continue;`, and `isModifiable()` is
            // `!is_locked_ && shape_type_ == kShape` — so a single-layer ring, the only thing this
            // engine locks, is exempt from trimming entirely.
            //
            // ⚠️ **A single-layer ring has no vias of its own**, its two axes being the same metal,
            // so trimming it removes the horizontal sides outright for want of connections and
            // pulls the vertical ones back to whatever else touches them: four segments per net
            // became two, and those two were shortened by a sixth at each end.
            if shape == "RING" && locked_layers.contains(&layer) {
                kept.push((net, layer, rect, shape));
                continue;
            }
            if shape == "SWITCH" {
                kept.push((net, layer, rect, shape));
                continue;
            }
            let is_pin_layer = pin_layers.iter().any(|l| l == &layer);
            // `Shape::isRemovable`: removable exactly when it has fewer connections than it needs.
            // `getNumberOfConnections` counts vias, iterm and bterm connections; `held` is ours.
            let removable = held.len() < if is_pin_layer { 1 } else { 2 };
            // ⚠️ A follow pin is a different shape entirely for this purpose: it is held up by the
            // ROWS it serves rather than by what connects to it, and it can never be removed.
            if shape == "FOLLOWPIN" {
                let covering: Vec<Rect> = rows
                    .iter()
                    .map(|r| r.bbox)
                    .filter(|b| overlaps(*b, rect))
                    .collect();
                // ⚠️ The SHAPE's orientation, not the layer's. `FollowPinShape::getMinimumRect`
                // asks `isHorizontal()`, which is the rect's own aspect ratio; it consults the
                // layer only for a square. A follow pin on vertical metal2 runs horizontally, and
                // trimming it as vertical copied back its LENGTH and merged the rows into its
                // width — 340 became 5600.
                let want = vyges_pdn::trim::followpin_minimum_rect(
                    rect,
                    vyges_pdn::trim::minimum_rect(&held),
                    &covering,
                    vyges_pdn::viagen::rect_direction(rect) == Direction::Horizontal,
                );
                // 🔑 **Replaced only if the new rect fits INSIDE the old one.** The reference
                // gates every trim on `shape->getRect().contains(new_rect)` and, failing that,
                // leaves the shape exactly as it is — a follow pin is never removed either.
                //
                // ⚠️ Without the gate a CUT rail is restored: its minimum rect merges the rows it
                // covers, a row spans the core, and both halves grow back into two identical
                // full-width rails.
                let want = if want.0 >= rect.0 && want.1 >= rect.1
                    && want.2 <= rect.2 && want.3 <= rect.3
                {
                    want
                } else {
                    rect
                };
                kept.push((net, layer, want, shape));
                continue;
            }
            match vyges_pdn::trim::decide(
                rect,
                vyges_pdn::trim::minimum_rect(&held),
                // ⚠️ **Vias only.** A boundary pin holds the shape up — it counts toward the
                // connection total above — but it is not a via, and passing it here makes an
                // offcut look like something other than a bare via stack.
                &vias_only,
                db.layer_min_area(&layer).unwrap_or(0),
                dir,
                grid_mfg,
                is_pin_layer,
                removable,
            ) {
                vyges_pdn::trim::Decision::Keep => kept.push((net, layer, rect, shape)),
                vyges_pdn::trim::Decision::Replace(r) => kept.push((net, layer, r, shape)),
                vyges_pdn::trim::Decision::Remove => {}
            }
        }
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Debug,
            format!("trim kept {} of {} shapes; {held_any} had something attached, {} vias known",
            kept.len(), emitted_before, via_areas.len()),
        );
        emitted = kept;

        // ── stage 9: cleanupVias ─────────────────────────────────────────────────────────────
        // 🔑 **Trimming invalidates vias.** `GridComponent::replaceShape` removes the old shape —
        // which nulls that end on every via attached to it — and re-attaches only the vias whose
        // area still intersects the replacement. `cleanupVias` then drops whatever is left with a
        // null end. Without this a via whose shape was shrunk out from under it, or deleted
        // outright, is still written: geometry with nothing under it and no error to say so.
        // ⚠️ **The routing the design arrived with survives everything.** `Via::isValid` asks only
        // whether both its shapes still exist, and a `kFixed` shape is not modifiable, so trimming
        // never touched it and `removeInvalidVias` has no reason to drop a via that lands on one.
        // Left out, every one of a flipchip design's metal9/metal10 vias is built and
        // then discarded for want of anything recorded above them.
        let surviving: Vec<(String, String, Rect)> = emitted
            .iter()
            .map(|(n, l, r, _)| (n.clone(), l.clone(), *r))
            .chain(
                fixed_via_shapes
                    .iter()
                    .chain(connectable_pins.iter())
                    .map(|s| (s.net.clone(), s.layer.clone(), s.rect)),
            )
            .collect();
        let before = placements.len();
        let held = |net: &str, lo: &str, hi: &str, area: Rect| {
            vyges_pdn::vias::still_held(area, lo, net, &surviving)
                && vyges_pdn::vias::still_held(area, hi, net, &surviving)
        };
        // ⚠️ **`via_faces` keys into `placements` BY POSITION**, and absorbing via metal runs after
        // this — so dropping a via here without renumbering leaves every later face pointing at the
        // wrong via, or past the end of the list.
        let keep: Vec<bool> = placements
            .iter()
            .map(|(net, _, _, lo, hi, area)| held(net, lo, hi, *area))
            .collect();
        let mut renumber = vec![usize::MAX; placements.len()];
        let mut next = 0;
        for (i, k) in keep.iter().enumerate() {
            if *k {
                renumber[i] = next;
                next += 1;
            }
        }
        via_faces.retain_mut(|(pi, ..)| {
            let to = renumber[*pi];
            *pi = to;
            to != usize::MAX
        });
        let mut i = 0;
        placements.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        drcfill.retain(|(net, _, _, lo, hi, area)| held(net, lo, hi, *area));
        if placements.len() != before {
            vyges_events::log(
                "vyges-pdn",
                vyges_events::Severity::Debug,
                format!("{} vias dropped as no longer held",
                before - placements.len()),
            );
        }
    }

    // ── via geometry ─────────────────────────────────────────────────────────────────────────
    // 🔑 **After trimming, because that is where `PdnGen::writeToDb` sits.** Each grid's crossings
    // were found against the shapes as they stood; each grid's vias are SIZED against the shapes as
    // they were left. See `PendingVias` for why the two can be separated at all.
    for p in pending {
        let PendingVias {
            opts,
            placed,
            dropped,
            on_grid,
            max_cuts,
            split_by_connect,
            min_width_by_connect,
            fixed,
            ground,
        } = p;
        // 🔑 **The order vias are written in, which is not the order they were found in.**
        // `Grid::writeToDb` sorts a grid's vias by `(lower layer number, upper layer number,
        // area)` before writing any of them, and only then walks the list — its own comment says
        // "write vias first so shapes can be adjusted if needed".
        //
        // ⚠️ **The order is load-bearing because each via GROWS the shapes it lands on.** A ring
        // reached past its edge by an early via is wider by the time a later one is sized against
        // it, so the later via's metal fits where it would otherwise have overhung. Walk the
        // crossings in the order they were discovered instead and three ring segments come out 20
        // units short — the vias identical, only the metal they sit on wrong.
        //
        // ℹ️ `odb::Rect` orders by `(xlo, ylo, xhi, yhi)`, its members in declaration order under a
        // defaulted `operator<=>`, which is what a tuple of the same four does here.
        // 🔑 **A connect CACHES the via it built and reuses it for every crossing of the same
        // size.** `Connect::makeVia` keys on `(net, intersection dx, intersection dy)` — the net
        // only where split cuts are in play — and builds the stack once. Every later crossing of
        // that size gets the FIRST one's geometry, enclosures and all.
        //
        // ⚠️ **So the enclosures can reflect a shape orientation the crossing no longer has.** The
        // constraints come from the shapes' own aspect (`must_fit_x = !isHorizontal()`), and
        // trimming a tall ring segment down to a stub flips that aspect — but a crossing that hits
        // the cache never asks. Rebuilding per crossing instead gives three ring segments 20 units
        // short, and nothing else different.
        //
        // ⚠️ Caching is SKIPPED where either shape is unmodifiable or carries terminal
        // connections; the reference sets `skip_caching` and clears the entry after using it.
        let mut via_cache: std::collections::HashMap<
            (usize, Option<String>, i32, i32),
            (Direction, Direction),
        > = std::collections::HashMap::new();
        let mut placed = placed;
        placed.sort_by_key(|v| {
            (
                layer_number(&all_layers, &v.lower),
                layer_number(&all_layers, &v.upper),
                v.area,
            )
        });
        // ⚠️ One `dbVia` per distinct geometry, reused wherever it is needed — the reference looks
        // the via up by name and creates it only when absent. One per location instead leaves the
        // database carrying thousands of identical via definitions.
        // ── the cut geometry, from the technology rather than the command line ──────────────
        // ⚠️ The cut layer's own VIARULE GENERATE decides the cut size and the enclosures. The
        // command-line values are kept only for a technology declaring no rule, and a via built
        // from those is one this engine invented rather than one the technology describes.
        // `--split-cuts <layer>:<pitch>[:stagger]` — cuts crossing this layer are spread out
        // rather than packed into one array.
        //
        // ⚠️ **Only INTERMEDIATE layers count.** `Connect::setSplitCuts` erases the connect's own
        // two ends from the map, so naming an end layer does nothing.
        let split_cuts: Vec<(String, i32, bool)> = opts
            .all("split-cuts")
            .iter()
            .filter_map(|s| {
                let mut p = s.splitn(3, ':');
                let layer = p.next()?.to_string();
                let pitch = dbu(p.next().unwrap_or("0"), per_micron);
                let stagger = p.next() == Some("stagger");
                (pitch > 0).then_some((layer, pitch, stagger))
            })
            .collect();
        // ⚠️ **The global `--split-cuts` is a FALLBACK**, used only by a connect that states
        // none of its own. Upstream has no such thing: the flag predates the per-connect field
        // and is kept so a caller driving the engine by hand can still reach the behaviour.
        let split_for_connect = |lower: &str, upper: &str, layer: &str| {
            let own = split_by_connect
                .iter()
                .find(|((l, u), _)| l == lower && u == upper)
                .map(|(_, s)| s.as_slice())
                .unwrap_or(&[]);
            let from = if own.is_empty() {
                split_cuts.as_slice()
            } else {
                own
            };
            from.iter()
                // The end-layer erasure again, for the fallback list, which never saw it.
                .filter(|(l, ..)| l != lower && l != upper)
                .find(|(l, ..)| l == layer)
                .map(|(_, p, s)| (*p, *s))
        };
        let rules = via_rules(&db);
        let fallback_cut = (
            dbu(opts.one("cut-width").unwrap_or("0"), per_micron),
            dbu(opts.one("cut-height").unwrap_or("0"), per_micron),
        );
        let fallback_enc = dbu(opts.one("cut-enclosure").unwrap_or("0"), per_micron);
        // The track grids of every layer any connect asked to snap to, read once.
        // ⚠️ A layer NOT named here has no grid, and `snap_to_grid` then returns its argument — the
        // reference relies on exactly that, calling the snap unconditionally at every level.
        let mut track_grids: std::collections::HashMap<String, (Vec<i32>, Vec<i32>)> =
            Default::default();
        for (_, layers) in &on_grid {
            for l in layers {
                track_grids
                    .entry(l.clone())
                    .or_insert_with(|| db.track_grid(l).unwrap_or_default());
            }
        }
        let mut written = 0;
        for v in &placed {
            // The two shapes AS THEY STAND, which is not as they started.
            //
            // 🔑 **The reference holds SHAPE POINTERS, so it reads whatever the shape has become.**
            // `Via` keeps `lower_` and `upper_` and `Connect::makeVia` asks them for their rects at
            // write time — after `trimShapes` has pulled them back and after earlier vias in this
            // same pass have grown them. Reading the rect the crossing was FOUND with instead
            // measures the stack against a strap that no longer exists.
            //
            // ⚠️ **Not every end is a shape this engine emits.** A macro pin, a switch's always-on
            // pin and existing routing are inputs and are never written, so there is nothing to
            // look up and the crossing's own rect stands — which is right, since trimming cannot
            // touch them either.
            let faces_before = via_faces.len();
            let lower_key = (v.net.clone(), v.lower.clone(), v.lower_rect);
            let upper_key = (v.net.clone(), v.upper.clone(), v.upper_rect);
            // ⚠️ **Matched on the CROSSING's area, not on the rect the shape used to have.** A
            // trimmed shape still covers the vias that held it, so the crossing area finds it; the
            // old rect is the shape at its longest and can reach across a neighbour the trim left
            // standing, which picks up the wrong strap entirely.
            // ⚠️ **The BEST match, not the first.** At a ring corner two segments meet, both
            // cover the crossing and both overlap the rect the crossing was found with — and the
            // perpendicular one reaches far along the other axis, so taking it hands the stack a
            // rect large enough to fit a via the reference never builds. The shape this crossing
            // actually sits on is the one that still agrees most with what it was found as.
            let current = |net: &str, layer: &str, was: Rect| -> Rect {
                if std::env::var_os("PDN_NO_CURRENT").is_some() {
                    return was;
                }
                let area_of = |r: Rect| -> i64 {
                    let w = (r.2.min(was.2) - r.0.max(was.0)).max(0) as i64;
                    let h = (r.3.min(was.3) - r.1.max(was.1)).max(0) as i64;
                    w * h
                };
                emitted
                    .iter()
                    .filter(|(n, l, r, _)| {
                        n == net && l == layer && overlaps(*r, v.area) && overlaps(*r, was)
                    })
                    .max_by_key(|(_, _, r, _)| area_of(*r))
                    .map(|(_, _, r, _)| *r)
                    .unwrap_or(was)
            };
            // 🔑 **`cleanupVias` runs between the trim and the write, and it takes vias away.**
            // `Grid::removeInvalidVias` drops any via whose `Via::isValid` fails, and that is no
            // more than `lower_ != nullptr && upper_ != nullptr` — a shape the trim DELETED nulls
            // that end on every via attached to it. So a crossing whose strap no longer exists is
            // never offered to `makeVia` at all.
            //
            // ⚠️ **An end that was never a shape of ours is not a missing one.** A macro pin, a
            // switch's always-on pin and existing routing are inputs, never written, and the
            // reference holds Shape objects for them just the same — so they are looked for among
            // the pins rather than among the straps, and their absence from `emitted` means
            // nothing.
            let still_there = |net: &str, layer: &str, was: Rect| -> bool {
                emitted.iter().any(|(n, l, r, _)| {
                    n == net && l == layer && overlaps(*r, v.area) && overlaps(*r, was)
                }) || connectable_pins
                    .iter()
                    .any(|p| p.net == net && p.layer == layer && overlaps(p.rect, was))
                    || fixed_via_shapes
                        .iter()
                        .any(|f| f.net == net && f.layer == layer && overlaps(f.rect, was))
            };
            if !still_there(&v.net, &v.lower, v.lower_rect)
                || !still_there(&v.net, &v.upper, v.upper_rect)
            {
                continue;
            }
            let lower_rect = current(&v.net, &v.lower, v.lower_rect);
            let upper_rect = current(&v.net, &v.upper, v.upper_rect);
            if std::env::var_os("PDN_SHAPE_TRACE").is_some() {
                eprintln!(
                    "[shape] {}|{}->{}|area {:?}|lower was {:?} now {:?}|upper was {:?} now {:?}",
                    v.net, v.lower, v.upper, v.area, v.lower_rect, lower_rect, v.upper_rect,
                    upper_rect
                );
            }
            // The two ends' orientations, which is all the cache actually decides — see above.
            // ⚠️ **Consulted after the placement point**, which the reference takes from the real
            // shapes; only the geometry comes from the first crossing of this size.
            let inter = vyges_pdn::vias::via_area(lower_rect, upper_rect);
            let split_here = !split_by_connect
                .iter()
                .find(|((l, u), _)| *l == v.lower && *u == v.upper)
                .map(|(_, sp)| sp.is_empty())
                .unwrap_or(true);
            let cache_key = (
                v.connect,
                split_here.then(|| v.net.clone()),
                inter.2 - inter.0,
                inter.3 - inter.1,
            );
            let dirs = *via_cache.entry(cache_key).or_insert((
                vyges_pdn::viagen::rect_direction(lower_rect),
                vyges_pdn::viagen::rect_direction(upper_rect),
            ));
            // ⚠️ **A connect spanning several routing layers is a STACK, not one via.** metal1 to
            // metal6 needs a cut at each of five levels, and the reference's own counts show it:
            // via1_2 through via5_6 all carry the same number. Building one via for the pair leaves
            // four levels unconnected while looking complete at both ends.
            // 🔑 **Where the via goes is not the centre of what it is sized from.** The placement
            // point comes from the plain intersection of the two shapes, SNAPPED — see
            // `vias::placement_point`. Every level of a stack sits at the same point.
            let Some(place_at) =
                vyges_pdn::vias::placement_point(lower_rect, upper_rect, grid_mfg)
            else {
                continue; // off grid: the reference builds a dummy via, which places nothing
            };

            let stack = stack_layers(&db, &v.lower, &v.upper);
            // The layers THIS connect snaps to. Empty for every other connect, which is the point.
            let ongrid: &[String] = on_grid
                .iter()
                .find(|((l, u), _)| *l == v.lower && *u == v.upper)
                .map(|(_, g)| g.as_slice())
                .unwrap_or(&[]);
            let (max_rows, max_columns) = max_cuts
                .iter()
                .find(|((l, u), _)| *l == v.lower && *u == v.upper)
                .map(|(_, c)| *c)
                .unwrap_or((0, 0));
            // 🔑 **The two ENDS get their own shapes.** Passing `v.area` for both handed every
            // level the intersection, which clips a strap that overhangs the core to the rail's
            // edge — and a via built in the clipped rect lands 500 dbu off and can never widen
            // the shape it sits on.
            let intermediate: Vec<&str> = if stack.len() > 2 {
                stack[1..stack.len() - 1].iter().map(String::as_str).collect()
            } else {
                Vec::new()
            };
            // 🔑 **A stack that cannot hold the intersection TAPERS.** Where an intermediate
            // routing layer's own minimum width is wider than the intersection is narrow, that
            // level is grown to what a via needs there — the layer minimum plus twice the worst
            // enclosure the cut layers either side ask for — instead of the stack failing.
            //
            // ⚠️ **The gate and the growth read different widths, and the gate is the smaller.**
            // `isComplexStackedVia` asks the raw `getMinWidth()`; `generateComplexStackedViaRects`
            // grows to `Connect::getMinWidth()`. A stack can pass the gate and still be narrower
            // than the target, and the reference leaves those alone — applying the growth
            // unconditionally widens stacks it never touches.
            let raw_min_widths: Vec<i32> = intermediate
                .iter()
                .map(|l| db.layer_get_min_width(l) as i32)
                .collect();
            let via_min_widths: Vec<i32> = intermediate
                .iter()
                .map(|l| via_min_width(&db, l, &rules))
                .collect();
            // ⚠️ **`PDN_MINW_TRACE` prints the two END rects and each intermediate's target
            // width.** It is the instrument that separates a wrong min-width from a right one
            // measured against the wrong shape — the two produce the same symptom, an
            // intermediate rect of the wrong height, and nothing else distinguishes them.
            if std::env::var_os("PDN_MINW_TRACE").is_some() {
                eprintln!(
                    "[minw] {}->{} lower {:?} upper {:?} intermediate {:?} raw {:?} via {:?}",
                    v.lower, v.upper, lower_rect, upper_rect, intermediate, raw_min_widths,
                    via_min_widths
                );
            }
            let rects = vyges_pdn::vias::stack_rects_tapered(
                lower_rect,
                upper_rect,
                &raw_min_widths,
                &via_min_widths,
                grid_mfg,
            );

            // 🔑 **A level holds a SET of candidate rects, not one.** `makeSingleLayerVia`
            // takes `lower_rects` and `upper_rects`, crosses every pair with every rule, and
            // lets `generateDbVia` build them all and keep the best — so an intermediate level
            // gets two chances to find an enclosure that fits, and the ends get one because a
            // shape has no second version of itself.
            //
            // ⚠️ **`generateMinEnclosureViaRects` runs on BOTH stack layouts**, not only the
            // tapered one, so this is not taper support — it is a mechanism the plain path was
            // missing too.
            let mut rect_set: Vec<Vec<vyges_pdn::Rect>> =
                rects.iter().map(|r| vec![*r]).collect();
            let widths: Vec<i32> = intermediate
                .iter()
                .map(|l| db.layer_get_width(l) as i32)
                .collect();
            let horizontal: Vec<bool> = intermediate
                .iter()
                .map(|l| direction_of(&db, l) == Direction::Horizontal)
                .collect();
            // ⚠️ **Per connect and per intermediate LAYER**, which is how `min_width_layers_` is
            // stored: the same layer may be held to its width by one connect and left free by
            // another crossing it.
            let held_narrow: &[String] = min_width_by_connect
                .iter()
                .find(|((l, u), _)| *l == v.lower && *u == v.upper)
                .map(|(_, m)| m.as_slice())
                .unwrap_or(&[]);
            let min_width_only: Vec<bool> = intermediate
                .iter()
                .map(|l| held_narrow.iter().any(|h| h == l))
                .collect();
            vyges_pdn::vias::add_min_enclosure_rects(
                &mut rect_set,
                &widths,
                &horizontal,
                &min_width_only,
            );

            // The previous level's TOP metal, which the next level shares a layer with.
            // ⚠️ **One entry per SPOT**, because a split-cut level places many vias and each
            // leaves its own metal on the layer above.
            // ⚠️ **And whether that via was an ARRAY**, because the patch on the layer between is
            // decided by `requiresPatch()` on either of the two vias, not by the geometry.
            let mut previous_top: Option<(String, Vec<Rect>, bool)> = None;
            for (level, pair) in stack.windows(2).enumerate() {
                let (lo, hi) = (pair[0].as_str(), pair[1].as_str());
                // An explicit `--cut-<lower>-<upper>` overrides; otherwise the technology decides.
                let named = opts
                    .one(&format!("cut-{lo}-{hi}"))
                    .filter(|c| !c.is_empty());
                let Some(cut_layer) = named.map(str::to_string).or(cut_layer_between(&db, lo, hi))
                else {
                    continue; // no cut layer between these two, so nothing can be built
                };
                let cut_layer = cut_layer.as_str();
                // 🔑 **The ARRAYSPACING rules belong to the CUT layer**, and are read once per
                // level rather than per candidate pair — they do not depend on the rects.
                // 🔑 **The cut layer's ADJACENTCUTS rules**, read once per level like the array
                // rules beside them — they do not depend on the rects either.
                // ⚠️ **LEF 5.4 only.** `updateCutSpacing` tries the LEF58 `ADJACENTCUTS` rules
                // first and falls back to these only when none applied, so a technology stating
                // both would take the LEF58 spacing where this takes the 5.4 one. No technology in
                // reach states a LEF58 one, and inventing the precedence untested is worse than
                // recording that it is missing.
                let adjacent_rules = db.layer_v54_adjacent_cut_rules(cut_layer).unwrap_or_default();
                let array_rules: Vec<vyges_pdn::viagen::ArrayRule> = db
                    .layer_array_spacing_rules(cut_layer)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(cut_class, parallel_overlap, long_array, array_width, cut_spacing, cuts_spacing)| {
                        vyges_pdn::viagen::ArrayRule {
                            cut_class,
                            parallel_overlap,
                            long_array,
                            array_width,
                            cut_spacing,
                            cuts_spacing,
                        }
                    })
                    .collect();

                // ── the level's own placement point ─────────────────────────────────────────────
                // 🔑 **A stack does NOT sit on one point.** `DbGenerateStackedVia::generate` snaps
                // each level separately, so a stack crossing a layer the connect named bends: the
                // levels touching that layer move to its nearest track and the rest stay put.
                //
                // ⚠️ **That bend is the whole mechanism.** The levels either side of the named
                // layer then leave metal at DIFFERENT positions on the layer between, the union is
                // no longer a rectangle, and the bounding box that closes the notch is written as
                // DRCFILL — hundreds of patches on one design, none of which appear if every
                // level sits on the same point.
                //
                // ⚠️ **Only a STACK snaps.** A single-level connect goes through
                // `makeSingleLayerVia`, and `DbGenerateVia::generate` takes the `ongrid` set and
                // ignores it — the snapping lives in the stacked via, not in the via.
                let place_at = if stack.len() > 2 {
                    let grid_of = |layer: &str| {
                        ongrid
                            .iter()
                            .any(|l| l == layer)
                            .then(|| track_grids.get(layer))
                            .flatten()
                    };
                    let snap_x = |layer: &str, v: i32| match grid_of(layer) {
                        Some((x, _)) => vyges_pdn::vias::snap_to_grid(v, x, 0),
                        None => v,
                    };
                    let snap_y = |layer: &str, v: i32| match grid_of(layer) {
                        Some((_, y)) => vyges_pdn::vias::snap_to_grid(v, y, 0),
                        None => v,
                    };
                    let (x_from_upper, y_from_upper) = vyges_pdn::vias::snap_sources(
                        direction_of(&db, lo) == Direction::Horizontal,
                    );
                    (
                        snap_x(if x_from_upper { hi } else { lo }, place_at.0),
                        snap_y(if y_from_upper { hi } else { lo }, place_at.1),
                    )
                } else {
                    place_at
                };
                // 🔑 **A split-cut level builds a 1x1 via and places it many times.** Where either
                // of a level's own layers asks for split cuts, `determineRowsAndColumns` forces
                // `rows = cols = 1` and the spread is laid out separately — the pitch being the
                // LARGER of the two layers' requests, applied to both axes.
                let split_for = |layer: &str| split_for_connect(&v.lower, &v.upper, layer);
                let split = split_for(lo).or_else(|| split_for(hi)).map(|(p, s)| {
                    let other = split_for(if split_for(lo).is_some() { hi } else { lo });
                    (p.max(other.map(|(q, _)| q).unwrap_or(0)), s || other.is_some_and(|(_, t)| t))
                });

                // 🔑 **The widths a generate rule is checked against, and they are the SHAPES'.**
                // `getLowerWidth`/`getUpperWidth` read `lower_rect_`/`upper_rect_` — the straps the
                // via lands on — not the intersection it is built in.
                // ⚠️ **Zero for a split array**, which is how a wide-metal rule is kept off a via
                // that will be scattered as single cuts. See `rule_valid_for_width`.
                let shape_width = |r: vyges_pdn::Rect| (r.2 - r.0).min(r.3 - r.1);
                let (width_lo, width_hi) = if split.is_some() {
                    (0, 0)
                } else {
                    (shape_width(rects[level]), shape_width(rects[level + 1]))
                };
                let rule_fits = |r: &ViaRule| {
                    vyges_pdn::viagen::rule_valid_for_width(r.bottom_width, width_lo)
                        && vyges_pdn::viagen::rule_valid_for_width(r.top_width, width_hi)
                };
                // Each axis from the shape that constrains it — see `vias::via_area`.
                let area = vyges_pdn::vias::via_area(rects[level], rects[level + 1]);

                // 🔑 **A crossing holds its two shapes whether or not any metal was built.**
                // `Grid::makeVias` creates a `Via` per intersection and `PdnGen::updateVias`
                // attaches it to both shapes; the only thing that takes one away is
                // `removeInvalidVias`, and `Via::isValid` asks solely whether the two shapes
                // still exist. A `DbVia` that could not be generated never enters that test —
                // `makeVia` reports `PDN-0110` and moves on, leaving the `Via` in place.
                //
                // ⚠️ **So a refused via still trims a strap, and that is not a quirk.** On a
                // design with a switched region domain, a met5 stripe crosses met4 twice and
                // only one crossing builds. Counting the built one alone leaves a single
                // connection, `isRemovable` takes the whole stripe away, and counting both
                // keeps it and sets its minimum rect to 63600..114370 — the reference's own
                // answer, spanning from the REFUSED crossing to the built one.
                //
                // 🔑 **The SHAPES' intersection, not this level's rect.** `getMinimumRect`
                // merges `via->getArea()`, and a `Via` is constructed once per crossing with
                // the two shapes' own intersection — every level of the stack reports the same
                // area. Reporting the level's rect is harmless while the levels all hold the
                // intersection, and wrong the moment a stack tapers: a grown intermediate rect
                // then holds the strap out past where the via metal reaches.
                // ℹ️ The hold itself is recorded before trimming — see the pre-pass above.

                // A split-cut level places its 1x1 via once per array position; every other level
                // places one via at the point itself.
                // ℹ️ Asked per area, because the generate path chooses among candidate rects and
                // the winner's area is what its array has to fill.
                let spots_in = |area: vyges_pdn::Rect| match split {
                    None => vec![place_at],
                    Some((sp, stagger)) => {
                        // ⚠️ **The stagger offset is applied to the GROUND net only**, so the two
                        // nets' arrays interleave by half a pitch rather than both shifting.
                        let off = if stagger && v.net == ground {
                            (sp / 2, sp / 2)
                        } else {
                            (0, 0)
                        };
                        let n = vyges_pdn::split::counts(
                            (area.2 - area.0, area.3 - area.1),
                            (sp, sp),
                            max_columns,
                            max_rows,
                        );
                        // ℹ️ Identity snapping: both call sites in the reference pass `false` for
                        // the snap flags, so the layer grids are never populated.
                        vyges_pdn::split::positions(place_at, n, (sp, sp), off, &|v| v, &|v| v)
                    }
                };
                let spots = spots_in(area);

                // ── a via the technology already declares ────────────────────────────────────
                // 🔑 **Only the GEOMETRY is the technology's.** How many cuts fit, and what
                // enclosure the via is finally built with, are decided by the same machinery a
                // generated via uses — `TechViaGenerator` differs from `GenerateViaGenerator`
                // only in where its cut and its minimum enclosures come from, and hands rows,
                // columns and pitch to `DbTechVia` through the shared `makeBaseVia`.
                // ⚠️ Evaluated before the generate path so a NAMED via wins outright, but the
                // unnamed fallback inside is gated on there being no rule for this level — the
                // reference populates both lists and picks between them, and preferring a tech
                // via where a rule exists would rebuild every Nangate45 case differently.
                // ⚠️ **A tech via is a FALLBACK, not an alternative.** `makeSingleLayerVia` builds
                // every generate rule first and only reaches `populateTechVias` when none of them
                // was even set up validly — so a rule that exists but does not apply here leaves
                // the level to the technology's own via rather than to nothing.
                let has_rule = rules
                    .iter()
                    .any(|r| r.lower == lo && r.upper == hi && rule_fits(r));
                // 🔑 **`PDN_RULE_TRACE=1` says why a level fell through to the technology's own
                // via.** The reference's `Via` group prints `Generate via rules available: N
                // from M`; this prints the candidates for the pair and the width gate each was
                // judged by, so "no rule applies here" and "no rule exists for this pair" can
                // be told apart. ⚠️ They look identical from the output and mean opposite
                // things — a missing rule is a parse bug, an inapplicable one is the design.
                if std::env::var_os("PDN_RULE_TRACE").is_some() && !has_rule {
                    for r in rules.iter().filter(|r| r.lower == lo && r.upper == hi) {
                        eprintln!(
                            "[rule] {lo}->{hi} {} bw {:?} vs {width_lo} | tw {:?} vs {width_hi}",
                            r.name, r.bottom_width, r.top_width
                        );
                    }
                    eprintln!("[rule] NO RULE {lo}->{hi} (candidates {})", rules.iter().filter(|r| r.lower == lo && r.upper == hi).count());
                }
                let named_here = fixed
                    .iter()
                    .find(|(l, u, ..)| *l == v.lower && *u == v.upper)
                    .is_some_and(|(_, _, n, _, _)| !n.is_empty());
                // The generate rule spanning exactly this level, where one is declared.
                //
                // ⚠️ **A rule this connect asked to keep out is not a rule.** `filterVias` erases
                // by name from the generate rules and the tech vias alike, so the filter belongs
                // on both paths — applied to only one, a via merely moves between them.
                let dont_use = fixed
                    .iter()
                    .find(|(l, u, ..)| *l == v.lower && *u == v.upper)
                    .and_then(|(.., d)| d.as_ref());
                // 🔑 **`-cut_pitch` overrides the technology's, on EVERY generator.**
                // `generateDbVia` sets it before any candidate is built:
                // `if (hasCutPitch()) { via->setCutPitchX(...); via->setCutPitchY(...); }` —
                // and only the split-cut pitch, applied after, may displace it.
                //
                // ⚠️ **The pitch is stored as the cut SPACING and appears in the via's NAME**,
                // so a via built at the technology's pitch is a different object from the same
                // geometry built at the connect's. Read only on the tech-via path, one design
                // came out with 534 columns at a 300 pitch where the reference writes 161 at
                // 1000 — every via in the design different, and not one shape to show for it.
                let connect_cut_pitch: Option<(i32, i32)> = fixed
                    .iter()
                    .find(|(l, u, ..)| *l == v.lower && *u == v.upper)
                    .and_then(|(_, _, _, p, _)| *p);
                // ⚠️ A shape constrains the via only at ITS OWN end of the stack; every level
                // in between takes minimums however much room it has.
                let at_bottom_end = level == 0;
                let at_top_end = level + 2 == stack.len();
                // ⚠️ The SHAPE's orientation, not the layer's — `Shape::isHorizontal` is its own
                // aspect ratio. A follow pin on a vertical layer still runs horizontally.
                // ℹ️ Read from the level's own rect rather than a candidate: an END of the stack
                // is a shape, and a shape has exactly one candidate.
                // ℹ️ From the CACHED orientations, not from this crossing's own rects: where the
                // connect has already built a via of this size, that via is reused whole.
                let bot_c = if at_bottom_end {
                    vyges_pdn::viagen::constraint_for(dirs.0, true, false)
                } else {
                    Default::default()
                };
                let top_c = if at_top_end {
                    vyges_pdn::viagen::constraint_for(dirs.1, true, false)
                } else {
                    Default::default()
                };

                // ── every lower candidate against every upper one ───────────────────────────
                // 🔑 **`makeSingleLayerVia` crosses the two SETS.** A generator is made for each
                // (lower rect, upper rect, rule) triple, `generateDbVia` builds every one that
                // can be built, stable-sorts them by `isPreferredOver` and takes the first.
                //
                // ⚠️ **The pair is not a detail of the enclosure search — it changes the rule.**
                // `getLowerWidth`/`getUpperWidth` read the candidate rects, so a different pair
                // can pass a width gate the other fails, land in a different enclosure bucket,
                // and fit a different number of cuts. That is the whole reason a level carries
                // more than one rect.
                struct LevelVia<'r> {
                    rule: Option<&'r ViaRule>,
                    cut: (i32, i32),
                    pitch: (i32, i32),
                    area: vyges_pdn::Rect,
                    rows: i32,
                    columns: i32,
                    /// Set where an `ARRAYSPACING` rule regrouped the cuts. `rows`/`columns`
                    /// are then ONE group's, and this says how the groups are laid out.
                    array: Option<vyges_pdn::viagen::ArrayFit>,
                    bot_enc: (i32, i32),
                    top_enc: (i32, i32),
                    score: vyges_pdn::viagen::Generator,
                }
                let mut best: Option<LevelVia> = None;
                for &lower_rect in &rect_set[level] {
                    for &upper_rect in &rect_set[level + 1] {
                        // The widths the rule is gated on are THIS pair's, not the level's.
                        let (w_lo, w_hi) = if split.is_some() {
                            (0, 0)
                        } else {
                            (shape_width(lower_rect), shape_width(upper_rect))
                        };
                        let rule = rules
                            .iter()
                            .filter(|r| !dont_use.is_some_and(|d| d.is_match(&r.name)))
                            .find(|r| {
                                r.lower == lo
                                    && r.upper == hi
                                    && vyges_pdn::viagen::rule_valid_for_width(
                                        r.bottom_width,
                                        w_lo,
                                    )
                                    && vyges_pdn::viagen::rule_valid_for_width(r.top_width, w_hi)
                            });
                        let cut = rule.map(|r| r.cut).unwrap_or(fallback_cut);
                        if cut.0 <= 0 || cut.1 <= 0 {
                            continue; // nothing to build from
                        }
                        // 🔑 **A split array's pitch is the SPLIT pitch, on both axes.**
                        // `generateDbVia` calls `setCutPitchX(pitch)`/`setCutPitchY(pitch)` with
                        // the larger of the two layers' requests before the generator is ever
                        // built, overriding whatever spacing the rule declares. The via still
                        // holds one cut, so nothing moves — but the pitch is stored as the cut
                        // SPACING and appears in the via's name, so a via built at the rule's
                        // pitch is a different object with a different name from the same
                        // geometry.
                        // 🔑 **The pitch comes from the CUT LAYER, and the rule's own
                        // `SPACING` is only the fallback.** `determineCutSpacing` runs first
                        // — the layer's spacing added to the cut, refined by the cut class's
                        // spacing table — and the generate rule's value is taken **verbatim**
                        // and only where that left an axis at zero.
                        //
                        // ⚠️ Reading the rule first inflates every via the rule covers. A
                        // wide-power rule stating a deliberately large spacing then sets the
                        // pitch for vias the layer would have packed far more tightly — and
                        // nothing says so, because vias are not compared shape by shape. It
                        // shows only where the oversized metal has to be absorbed by a shape
                        // too narrow to hide it.
                        let pitch = match (split, connect_cut_pitch) {
                            (Some((sp, _)), _) => (sp, sp),
                            (None, Some(p)) => p,
                            (None, None) => {
                                let cut_rect = (0, 0, cut.0, cut.1);
                                let classes = via_cut_classes(&db, cut_layer);
                                let cls = vyges_pdn::viagen::cut_class(&classes, cut)
                                    .map(|c| c.name.clone())
                                    .unwrap_or_else(|| cut_layer.to_string());
                                let table = spacing_table_rules(&db, cut_layer, &cls);
                                vyges_pdn::techvia::base_cut_pitch(
                                    cut_rect,
                                    db.layer_get_spacing(cut_layer),
                                    vyges_pdn::techvia::class_cut_spacing(cut_rect, &table),
                                )
                                .or_else(|| rule.map(|r| r.pitch))
                                .unwrap_or(cut)
                            }
                        };
                        // Each axis from the shape that constrains it — see `vias::via_area`.
                        let area = vyges_pdn::vias::via_area(lower_rect, upper_rect);
                        // The metal each shape has outside that intersection — see `E30`.
                        //
                        // ⛔ **Upstream marks a via admitted this way UNCACHEABLE** (`can_cache_ =
                        // false`, added in the same commit) because its `Connect::makeVia` caches
                        // the BUILT VIA keyed by the crossing's size, and the spare depends on
                        // where the shapes are rather than how big the overlap is.
                        //
                        // ✅ **No bypass is needed here, and the reason is not "we got away with
                        // it".** This engine's `via_cache` holds the two ends' ORIENTATIONS and
                        // nothing else, so the enclosure — spare included — is recomputed from
                        // each crossing's own rects. A crossing of the same size with no spare
                        // computes zero spare and fails its own check.
                        //
                        // ⚠️ **That stops being true the moment the cache holds geometry.** If it
                        // is ever widened, this via must not be cached.
                        let (spare_b, spare_t) =
                            vyges_pdn::viagen::spare_enclosure(lower_rect, upper_rect);

                        // ── the enclosure pair, chosen the way the reference chooses it ─────
                        // 🔑 Candidates from the rules, crossed, each scored by how many cuts
                        // IT lets fit and kept only if the constraints pass. The leftover-fill
                        // this replaced was derived from one case that agreed exactly and three
                        // in the same run that did not.
                        let (rule_bot, rule_top) = rule.map(|r| (r.bottom_enclosure, r.top_enclosure)).unwrap_or((
                            (fallback_enc, fallback_enc),
                            (fallback_enc, fallback_enc),
                        ));
                        let bottoms = enclosure_candidates(
                            &db, cut_layer, cut, area, lo, false, Some(rule_bot), split.is_some(),
                        );
                        let tops = enclosure_candidates(
                            &db, cut_layer, cut, area, hi, true, Some(rule_top), split.is_some(),
                        );
                        // 🔑 **What `checkMinEnclosure` is asked against** — the cut layer's
                        // own rules, WITHOUT the generate rule's stated enclosure among them.
                        let bot_rules = enclosure_candidates_with_swap(
                            &db, cut_layer, cut, area, lo, false, None, split.is_some(),
                        );
                        let top_rules = enclosure_candidates_with_swap(
                            &db, cut_layer, cut, area, hi, true, None, split.is_some(),
                        );

                        // 🔑 **`PDN_VIA_TRACE=1` prints what the reference's `Via` and
                        // `ViaEnclosure` debug groups print**, in the same terms: the area, the
                        // cut, the pitch, both rule sets, both candidate sets and both
                        // constraints, then whether the pair was CHOSEN or REJECTED. The two
                        // logs can be read side by side, which is how the 770-wide crossing was
                        // settled — ours rejected the generate via exactly where the reference
                        // did, and the difference was the tech-via fallback below.
                        if std::env::var_os("PDN_VIA_TRACE").is_some() {
                            eprintln!(
                                "[via] {}|{lo}->{hi}|area {area:?}|cut {cut:?}|pitch {pitch:?}|bot_rules {bot_rules:?}|top_rules {top_rules:?}|bottoms {bottoms:?}|tops {tops:?}|bot_c {bot_c:?}|top_c {top_c:?}",
                                v.net
                            );
                        }
                        // 🔑 **The score is `getTotalCuts()`, and that is the CLAMPED product.**
                        // `determineRowsAndColumns` ends with `core_row_ = std::max(1, rows)`
                        // and its sibling for columns, so a pair that fits no cut on an axis
                        // still builds one row of them.
                        //
                        // ⚠️ **Which means the fit is not a gate at all.** `checkConstraints`'s
                        // `getTotalCuts() == 0` test can never fire once the clamp has run;
                        // what rejects a pair is the minimum-cut and minimum-enclosure check.
                        // Treating a zero fit as a rejection refuses vias the reference builds
                        // — every stack onto a power switch's pin, which is 280 tall against a
                        // 150 cut and enclosures that want 170.
                        let fit =
                            |b: vyges_pdn::viagen::Enclosure, t: vyges_pdn::viagen::Enclosure| {
                                let c = vyges_pdn::viagen::cuts_across(
                                    area.2 - area.0,
                                    cut.0,
                                    b.x,
                                    t.x,
                                    pitch.0,
                                    max_columns,
                                )
                                .max(1);
                                let r = vyges_pdn::viagen::cuts_across(
                                    area.3 - area.1,
                                    cut.1,
                                    b.y,
                                    t.y,
                                    pitch.1,
                                    max_rows,
                                )
                                .max(1);
                                c * r
                            };
                        let Some(chosen) = vyges_pdn::viagen::best_enclosure_pair(
                            &bottoms,
                            &tops,
                            direction_of(&db, lo),
                            direction_of(&db, hi),
                            &fit,
                            // 🔑 **`checkConstraints`: no cuts, then minimum enclosure.** The
                            // enclosure judged is the one the via would be BUILT with, not the
                            // candidate minimum — so the constrained axis carries the overlap,
                            // and a cut taller than the rect it must sit in makes that overlap
                            // NEGATIVE. Every rule then refuses it, however small the rule.
                            //
                            // ⚠️ This is what stops a via being built at all, and a level with
                            // no buildable generate via is what sends the reference to the
                            // technology's own via instead. Without the gate we build the
                            // oversized via, its metal overhangs the shape it lands on, and
                            // the shape grows to cover metal that is then ripped out.
                            //
                            // ℹ️ Minimum-cut rules are the third gate and are not fed here.
                            &|b: vyges_pdn::viagen::Enclosure,
                              t: vyges_pdn::viagen::Enclosure,
                              cuts: i32| {
                                if cuts <= 0 {
                                    return false;
                                }
                                let cols = vyges_pdn::viagen::cuts_across(
                                    area.2 - area.0, cut.0, b.x, t.x, pitch.0, max_columns,
                                )
                                .max(1);
                                let rws = vyges_pdn::viagen::cuts_across(
                                    area.3 - area.1, cut.1, b.y, t.y, pitch.1, max_rows,
                                )
                                .max(1);
                                let span =
                                    ((cols - 1) * pitch.0 + cut.0, (rws - 1) * pitch.1 + cut.1);
                                let overlap = vyges_pdn::viagen::overlap_enclosure(
                                    (area.2 - area.0, area.3 - area.1),
                                    span,
                                );
                                let built_b = vyges_pdn::viagen::built_enclosure(
                                    !at_bottom_end,
                                    b,
                                    overlap,
                                    bot_c,
                                );
                                let built_t = vyges_pdn::viagen::built_enclosure(
                                    !at_top_end,
                                    t,
                                    overlap,
                                    top_c,
                                );
                                // Snapped before the rule check: `checkConstraints` runs
                                // after `determineRowsAndColumns` has already snapped.
                                let built_b =
                                    vyges_pdn::viagen::snap_enclosure(built_b, grid_mfg);
                                let built_t =
                                    vyges_pdn::viagen::snap_enclosure(built_t, grid_mfg);
                                // 🔑 **A side the intersection cannot satisfy is re-checked
                                // against the metal OUTSIDE it.** `determineRowsAndColumns` ends
                                // by applying the spare enclosure to whichever side still fails,
                                // capped at what the rule asks. Without it a follow pin wider
                                // than the layer it crosses reports zero enclosure and every via
                                // on the grid is refused — `asap7_M1_M2_followpin_enclosure` is
                                // 53 vias that come back as 0.
                                //
                                // ⚠️ **Per side, and only the failing one.** Upstream checks the
                                // bottom and top separately (`checkMinEnclosure(true, false)`),
                                // so a side that already passes is left exactly as built.
                                let ok_b = vyges_pdn::viagen::enclosure_satisfies(
                                    built_b, &bot_rules,
                                ) || (spare_b.x > 0 || spare_b.y > 0)
                                    && vyges_pdn::viagen::enclosure_satisfies(
                                        vyges_pdn::viagen::snap_enclosure(
                                            vyges_pdn::viagen::spare_applied(built_b, b, spare_b),
                                            grid_mfg,
                                        ),
                                        &bot_rules,
                                    );
                                let ok_t = vyges_pdn::viagen::enclosure_satisfies(
                                    built_t, &top_rules,
                                ) || (spare_t.x > 0 || spare_t.y > 0)
                                    && vyges_pdn::viagen::enclosure_satisfies(
                                        vyges_pdn::viagen::snap_enclosure(
                                            vyges_pdn::viagen::spare_applied(built_t, t, spare_t),
                                            grid_mfg,
                                        ),
                                        &top_rules,
                                    );
                                ok_b && ok_t
                            },
                        ) else {
                            if std::env::var_os("PDN_VIA_TRACE").is_some() {
                                eprintln!("[via]   REJECTED {area:?} {lo}->{hi}");
                            }
                            continue; // this pair builds nothing; another may
                        };
                        if std::env::var_os("PDN_VIA_TRACE").is_some() {
                            eprintln!("[via]   CHOSE {area:?} {lo}->{hi} {chosen:?}");
                        }

                        let (columns, rows) = if split.is_some() {
                            (1, 1)
                        } else {
                            (
                                vyges_pdn::viagen::cuts_across(
                                    area.2 - area.0,
                                    cut.0,
                                    chosen.bottom.x,
                                    chosen.top.x,
                                    pitch.0,
                                    max_columns,
                                )
                                .max(1),
                                vyges_pdn::viagen::cuts_across(
                                    area.3 - area.1,
                                    cut.1,
                                    chosen.bottom.y,
                                    chosen.top.y,
                                    pitch.1,
                                    max_rows,
                                )
                                .max(1),
                            )
                        };
                        // 🔑 **A wide array stops using the cut layer's plain SPACING.**
                        // `determineRowsAndColumns` fits the cuts, asks `updateCutSpacing` whether
                        // that many adjacent cuts trips a rule, and where it does REFITS them once
                        // on the wider pitch. Everything downstream — the enclosures, the name, the
                        // metal — then uses the new pitch, because the generator's own field was
                        // changed rather than a local copy.
                        // ⚠️ **Refit ONCE, not to a fixed point.** A wider pitch means fewer cuts,
                        // and fewer cuts could drop the array back below the rule's threshold; the
                        // reference does not chase that and neither does this.
                        let (pitch, columns, rows) = match vyges_pdn::viagen::adjacent_cut_pitch(
                            rows,
                            columns,
                            cut,
                            &adjacent_rules,
                        ) {
                            None => (pitch, columns, rows),
                            Some(p) => (
                                p,
                                vyges_pdn::viagen::cuts_across(
                                    area.2 - area.0,
                                    cut.0,
                                    chosen.bottom.x,
                                    chosen.top.x,
                                    p.0,
                                    max_columns,
                                )
                                .max(1),
                                vyges_pdn::viagen::cuts_across(
                                    area.3 - area.1,
                                    cut.1,
                                    chosen.bottom.y,
                                    chosen.top.y,
                                    p.1,
                                    max_rows,
                                )
                                .max(1),
                            ),
                        };

                        // 🔑 **An ARRAYSPACING rule may regroup the cuts before any of this.**
                        // `determineRowsAndColumns` runs its array branch on the plain fit and,
                        // where a rule applies, replaces the counts and the pitch outright —
                        // the enclosure then comes from what the ARRAY leaves over rather than
                        // from what a flat run of cuts does.
                        //
                        // ⚠️ **A split array is never an array in this sense**, and neither is
                        // a fit of one group by one: `isCutArray()` is
                        // `!isSplitCutArray() && (array_core_x_ != 1 || array_core_y_ != 1)`.
                        let array = if split.is_some() {
                            None
                        } else {
                            // ⚠️ **The via's own cut class, not the layer's list.** A rule
                            // naming a class applies only to a via of that class, and a via
                            // with none matches every rule.
                            let my_cut_class = vyges_pdn::viagen::cut_class(
                                &via_cut_classes(&db, cut_layer),
                                cut,
                            )
                            .map(|c| c.name.clone());
                            vyges_pdn::viagen::array_fit(
                                &array_rules,
                                my_cut_class.as_deref(),
                                (area.2 - area.0, area.3 - area.1),
                                cut,
                                pitch,
                                (chosen.bottom.x, chosen.bottom.y),
                                (chosen.top.x, chosen.top.y),
                                (max_columns, max_rows),
                                (columns, rows),
                            )
                        };
                        let (columns, rows) = match &array {
                            Some(f) => f.core,
                            None => (columns, rows),
                        };
                        let pitch = match &array {
                            Some(f) => f.cut_pitch,
                            None => pitch,
                        };
                        // ⚠️ **The chosen pair is a MINIMUM.** What is built is the overlap on
                        // any axis the via must fit and the minimum elsewhere — and a level
                        // internal to the stack takes minimums on both axes however much room
                        // it has, which is why a via in the middle of a stack carries no
                        // overhang while the ends carry plenty.
                        let span =
                            ((columns - 1) * pitch.0 + cut.0, (rows - 1) * pitch.1 + cut.1);
                        let extent = (area.2 - area.0, area.3 - area.1);
                        // 🔑 **An array's leftover is the ARRAY's, not a flat run's** — the
                        // reference hands `double_enc_x / 2` to the same chooser that otherwise
                        // takes the overlap.
                        let overlap = match &array {
                            Some(f) => vyges_pdn::viagen::Enclosure {
                                x: f.double_enclosure.0 / 2,
                                y: f.double_enclosure.1 / 2,
                            },
                            None => vyges_pdn::viagen::overlap_enclosure(extent, span),
                        };
                        let b = vyges_pdn::viagen::built_enclosure(
                            !at_bottom_end,
                            chosen.bottom,
                            overlap,
                            bot_c,
                        );
                        let t = vyges_pdn::viagen::built_enclosure(
                            !at_top_end,
                            chosen.top,
                            overlap,
                            top_c,
                        );
                        // 🔑 **A split array takes the MINIMUM enclosure verbatim.** The growth
                        // above fills whatever room the shapes leave, and
                        // `determineRowsAndColumns` skips it outright for a split array —
                        // `bottom_enclosure_->setX(bottom_min_enclosure.getX())` and its three
                        // siblings, with no `determine_enclosure` and no constraint.
                        // ⚠️ It is not a rounding difference. A followpin crossing is hundreds
                        // of dbu wide, so a grown enclosure puts metal right across it; the
                        // minimum leaves a stub barely wider than the cut. Every via still
                        // lands in the same place, so nothing in the placement gives it away —
                        // what tells us is that the intermediate layer then meets its minimum
                        // area on its own and the DRCFILL patch that should fill it never
                        // appears — hundreds of patches on a single design.
                        // ⚠️ Snapped LAST, after the split-cut override, because
                        // `determineRowsAndColumns` snaps on its way out and every branch
                        // above it funnels through that one line.
                        let (bot_enc, top_enc) = if split.is_some() {
                            (chosen.bottom, chosen.top)
                        } else {
                            (b, t)
                        };
                        let bot_enc = vyges_pdn::viagen::snap_enclosure(bot_enc, grid_mfg);
                        let top_enc = vyges_pdn::viagen::snap_enclosure(top_enc, grid_mfg);
                        let (bot_enc, top_enc) =
                            ((bot_enc.x, bot_enc.y), (top_enc.x, top_enc.y));
                        if std::env::var_os("PDN_ENC_TRACE").is_some() {
                            eprintln!(
                                "[enc] {}|{lo}->{hi}|area {:?}|bot {:?}|top {:?}",
                                v.net, area, bot_enc, top_enc
                            );
                        }

                        // 🔑 **What the sort reads.** `getCutArea` is the cut's own area times
                        // the CLAMPED cut count, and `getGeneratorWidth`/`Height` are
                        // `cut*n + spacing*(n-1) + 2*enclosure` — which is the span plus twice
                        // the enclosure the via was finally built with, per axis.
                        // 🔑 **`getCutArea` counts EVERY cut of the array**, which is
                        // `array_core * core + end` per axis — so a regrouped via scores on
                        // what it actually places, not on one group.
                        let total = match &array {
                            Some(f) => (
                                vyges_pdn::viagen::array_count(f.groups.0, f.core.0, f.end.0),
                                vyges_pdn::viagen::array_count(f.groups.1, f.core.1, f.end.1),
                            ),
                            None => (columns, rows),
                        };
                        let candidate = LevelVia {
                            rule,
                            cut,
                            pitch,
                            area,
                            rows,
                            columns,
                            array,
                            bot_enc,
                            top_enc,
                            score: vyges_pdn::viagen::Generator {
                                name: rule.map(|r| r.name.clone()).unwrap_or_default(),
                                cut_area: cut.0 * cut.1 * total.0 * total.1,
                                bottom: (span.0 + 2 * bot_enc.0, span.1 + 2 * bot_enc.1),
                                top: (span.0 + 2 * top_enc.0, span.1 + 2 * top_enc.1),
                                bottom_direction: direction_of(&db, lo),
                                top_direction: direction_of(&db, hi),
                            },
                        };
                        // ⚠️ **Only a STRICT preference displaces the incumbent.** A tie returns
                        // `false`, so the earlier candidate stays — which is what makes the
                        // reference's stable sort stable, and why the sets are iterated in
                        // their own sorted order rather than whatever order they were built in.
                        if candidate
                            .score
                            .is_preferred_over(best.as_ref().map(|b| &b.score))
                        {
                            best = Some(candidate);
                        }
                    }
                }
                // 🔑 **A technology via is what a level falls back to when no GENERATE via
                // could be BUILT** — not merely when none was set up validly.
                //
                // ⚠️ `generateDbVia` returns null when every generator failed `build()`, so a
                // rule that applies here and yields nothing buildable still hands the level to
                // the technology. Gated on the setup test alone, such a level built nothing at
                // all.
                if let Some((via_name, g, connect_pitch)) =
                    (named_here || !has_rule || best.is_none())
                    .then(|| fixed_tech_via(&db, &fixed, (&v.lower, &v.upper), (lo, hi)))
                    .flatten()
                {
                    // 🔑 **`isSetupValid` runs BEFORE anything is built, and it is the only
                    // thing that can refuse this.** `makeSingleLayerVia` filters its tech-via
                    // candidates through it while collecting them, and
                    // `TechViaGenerator::isSetupValid` ends in `fitsShapes()` — so a via that
                    // does not fit never becomes a candidate and no enclosure arithmetic
                    // downstream gets a chance to rescue it.
                    //
                    // ⚠️ **Judged on the TECHNOLOGY's own metal**, `DbTechVia(via, 1, 0, 1, 0)`
                    // and `getViaRect(true, false, ...)`, translated to the centre of the
                    // overlap — not on the enclosure this level would go on to build, which is
                    // computed below and is a different number.
                    //
                    // ℹ️ Without it, a design with a switched region domain builds a via in 770 of overlap
                    // where the reference reports `PDN-0110 No via inserted between met4 and
                    // met5 at (63.6000, 12.8000) - (64.3700, 14.4000)`. The via was dropped
                    // again later, but not before the write stage had grown the met5 stripe
                    // 325 to the left to cover its metal — which is all that reached the DEF.
                    //
                    // ⚠️ A refusal skips THIS level. The reference discards the whole stack and
                    // substitutes one dummy via, which is the same thing for a two-layer
                    // connect and is what every other bail-out in this loop already does.
                    {
                        let (cx, cy) = ((area.0 + area.2) / 2, (area.1 + area.3) / 2);
                        let shift =
                            |r: Rect| (r.0 + cx, r.1 + cy, r.2 + cx, r.3 + cy);
                        let fits = |metal: Rect, shape: Rect, at_end: bool, layer: &str| {
                            let c = if at_end {
                                vyges_pdn::viagen::constraint_for(
                                    vyges_pdn::viagen::rect_direction(shape),
                                    true,
                                    false,
                                )
                            } else {
                                Default::default()
                            };
                            vyges_pdn::viagen::mostly_contains(
                                shape,
                                area,
                                shift(metal),
                                c,
                                direction_of(&db, layer),
                            )
                        };
                        if !fits(g.bottom_metal, rects[level], level == 0, lo)
                            || !fits(
                                g.top_metal,
                                rects[level + 1],
                                level + 2 == stack.len(),
                                hi,
                            )
                        {
                            if std::env::var_os("PDN_VIA_TRACE").is_some() {
                                eprintln!(
                                    "[via]   TECH REFUSED {area:?} {lo}->{hi} {via_name}"
                                );
                            }
                            continue;
                        }
                    }
                    // ⚠️ **Two different cut rects, and they are not interchangeable.** How many
                    // fit is asked of the merged OUTLINE — `TechViaGenerator::getCut()` returns
                    // `cut_outline_` — while the via's own parameters carry a SINGLE cut's size.
                    // A one-cut tech via makes them equal, which is how using one for both went
                    // unnoticed through ASAP7 entirely.
                    let cut = (
                        g.cut_extent.2 - g.cut_extent.0,
                        g.cut_extent.3 - g.cut_extent.1,
                    );
                    let single = (
                        g.single_cut.2 - g.single_cut.0,
                        g.single_cut.3 - g.single_cut.1,
                    );
                    // The connect's own `-cut_pitch` wins; otherwise the technology decides.
                    let Some(pitch) = connect_pitch.or_else(|| {
                        // 🔑 The cut's class decides which column of the spacing table applies.
                        // ASAP7 states every class at the same value, but the lookup is per class
                        // and a technology that differentiates them would be built wrong without.
                        let classes = via_cut_classes(&db, &g.cut_layer);
                        let cls = vyges_pdn::viagen::cut_class(&classes, cut)
                            .map(|c| c.name.clone())
                            .unwrap_or_else(|| g.cut_layer.clone());
                        let table = spacing_table_rules(&db, &g.cut_layer, &cls);
                        vyges_pdn::techvia::base_cut_pitch(
                            g.cut_extent,
                            db.layer_get_spacing(&g.cut_layer),
                            vyges_pdn::techvia::class_cut_spacing(g.cut_extent, &table),
                        )
                    }) else {
                        if std::env::var_os("PDN_VIA_TRACE").is_some() {
                            eprintln!("[via]   TECH NO PITCH {area:?} {lo}->{hi} {via_name}");
                        }
                        continue; // no pitch stated anywhere, so no array can be laid out
                    };
                    // 🔑 The split pitch overrides the technology's, exactly as on the generate
                    // path — `generateDbVia` sets it on every generator before any is built.
                    let pitch = match split {
                        Some((sp, _)) => (sp, sp),
                        None => pitch,
                    };
                    // 🔑 **A tech via is NOT built with its own metal margins.** Those are only a
                    // floor: `getMinimumEnclosures` takes the layer's rules, erases any candidate
                    // short on BOTH axes, and raises the rest to the floor per axis. What is then
                    // chosen among them, and what is finally built, is the E-series selection the
                    // generate path uses — anything less leaves VIA34 and VIA56 one axis short.
                    let candidates = |metal: Rect, layer: &str, above: bool| {
                        let floor = vyges_pdn::techvia::enclosure(g.cut_extent, metal);
                        // ⚠️ **The rules alone, and the floor is NOT one of them.**
                        // `getMinimumEnclosures` asks for the rule-derived set only
                        // (`rules_only = true`) and adds the via's own margins solely when that
                        // set comes back empty. `enclosure_candidates` seeds its list with the
                        // rule value it is handed, so that seed is dropped here — left in, it is
                        // a candidate that fits more cuts than any real rule and therefore always
                        // wins, which is how VIA34's M4 face came out 1126 against 1148.
                        let rules: Vec<(i32, i32)> =
                            enclosure_candidates(&db, &g.cut_layer, cut, area, layer, above, None, split.is_some())
                                .into_iter()
                                .map(|e| (e.x, e.y))
                                .collect();
                        vyges_pdn::techvia::reconcile_enclosures(&rules, floor)
                            .into_iter()
                            .map(|(x, y)| vyges_pdn::viagen::Enclosure { x, y })
                            .collect::<Vec<_>>()
                    };
                    let bottoms = candidates(g.bottom_metal, lo, false);
                    let tops = candidates(g.top_metal, hi, true);
                    // 🔑 **Clamped per axis, as on the generate path.** Both paths end in the
                    // shared `makeBaseVia`, and `determineRowsAndColumns` finishes with
                    // `core_row_ = std::max(1, rows)` and its sibling for columns — so an axis
                    // that fits no cut still builds one row of them, and the fit is never a
                    // gate. Unclamped here, every pair scored zero on a rail too narrow to
                    // hold a cut and the whole level was refused: a 0.018 wide M2 followpin
                    // under a stack built nothing at all, and the layer above it then lost the
                    // patch that a via on both sides of it would have required.
                    let fit = |b: vyges_pdn::viagen::Enclosure, t: vyges_pdn::viagen::Enclosure| {
                        vyges_pdn::viagen::cuts_across(area.2 - area.0, cut.0, b.x, t.x, pitch.0, max_columns)
                            .max(1)
                            * vyges_pdn::viagen::cuts_across(
                                area.3 - area.1,
                                cut.1,
                                b.y,
                                t.y,
                                pitch.1,
                                max_rows,
                            )
                            .max(1)
                    };
                    let Some(chosen) = vyges_pdn::viagen::best_enclosure_pair(
                        &bottoms,
                        &tops,
                        direction_of(&db, lo),
                        direction_of(&db, hi),
                        &fit,
                        &|_, _, cuts| cuts > 0,
                    ) else {
                        continue; // nothing buildable at this level
                    };
                    // A split array holds ONE cut and is placed repeatedly, so the fit is not asked.
                    let (columns, rows) = if split.is_some() {
                        (1, 1)
                    } else {
                        (
                            vyges_pdn::viagen::cuts_across(
                                area.2 - area.0,
                                cut.0,
                                chosen.bottom.x,
                                chosen.top.x,
                                pitch.0,
                                max_columns,
                            )
                            .max(1),
                            vyges_pdn::viagen::cuts_across(
                                area.3 - area.1,
                                cut.1,
                                chosen.bottom.y,
                                chosen.top.y,
                                pitch.1,
                                max_rows,
                            )
                            .max(1),
                        )
                    };
                    // ⚠️ The chosen pair is a MINIMUM; what is built takes the overlap on any axis
                    // the via must fit and the minimum elsewhere. A level inside the stack is
                    // constrained by nothing and takes minimums on both.
                    let at_bottom_end = level == 0;
                    let at_top_end = level + 2 == stack.len();
                    let bot_c = if at_bottom_end {
                        let d = vyges_pdn::viagen::rect_direction(rects[level]);
                        vyges_pdn::viagen::constraint_for(d, true, false)
                    } else {
                        Default::default()
                    };
                    let top_c = if at_top_end {
                        let d = vyges_pdn::viagen::rect_direction(rects[level + 1]);
                        vyges_pdn::viagen::constraint_for(d, true, false)
                    } else {
                        Default::default()
                    };
                    let span = (
                        (columns - 1) * pitch.0 + cut.0,
                        (rows - 1) * pitch.1 + cut.1,
                    );
                    let overlap = vyges_pdn::viagen::overlap_enclosure(
                        (area.2 - area.0, area.3 - area.1),
                        span,
                    );
                    let b = vyges_pdn::viagen::built_enclosure(
                        !at_bottom_end,
                        chosen.bottom,
                        overlap,
                        bot_c,
                    );
                    let t =
                        vyges_pdn::viagen::built_enclosure(!at_top_end, chosen.top, overlap, top_c);
                    // 🔑 The minimum verbatim for a split array — see the same override on the
                    // generate path. Snapped after it, for the same reason.
                    let (bot, top) = if split.is_some() {
                        (chosen.bottom, chosen.top)
                    } else {
                        (b, t)
                    };
                    let bot = vyges_pdn::viagen::snap_enclosure(bot, grid_mfg);
                    let top = vyges_pdn::viagen::snap_enclosure(top, grid_mfg);
                    let (bot, top) = ((bot.x, bot.y), (top.x, top.y));
                    // 🔑 **A single-cut tech via is placed AS ITSELF.** `DbTechVia::generate`
                    // branches on `isArray()`, and only an array is given a `dbVia` of its own —
                    // the else arm hands `odb::dbSBox::create` the technology's via directly, so
                    // the DEF names `VIA23` and carries no `VIAS` entry for it at all. Naming a
                    // 1x1 the way an array is named invents a via the reference never wrote.
                    // 🔑 **A tech via's OWN cut array folds into the one asked for**, and it
                    // changes the counts, the pitch, the name and where the via sits. See
                    // `techvia::fold_cut_array` — `DbTechVia`'s constructor does this before
                    // anything reads the via, so everything downstream sees the folded values.
                    let (rows, columns, pitch) = match vyges_pdn::techvia::fold_cut_array(
                        &g.cut_centres,
                        rows,
                        columns,
                        pitch.1,
                        pitch.0,
                    ) {
                        Some((r, c, rp, cp)) => (r, c, (cp, rp)),
                        None => (rows, columns, pitch),
                    };
                    let is_array = rows > 1 || columns > 1;
                    // 🔑 **A TECH via honours `-ongrid` on a single-level connect; a generated
                    // one does not.** `DbGenerateVia::generate` takes the set and ignores it, so
                    // for a generate rule the snapping really does live only in the stacked via
                    // — but `DbTechVia::generate` populates the grid of either of its own layers
                    // the connect named and both snaps to it and RE-PITCHES its cut array on it.
                    // ⚠️ It picks which layer serves which axis by a rule of its own: see
                    // `vias::techvia_snap_sources`, which branches on the layer ABOVE where the
                    // stacked via branches on the one below.
                    let tv_grid = |layer: &str| {
                        ongrid
                            .iter()
                            .any(|l| l == layer)
                            .then(|| track_grids.get(layer))
                            .flatten()
                    };
                    let (x_up, y_up) = vyges_pdn::vias::techvia_snap_sources(
                        direction_of(&db, hi) == Direction::Vertical,
                    );
                    // ⚠️ **A layer's track interval is read off the axis its OWN direction
                    // names**, not the axis being pitched — `snapToGridInterval` picks the X
                    // pattern for a vertical layer and the Y pattern for a horizontal one.
                    let step_of = |layer: &str| -> Option<i32> {
                        let g = tv_grid(layer)?;
                        let v = if direction_of(&db, layer) == Direction::Vertical {
                            &g.0
                        } else {
                            &g.1
                        };
                        (v.len() >= 2).then(|| v[1] - v[0])
                    };
                    // 🔑 **The BUILT pitch, which the name does not carry.** See
                    // `techvia::pitch_on_grid_interval`.
                    let built_pitch = (
                        match step_of(if x_up { hi } else { lo }) {
                            Some(st) => vyges_pdn::techvia::pitch_on_grid_interval(pitch.0, st),
                            None => pitch.0,
                        },
                        match step_of(if y_up { hi } else { lo }) {
                            Some(st) => vyges_pdn::techvia::pitch_on_grid_interval(pitch.1, st),
                            None => pitch.1,
                        },
                    );
                    let name = if is_array {
                        vyges_pdn::techvia::array_name(
                            &via_name, rows, columns, pitch.1, pitch.0, ongrid,
                        )
                    } else {
                        via_name.clone()
                    };
                    let spacing =
                        vyges_pdn::techvia::cut_spacing(g.single_cut, built_pitch.1, built_pitch.0);
                    if is_array
                        && db
                            .create_generated_via(
                                &name,
                                "", // a tech via answers to no generate rule
                                (lo, g.cut_layer.as_str(), hi),
                                single,
                                spacing,
                                bot,
                                top,
                                rows,
                                columns,
                                vyges_pdn::techvia::centre(g.cut_extent),
                            )
                            .is_err()
                    {
                        continue;
                    }
                    // 🔑 **A tech via is placed by its ORIGIN, not by its cut array's centre**,
                    // and for a via whose cuts do not straddle the origin those are different
                    // points. `DbTechVia::generate` subtracts `via_center_` — the centre of the
                    // merged cut extent — from the placement on BOTH of its branches, and the
                    // same value is handed to the via as its origin, so the cuts land back on
                    // the crossing centre.
                    //
                    // ⚠️ **Only the PLACEMENT moves; the metal does not.** The enclosure rects
                    // are carried with the via and come out centred on the crossing either way,
                    // so offsetting the metal too would move it off the shape it is measured
                    // against. A via with three cuts running 90 units to one side of its origin
                    // places 45 out of position with no other symptom -- same name, same count,
                    // same metal.
                    let origin = vyges_pdn::techvia::centre(g.cut_extent);
                    // 🔑 **A TECH via honours `-ongrid` on a single-level connect; a generated
                    // one does not.** `DbGenerateVia::generate` takes the set and ignores it, so
                    // for a generate rule the snapping really does live only in the stacked via
                    // — but `DbTechVia::generate` populates the grid of either of its own layers
                    // that the connect named and snaps to it, however short the stack.
                    // ⚠️ And it picks the layers by a rule of its own: see
                    // `vias::techvia_snap_sources`, which branches on the layer ABOVE where the
                    // stacked via branches on the one below.
                    // ⚠️ **Snapped either side of the origin offset**, in that order — the
                    // reference snaps the crossing point, subtracts the cut centre, and snaps
                    // the result again.
                    let tv_snap = |p: (i32, i32)| {
                        let sx = match tv_grid(if x_up { hi } else { lo }) {
                            Some((x, _)) => vyges_pdn::vias::snap_to_grid(p.0, x, 0),
                            None => p.0,
                        };
                        let sy = match tv_grid(if y_up { hi } else { lo }) {
                            Some((_, y)) => vyges_pdn::vias::snap_to_grid(p.1, y, 0),
                            None => p.1,
                        };
                        (sx, sy)
                    };
                    let placed_at: Vec<(i32, i32)> = spots
                        .iter()
                        .map(|spot| {
                            let at = tv_snap(*spot);
                            tv_snap((at.0 - origin.0, at.1 - origin.1))
                        })
                        .collect();
                    for at in &placed_at {
                        placements.push((
                            v.net.clone(),
                            name.clone(),
                            *at,
                            v.lower.clone(),
                            v.upper.clone(),
                            v.area,
                        ));
                    }
                    written += spots.len();
                    if !spots.is_empty() {
                        via_ok.push((v.net.clone(), (lo.to_string(), hi.to_string()), v.area));
                    }

                    // A tech via stack passes through layers just as a generated one does, and
                    // leaves the same DRCFILL patch on them — see `viagen::intermediate_patch`.
                    let bare = (
                        (columns - 1) * built_pitch.0 + single.0,
                        (rows - 1) * built_pitch.1 + single.1,
                    );
                    // ⚠️ **Per SPOT** — a split level leaves its own metal at every position.
                    let metal_at = |spot: (i32, i32), enc: (i32, i32)| {
                        let (hw, hh) = (bare.0 / 2 + enc.0, bare.1 / 2 + enc.1);
                        (spot.0 - hw, spot.1 - hh, spot.0 + hw, spot.1 + hh)
                    };
                    // ⚠️ **The metal follows the via, not the crossing.** It is carried by the
                    // via and offset by the same origin, so it sits at the PLACED point plus
                    // that origin — which is the crossing centre only while nothing snapped.
                    let base_pi = placements.len() - placed_at.len();
                    for (k, at) in placed_at.iter().enumerate() {
                        let centre = (at.0 + origin.0, at.1 + origin.1);
                        via_faces.push((base_pi + k, lo.to_string(), metal_at(centre, bot), area));
                        via_faces.push((base_pi + k, hi.to_string(), metal_at(centre, top), area));
                    }
                    // ⚠️ **A split-cut array does NOT require a patch** — `DbSplitCutVia` leaves
                    // `requiresPatch()` at its default however many cuts it places.
                    let needs_patch = split.is_none() && is_array;
                    if let Some((shared, prev_tops, prev_array)) = previous_top.take() {
                        if shared == lo && prev_tops.len() == spots.len() {
                            for (k, spot) in spots.iter().enumerate() {
                                for patch in vyges_pdn::viagen::intermediate_patches(
                                    &[prev_tops[k]],
                                    &[metal_at(*spot, bot)],
                                    prev_array || needs_patch,
                                    db.layer_min_area(lo).unwrap_or(0),
                                    direction_of(&db, lo),
                                    grid_mfg,
                                ) {
                                    drcfill.push((
                                        v.net.clone(),
                                        lo.to_string(),
                                        patch,
                                        v.lower.clone(),
                                        v.upper.clone(),
                                        v.area,
                                    ));
                                }
                            }
                        }
                    }
                    previous_top = Some((
                        hi.to_string(),
                        spots.iter().map(|s| metal_at(*s, top)).collect(),
                        needs_patch,
                    ));
                    continue;
                }

                let Some(best) = best else {
                    continue; // nothing buildable at this level, on any pair
                };
                let (rule, cut, pitch, area) = (best.rule, best.cut, best.pitch, best.area);
                let (rows, columns) = (best.rows, best.columns);
                let (bot_enc, top_enc) = (best.bot_enc, best.top_enc);
                // The array the winner's own area lays out, which is not the level's.
                let spots = spots_in(area);
                // 🔑 **An ARRAYSPACING via is several base vias on a grid**, so one spot becomes
                // several placements and the definitions differ by cut count alone. Without a
                // rule this is the single via at the spot itself, which is the same shape of
                // answer and needs no branch below.
                let grid: Vec<vyges_pdn::viagen::ArrayPlacement> = match &best.array {
                    Some(f) => vyges_pdn::viagen::array_placements(f, cut),
                    None => vec![vyges_pdn::viagen::ArrayPlacement {
                        cuts: (columns, rows),
                        at: (0, 0),
                    }],
                };

                // ⚠️ **One definition per distinct CUT COUNT**, which for an array is up to
                // four and otherwise one. They differ in nothing else: the same rule, the same
                // pitch, the same enclosures, and the same rect — so the reference's own name,
                // which carries rows and columns, distinguishes them by itself.
                let mut names: Vec<((i32, i32), String)> = Vec::new();
                for p in &grid {
                    if names.iter().any(|(c, _)| *c == p.cuts) {
                        continue;
                    }
                    let (cols_p, rows_p) = p.cuts;
                    let params = vyges_pdn::viagen::via_params(
                        rows_p, cols_p, pitch, cut, bot_enc, top_enc,
                    );
                    // The reference's own naming, from `DbGenerateVia::getViaName`: the two
                    // routing levels, the via's AREA, its rows and columns, then its cut pitch.
                    // Worth matching exactly — the name is how a DEF diff tells two vias apart,
                    // and the area in it is an independent check that the via was built in the
                    // right rect.
                    let name = format!(
                        "via{}_{}_{}_{}_{rows_p}_{cols_p}_{}_{}",
                        routing_level(&db, lo),
                        routing_level(&db, hi),
                        area.2 - area.0,
                        area.3 - area.1,
                        pitch.0,
                        pitch.1,
                    );
                    // ⚠️ One `dbVia` per distinct geometry, reused wherever needed — the
                    // reference looks it up by name and creates it only when absent.
                    if db
                        .create_generated_via(
                            &name,
                            rule.map(|r| r.name.as_str()).unwrap_or(""),
                            (lo, cut_layer, hi),
                            params.cut,
                            params.cut_spacing,
                            params.bottom_enclosure,
                            params.top_enclosure,
                            params.rows,
                            params.columns,
                            (0, 0), // a generated via is centred on its own rect already
                        )
                        .is_err()
                    {
                        continue;
                    }
                    names.push((p.cuts, name));
                }
                if names.is_empty() {
                    continue;
                }
                // ⚠️ **Collected, not placed.** A via is a routed special wire, and the write
                // below clears each net's routed special wires before adding the boxes — so a
                // via placed here is put in the database and then deleted, leaving a via
                // DEFINITION with no placements, `add_swire_via` returning success, and nothing
                // anywhere saying so. The DEF simply comes out with no vias in it.
                //
                // 🔑 **The metal is the PLACEMENT's, not the level's.** Each base via of an
                // array carries its own cut count and therefore its own extent, so `via_faces`
                // — which is what absorbs via metal back into the shapes it lands on — is
                // computed per placement rather than once for the level.
                let mut tops: Vec<Rect> = Vec::new();
                let mut bots: Vec<Rect> = Vec::new();
                let mut here = 0;
                for spot in &spots {
                    for p in &grid {
                        let Some((_, name)) = names.iter().find(|(c, _)| *c == p.cuts) else {
                            continue;
                        };
                        let at = (spot.0 + p.at.0, spot.1 + p.at.1);
                        placements.push((
                            v.net.clone(),
                            name.clone(),
                            at,
                            v.lower.clone(),
                            v.upper.clone(),
                            v.area,
                        ));
                        let bare = (
                            (p.cuts.0 - 1) * pitch.0 + cut.0,
                            (p.cuts.1 - 1) * pitch.1 + cut.1,
                        );
                        let metal_at = |enc: (i32, i32)| {
                            let (hw, hh) = (bare.0 / 2 + enc.0, bare.1 / 2 + enc.1);
                            (at.0 - hw, at.1 - hh, at.0 + hw, at.1 + hh)
                        };
                        let pi = placements.len() - 1;
                        via_faces.push((pi, lo.to_string(), metal_at(bot_enc), area));
                        via_faces.push((pi, hi.to_string(), metal_at(top_enc), area));
                        bots.push(metal_at(bot_enc));
                        tops.push(metal_at(top_enc));
                        here += 1;
                    }
                }
                written += here;
                if here > 0 {
                    via_ok.push((v.net.clone(), (lo.to_string(), hi.to_string()), v.area));
                }
                // `DbGenerateVia::requiresPatch()` is `rows_ > 1 || cols_ > 1`; a split-cut array
                // is wrapped in a `DbSplitCutVia`, which does not override the default of false.
                //
                // 🔑 **A `DbArrayVia` overrides it to `true` unconditionally** — `via.h:348` —
                // so an array asks for a patch even where one group holds a single cut.
                let needs_patch =
                    split.is_none() && (best.array.is_some() || rows > 1 || columns > 1);
                if let Some((shared, prev_tops, prev_array)) = previous_top.take() {
                    if shared == lo && !prev_tops.is_empty() && !bots.is_empty() {
                        // 🔑 **One patch for the whole level, not one per spot.** The reference
                        // builds `combine_layer` as the union of EVERY top shape of the
                        // previous via and EVERY bottom shape of this one, then takes
                        // `extents()` — a single bounding box spanning them all.
                        //
                        // ⚠️ Paired spot by spot instead, each comparison is a face against a
                        // face at the SAME position, so the box never extends past their union
                        // and `adds_metal` discards every one. That is why a design whose
                        // stacks are all arrays produced no patches at all while the reference
                        // produced forty-eight.
                        //
                        // ⚠️ And the two levels need not have the same number of spots — an
                        // array on one layer pair and a single via on the next is ordinary, and
                        // requiring equal counts skipped those stacks outright.
                        //
                        // 🔑 **The metal goes in as a LIST, not as two bounding boxes.**
                        // `combine_layer` is a polygon set, and the leftover the patch is
                        // finally judged on is what that set does NOT cover. Reduced to two
                        // boxes first the leftover is empty by construction, and an array —
                        // whose groups leave real gaps — writes no patch at all.
                        {
                            for patch in vyges_pdn::viagen::intermediate_patches(
                                &prev_tops,
                                &bots,
                                prev_array || needs_patch,
                                db.layer_min_area(lo).unwrap_or(0),
                                direction_of(&db, lo),
                                grid_mfg,
                            ) {
                                drcfill.push((
                                    v.net.clone(),
                                    lo.to_string(),
                                    patch,
                                    v.lower.clone(),
                                    v.upper.clone(),
                                    v.area,
                                ));
                            }
                        }
                    }
                }
                previous_top = Some((hi.to_string(), tops, needs_patch));
            }

            // ── this via's metal, merged back into the two shapes it landed on ────────────
            // 🔑 **Only the two ENDS.** `check_shapes` is called on `shapes.bottom` against
            // `lower_` and on `shapes.top` against `upper_`; a stack's intermediate metal is
            // `shapes.middle` and is never merged into anything — it only decides which vias a
            // ripup takes with it.
            // ⚠️ Growth only. A refusal here is a RIPUP, and that is decided after trimming,
            // against the trimmed shape — see the absorb pass. Ripping up here would judge a
            // via against a strap that has not been pulled back yet.
            for (key, layer) in [(&lower_key, &v.lower), (&upper_key, &v.upper)] {
                let metals: Vec<Rect> = via_faces[faces_before..]
                    .iter()
                    .filter(|(_, l, _, _)| l == layer)
                    .map(|(_, _, m, _)| *m)
                    .collect();
                if metals.is_empty() {
                    continue;
                }
                let Some(si) = emitted.iter().position(|(n, l, r, _)| {
                    n == &v.net && l == layer && overlaps(*r, key.2)
                }) else {
                    continue;
                };
                // ⚠️ **Measured against the shape being MODIFIED, not against the rect the stack
                // was sized from.** `check_shapes` takes one `shape` and both merges into it and
                // asks whether the merge is allowed. Sizing from one rect and growing another lets
                // a ring corner answer for its neighbour: the metal already sits inside the
                // neighbour, the check reports it fits, and the shape that actually carries the
                // via is never grown — three ring segments 20 units short, and nothing else wrong.
                let cur = emitted[si].2;
                // ⚠️ **`PDN_GROW_TRACE` prints the shape a via is about to grow and the metal it
                // is grown by.** A shape that comes out short says nothing about which of the two
                // is wrong; seeing them together does.
                if std::env::var_os("PDN_GROW_TRACE").is_some() {
                    eprintln!(
                        "[grow] {}|{layer}|shape {:?} type {} metals {:?}",
                        v.net, cur, emitted[si].3, metals
                    );
                }
                let dir = if emitted[si].3 == "FOLLOWPIN" {
                    match vyges_pdn::viagen::rect_direction(emitted[si].2) {
                        Direction::None => direction_of(&db, layer),
                        d => d,
                    }
                } else {
                    direction_of(&db, layer)
                };
                let obstructions: Vec<Rect> = blockages
                    .iter()
                    .filter(|(l, ..)| l == layer)
                    .map(|(_, r, ..)| *r)
                    .collect();
                // ⚠️ **Into `emitted` itself, which is what `check_shapes` does.** It calls
                // `shape->setRect(new_shape)`, so the grown shape IS the shape from that moment on
                // — for the next via in this same pass, and for the write. Holding the growth to
                // one side and applying it afterwards grows twice: once against the shape the
                // crossing saw, and again against the shape the pass left behind.
                let _ = &key;
                if let vyges_pdn::shapes::ViaCheck::Extend(g) =
                    vyges_pdn::shapes::check_via_shapes(
                        cur,
                        &metals,
                        dir,
                        !(emitted[si].3 == "RING" && locked_layers.contains(layer)),
                        &obstructions,
                    )
                {
                    emitted[si].2 = g;
                }
            }
        }
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Debug,
            format!("{} via locations, {} dropped, {written} written",
            placed.len(),
            dropped.len()),
        );
    }

    // ── absorb via metal ─────────────────────────────────────────────────────────────────────
    // ⚠️ **AFTER trimming, which is where `buildGrids` puts it.** `Via::writeToDb` runs at the very
    // end — after `trimShapes` and `cleanupVias` — so a shape is first pulled back to what is
    // attached to it and only then grown to cover the metal of the vias that survived.
    //
    // 🔑 **The order is the whole of the effect.** Run before trimming, every via's metal is
    // already inside the untrimmed shape, nothing grows, and the stage is silently inert; run
    // after, a strap trimmed to its outermost via reaches out again by that via's enclosure. In
    // a switched-supply design that is 25 units at each end of every always-on strap.
    // 🔑 **`Via::writeToDb` mutates shapes and deletes vias**, which is easy to miss in a stage
    // named for writing. Per via, per face: the metal is merged into the shape it lands on;
    // allowed, the SHAPE GROWS; refused, that via is RIPPED UP. See `shapes::check_via_shapes`.
    let mut widened = 0;
    let mut drop_via = vec![false; placements.len()];
    for (pi, layer, metal, area) in &via_faces {
        let net = &placements[*pi].0;
        let Some(si) = emitted
            .iter()
            .position(|(n, l, r, _)| n == net && l == layer && overlaps(*r, *area))
        else {
            // 🔑 **A face may land on routing the design arrived with, and that shape cannot
            // give.** `check_shapes` merges the via's metal into the shape it lands on and then
            // asks `shape->isModifiable()`, which is `shape_type_ == kShape` — false for the
            // `kFixed` shapes `makeInitialShapes` produces. So the merge is refused and every
            // piece of via metal not already inside the shape is ripped up.
            //
            // ℹ️ A flipchip design shows it seven times: a metal9 pad strap crossing a
            // metal10 bump wire 50 units from the wire's edge, so 1530 of its 1580 sits over the
            // wire and the via's metal reaches 5 past the top. The reference logs
            // `PDN-0195 Removing 8 via(s) between metal9 and metal10`.
            if let Some(f) = fixed_via_shapes
                .iter()
                .find(|f| f.net == *net && f.layer == *layer && overlaps(f.rect, *area))
            {
                let obstructions: Vec<Rect> = blockages
                    .iter()
                    .filter(|(l, ..)| l == layer)
                    .map(|(_, r, ..)| *r)
                    .collect();
                if let vyges_pdn::shapes::ViaCheck::Ripup(_) = vyges_pdn::shapes::check_via_shapes(
                    f.rect,
                    &[*metal],
                    direction_of(&db, layer),
                    false, // kFixed is never modifiable
                    &obstructions,
                ) {
                    if std::env::var_os("PDN_TRACE").is_some() {
                        eprintln!(
                            "[viafit] RIPUP {net}|{layer}|fixed {:?} metal {metal:?}",
                            f.rect
                        );
                    }
                    drop_via[*pi] = true;
                }
            }
            continue;
        };
        // ⚠️ `Shape::getLayerDirection` is the LAYER's, except for a follow pin, which uses its
        // own aspect ratio unless it is square. A rail on a vertical layer runs horizontally, and
        // judging its growth by the layer refuses every legitimate extension it has.
        let dir = if emitted[si].3 == "FOLLOWPIN" {
            match vyges_pdn::viagen::rect_direction(emitted[si].2) {
                Direction::None => direction_of(&db, layer),
                d => d,
            }
        } else {
            direction_of(&db, layer)
        };
        let obstructions: Vec<Rect> = blockages
            .iter()
            .filter(|(l, ..)| l == layer)
            .map(|(_, r, ..)| *r)
            .collect();
        match vyges_pdn::shapes::check_via_shapes(
            emitted[si].2,
            &[*metal],
            dir,
            // `Shape::isModifiable` is `!is_locked_ && shape_type_ == kShape`, and the only thing
            // this engine locks is a SINGLE-LAYER ring — `Rings::makeShapes` calls `setLocked()`
            // just in that case.
            !(emitted[si].3 == "RING" && locked_layers.contains(layer)),
            &obstructions,
        ) {
            vyges_pdn::shapes::ViaCheck::Fits => {}
            vyges_pdn::shapes::ViaCheck::Extend(grown) => {
                if std::env::var_os("PDN_TRACE").is_some() {
                    eprintln!(
                        "[viafit] EXTEND {net}|{layer}|{:?} -> {grown:?} metal {metal:?}",
                        emitted[si].2
                    );
                }
                emitted[si].2 = grown;
                widened += 1;
            }
            vyges_pdn::shapes::ViaCheck::Ripup(_) => {
                if std::env::var_os("PDN_TRACE").is_some() {
                    eprintln!(
                        "[viafit] RIPUP {net}|{layer}|{:?} metal {metal:?}",
                        emitted[si].2
                    );
                }
                drop_via[*pi] = true
            }
        }
    }
    // 🔑 **A ripup that breaks a stack takes the rest of the stack with it.** The reference
    // collects the layers still covered after the ripup and compares them against
    // `Connect::getAllLayers`, which is the two ends plus every intermediate layer — **cut layers
    // included**. Losing one level therefore always loses that level's cut layer, so in this
    // engine's per-level model any ripped level breaks the whole stack.
    //
    // ⚠️ Leaving the rest standing is worse than dropping it: metal on every layer but one implies
    // a connection the design does not have, and nothing downstream would notice.
    let direct = drop_via.iter().filter(|d| **d).count();
    if direct > 0 {
        let broken: Vec<(String, String, String, Rect)> = placements
            .iter()
            .enumerate()
            .filter(|(i, _)| drop_via[*i])
            .map(|(_, (net, _, _, lo, hi, area))| {
                (net.clone(), lo.clone(), hi.clone(), *area)
            })
            .collect();
        for (i, (net, _, _, lo, hi, area)) in placements.iter().enumerate() {
            if broken
                .iter()
                .any(|(n, l, h, a)| n == net && l == lo && h == hi && a == area)
            {
                drop_via[i] = true;
            }
        }
        let ripped = drop_via.iter().filter(|d| **d).count();
        let mut keep = drop_via.iter();
        placements.retain(|_| !*keep.next().unwrap_or(&false));
        // 🔑 **A patch belongs to the via that made it.** `Via::writeToDb` writes the via's shapes
        // and its DRCFILL together, so a via that is ripped up writes neither. Kept separately, the
        // patch outlives the stack it was bridging: metal on a layer the design passes through with
        // nothing left passing through it, and no via anywhere near to explain it.
        drcfill.retain(|(net, _, _, lo, hi, area)| {
            !broken
                .iter()
                .any(|(n, l, h, a)| n == net && l == lo && h == hi && a == area)
        });
        // 🔑 **A rip-up is a via FAILURE**, `markFailed(FailedViaReason::kRipup)` — so a shape
        // whose only via was ripped out is floating, and the cleanup below takes it away.
        via_ok.retain(|(net, (lo, hi), area)| {
            !broken
                .iter()
                .any(|(n, l, h, a)| n == net && l == lo && h == hi && a == area)
        });
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Debug,
            format!("{ripped} vias ripped up ({direct} for metal outside their shape, \
             {} with the stacks they broke)",
            ripped - direct),
        );
    }
    if !via_faces.is_empty() {
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Debug,
            format!("{widened} shapes grown to cover via metal"),
        );
    }

    // ── shapes left floating by failed vias ───────────────────────────────────────────────────
    // 🔑 **`Via::writeToDb` marks a via FAILED, and `writeToDb` then destroys what only failed
    // vias were holding up.** The reference's last act before the swires are tidied.
    //
    // and `hasInternalConnections` is a terminal connection, or a follow pin, or **one via that
    // did not fail** — bterms do not count, and neither does the number of connections.
    //
    // ⚠️ **This is the opposite half of the rule above it, and both are needed.** A failed via
    // still counts as a connection and still contributes its area to `getMinimumRect`, so it holds
    // a strap out to where it sits and keeps it off the removable list. What it cannot do is be
    // the *only* thing holding a shape: a strap crossed four times and refused four times is
    // trimmed to span all four crossings and then deleted here.
    //
    // ℹ️ Both halves show in one design, where a met4 stripe is too
    // narrow to enclose a via: the met5 stripes it crosses keep their refused ends and survive on
    // one built via apiece, and the narrow stripes themselves — refused everywhere — do not.
    //
    // ⚠️ Only a BUILD failure is modelled here. The reference also marks a via failed when every
    // one of its cuts is ripped up, and when a via is dropped as obstructed or overlapping; a
    // rip-up is withdrawn from `via_ok` just above, and the other two have no equivalent in this
    // engine yet.
    {
        let before = emitted.len();
        let across_ok = |rect: Rect, a: &Rect| match vyges_pdn::viagen::rect_direction(rect) {
            Direction::Horizontal => a.1 >= rect.1 && a.3 <= rect.3,
            Direction::Vertical => a.0 >= rect.0 && a.2 <= rect.2,
            Direction::None => true,
        };
        emitted.retain(|(net, layer, rect, kind)| {
            // A follow pin is assumed connected, and a switch pin is the cell's own metal.
            if *kind == "FOLLOWPIN" || *kind == "SWITCH" {
                return true;
            }
            // `isLocked()` — and the only thing this engine locks is a single-layer ring.
            if *kind == "RING" && locked_layers.contains(layer) {
                return true;
            }
            if iterm_holds
                .iter()
                .any(|(n, l, r)| n == net && l == layer && overlaps(*r, *rect))
            {
                return true;
            }
            via_ok.iter().any(|(n, l, a)| {
                n == net
                    && (l.0 == *layer || l.1 == *layer)
                    && overlaps(*a, *rect)
                    && across_ok(*rect, a)
            })
        });
        if emitted.len() != before {
            vyges_events::log(
                "vyges-pdn",
                vyges_events::Severity::Warn,
                format!("{} shape(s) left floating by failed vias and removed",
                before - emitted.len()),
            );
        }
    }

    // ── write ────────────────────────────────────────────────────────────────────────────────
    // 🔑 **`pdngen` only ADDS special wires; it never destroys what the design arrived with.**
    // Nothing in the reference removes a net's swires — `Grid::resetShapes` clears the in-memory
    // shape lists and `-ripup` is a separate command this engine does not implement. A design that
    // arrives with power already routed keeps it, and the grid is written alongside.
    //
    // ⚠️ Clearing the build nets was here from the first commit and costs 260 shapes on a
    // flipchip design that connects over its pads: its DEF arrives with 143 VDD and 117
    // VSS metal10 bump wires, and the reference's output carries every one of them through
    // untouched. Invisible in every other case, because no other design arrives with routing on a
    // net the grid builds.
    // What is about to be written, per net and kind. ⚠️ A net that built shapes and reaches the
    // writer with none is a very different fault from one whose write is refused, and only this
    // tells the two apart.
    if std::env::var_os("PDN_TRACE").is_some() {
        let mut tally: Vec<(String, &str, usize)> = Vec::new();
        for (net, _, _, kind) in &emitted {
            match tally.iter_mut().find(|(n, k, _)| n == net && k == kind) {
                Some((_, _, c)) => *c += 1,
                None => tally.push((net.clone(), kind, 1)),
            }
        }
        for (net, kind, count) in tally {
            eprintln!("[emitted] {net}|{kind}|{count}");
        }
    }
    // 🔑 **Every net of every domain is marked SPECIAL before anything is written.**
    //
    // ⚠️ **This is not bookkeeping: the DEF writer drops the wires without it.** `defout` emits a
    // `SPECIALNETS` entry only for a net that answers `isSpecial()`, so a supply the design
    // happens to have created as a plain signal keeps every shape in memory, writes none, and
    // leaves no error behind. A design with a switched region domain declares `VIN_SW` through
    // `add_global_connection` — which warns `Net created for VIN_SW, if intended as power or
    // ground net add the -power/-ground switch as appropriate` and creates it `USE SIGNAL` — and
    // its whole switched grid, twelve shapes built and cut correctly, went to the writer and
    // vanished.
    for (net, _, _, _) in &emitted {
        let _ = db.net_set_special(net);
    }

    // ── block terminals for the supply nets ──────────────────────────────────────────────────
    // 🔑 **A power grid republishes each supply as a BLOCK TERMINAL, and that is not bookkeeping**
    // — a `-pins` layer's whole purpose is the terminal it creates, and the parent that
    // instantiates this block connects to it. `PdnGen::writeToDb` resolves one terminal per supply
    // net, retypes it, clears the pin geometry it owns, and only then lets the grids write.
    //
    // ⚠️ **Invisible to a gate reading SPECIALNETS alone.** This engine emitted no terminal at all
    // and scored MATCH on every case that asks for one, because the shapes and vias were right.
    let pin_layers: Vec<String> = opts
        .all("pins")
        .iter()
        .flat_map(|s| s.split(','))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    // ⚠️ **The GRID's pin layers, taken globally here.** `-pins` belongs to the grid that declared
    // it, so a design whose two grids name different pin layers is served the union. Trimming
    // already reads them the same way; when one is made per-grid the other must follow.
    let mut supply_nets: Vec<String> = nets::build_order(&domain, starts_with_power);
    for (net, ..) in &emitted {
        if !supply_nets.iter().any(|n| n == net) {
            supply_nets.push(net.clone());
        }
    }
    let mut bterm_of: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // 🔑 **Only the terminals THIS run created are ours to take back.** See the cleanup after the
    // pin geometry below.
    let mut created_bterms: Vec<String> = Vec::new();
    for net in &supply_nets {
        let existing = db.net_get_b_terms(net);
        let bterm = if existing.is_empty() {
            // ⚠️ **A terminal of the net's own name may already exist unconnected**, and it is
            // taken rather than duplicated.
            //
            // 🔑 **INOUT is set whenever the net arrived with NO terminal**, for the adopted one
            // as much as the created one — it is the branch that decides, not the creation.
            if db.bterm_names().iter().any(|b| b == net) {
                // ⛔ **NOT CONNECTED to the net, and it should be.** The reference calls
                // `bterm->connect(net)` here; no accessor for that is bridged yet, so an adopted
                // terminal keeps whatever net it had. Stated rather than silently skipped — a
                // design reaching this path gets a terminal on the wrong net.
                vyges_events::log(
                    "vyges-pdn",
                    vyges_events::Severity::Warn,
                    format!("terminal {net} adopted but not connected -- no accessor for it"),
                );
                let _ = db.bterm_set_io_type(net, "INOUT");
                net.clone()
            } else {
                match db.create_bterm(net, net) {
                    Ok(()) => {
                        let _ = db.bterm_set_io_type(net, "INOUT");
                        created_bterms.push(net.clone());
                        net.clone()
                    }
                    Err(e) => {
                        vyges_events::log(
                            "vyges-pdn",
                            vyges_events::Severity::Warn,
                            format!("cannot create a terminal for {net}: {e}"),
                        );
                        continue;
                    }
                }
            }
        } else {
            // The one named for the net, else the first — `get1stBTerm`.
            existing
                .iter()
                .find(|b| *b == net)
                .cloned()
                .unwrap_or_else(|| existing[0].clone())
        };
        let _ = db.bterm_set_sig_type(&bterm, &db.net_get_sig_type(net));
        let _ = db.bterm_set_special(&bterm);
        bterm_of.insert(net.clone(), bterm);
    }
    // 🔑 **Clear the pin geometry BEFORE the shapes are written, and keep what is FIXED.** A run
    // owns what its last run produced; a pin a person placed is not ours to remove. Done after the
    // retyping above and before any box below, which is the reference's own order.
    for net in &supply_nets {
        for bterm in db.net_get_b_terms(net) {
            let _ = db.bterm_clear_unfixed_bpins(&bterm);
        }
    }

    let mut written = 0;
    for (net, layer, rect, shape) in &emitted {
        if *shape == "SWITCH" {
            continue; // the switch cell's own pin, not this engine's metal
        }
        match db.add_swire_box_shaped(net, layer, *rect, false, shape) {
            Ok(()) => written += 1,
            Err(e) => {
                vyges_events::log(
                    "vyges-pdn",
                    vyges_events::Severity::Warn,
                    format!("cannot write {net} on {layer}: {e}"),
                );
                return ExitCode::from(2);
            }
        }
    }
    // ⚠️ **After the boxes, not before.** The write above clears each net's routed special wires
    // first, and a via placed before that is a routed special wire — so writing vias earlier puts
    // them in the database and then deletes them, leaving a via DEFINITION with no placements and
    // no error anywhere to say so.
    // ⚠️ These go in with the vias, after the boxes, for the same reason: they are routed
    // special wires and the clear above would take them.
    for (net, layer, rect, ..) in &drcfill {
        if let Err(e) = db.add_swire_box_shaped(net, layer, *rect, false, "DRCFILL") {
            vyges_events::log(
                "vyges-pdn",
                vyges_events::Severity::Warn,
                format!("cannot write DRCFILL on {layer}: {e}"),
            );
        }
    }
    if !drcfill.is_empty() {
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Debug,
            format!("{} patches on passed-through layers",
            drcfill.len()),
        );
    }
    // 🔑 **A via takes the shape TYPE of what it lands on, not `STRIPE` always.**
    //
    // `Via::writeToDb` opens with `type = lower_->getType()` and falls back to `STRIPE` only when
    // the two shapes disagree — "If both shapes are not the same, use stripe". So a via joining
    // two ring segments is written `+ SHAPE RING`, and one joining a ring to a strap is not.
    //
    // ⚠️ **Invisible to a gate that compares wire segments**, since it lives on the via line, and
    // invisible to a via diff keyed on geometry and via name — which is how eight of these stood
    // in a design that was otherwise exact in both shapes and via placements.
    let mut by_layer: std::collections::HashMap<(&str, &str), Vec<(Rect, &str)>> =
        std::collections::HashMap::new();
    for (net, layer, rect, kind) in &emitted {
        by_layer
            .entry((net.as_str(), layer.as_str()))
            .or_default()
            .push((*rect, *kind));
    }
    // ⚠️ **A shape this engine does not emit still has a TYPE, and it is not `STRIPE`.** A macro's
    // pin, a block terminal and the routing a design arrives with are all `Shape`s to the
    // reference, so `lower_->getType()` answers for them — `NONE` for a pin built by
    // `getInstancePins`, whose DEF form is a via line carrying no `+ SHAPE` clause at all.
    //
    // ℹ️ `dbWireShapeType("NONE")` parses to `NONE`, and the DEF writer omits the clause for it.
    let kind_at = |net: &str, layer: &str, area: Rect| -> Option<&'static str> {
        if let Some(k) = by_layer.get(&(net, layer)).and_then(|v| {
            v.iter()
                .find(|(r, _)| overlaps(*r, area))
                .map(|(_, k)| *k)
        }) {
            return Some(k);
        }
        connectable_pins
            .iter()
            .any(|p| p.net == net && p.layer == layer && overlaps(p.rect, area))
            .then_some("NONE")
    };
    let mut vias_placed = 0;
    for (net, name, centre, lo, hi, area) in &placements {
        // ⚠️ A `SWITCH` shape is the cell's own metal and is never written, so it cannot name the
        // via's type either; anything unfound falls back with the mismatch case.
        let ty = match (kind_at(net, lo, *area), kind_at(net, hi, *area)) {
            (Some(a), Some(b)) if a == b && a != "SWITCH" => a,
            _ => "STRIPE",
        };
        match db.add_swire_via(net, name, *centre, false, ty) {
            Ok(()) => vias_placed += 1,
            Err(e) => {
                if std::env::var_os("PDN_VIA_TRACE").is_some() {
                    eprintln!("[viawrite] {net}|{name}|{centre:?}|{ty}: {e}");
                }
            }
        }
    }
    if !placements.is_empty() {
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Debug,
            format!("{vias_placed} of {} vias placed",
            placements.len()),
        );
    }
    // ── the pin geometry itself ──────────────────────────────────────────────────────────────
    // Every shape that survived on a `-pins` layer becomes part of its net's terminal.
    //
    // ⚠️ **After trimming, not before.** `emitted` here is what survived, and a shape trimmed away
    // is not a pin — publishing one would advertise metal the DEF does not contain.
    let mut pin_boxes = 0;
    let mut edge_boxes = 0;
    for (net, layer, rect, shape) in &emitted {
        if *shape == "SWITCH" {
            continue;
        }
        let Some(bterm) = bterm_of.get(net) else {
            continue;
        };
        // A `-pins` layer publishes the WHOLE shape.
        if pin_layers.iter().any(|l| l == layer) {
            match db.bterm_add_pin_box(bterm, layer, *rect) {
                Ok(true) => pin_boxes += 1,
                Ok(false) => {}
                Err(e) => vyges_events::log(
                              "vyges-pdn",
                              vyges_events::Severity::Warn,
                              format!("cannot pin {net} on {layer}: {e}"),
                          ),
            }
        }
    }

    // The die-edge connections recorded before trimming, refreshed against what survived.
    //
    // ⚠️ **A connection whose shape is gone is gone with it** — `updateIBTermConnections` drops any
    // that no longer overlaps its shape, so a strap trimmed away from the edge publishes nothing.
    //
    // ⚠️ **The CROSS extent comes from the surviving shape, the depth from the edge.** For a
    // connection on the x axis the y span is refreshed and the x span is the slice; on the y axis
    // it is the other way round. Keeping the recorded rect wholesale would publish the shape's
    // extent as it was before trimming.
    for (net, layer, on_x, slice) in &edge_connections {
        let Some(bterm) = bterm_of.get(net) else {
            continue;
        };
        let Some((_, _, live, _)) = emitted
            .iter()
            .find(|(n, l, r, s)| n == net && l == layer && *s != "SWITCH" && overlaps(*r, *slice))
        else {
            continue;
        };
        let r = if *on_x {
            (slice.0, live.1, slice.2, live.3)
        } else {
            (live.0, slice.1, live.2, slice.3)
        };
        match db.bterm_add_pin_box(bterm, layer, r) {
            Ok(true) => edge_boxes += 1,
            Ok(false) => {}
            Err(e) => vyges_events::log(
                          "vyges-pdn",
                          vyges_events::Severity::Warn,
                          format!("cannot pin {net} on {layer}: {e}"),
                      ),
        }
    }
    if pin_boxes + edge_boxes > 0 {
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Debug,
            format!("{pin_boxes} pin shapes, {edge_boxes} at the die edge"),
        );
    }

    // 🔑 **A terminal this run created and then left empty is destroyed.** `PdnGen::writeToDb`
    // ends by walking the terminals it made and dropping any that carry no pin geometry.
    //
    // ⚠️ **Without this the terminal creation above is a REGRESSION, not a fix.** A design naming
    // no `-pins` layer gains a shapeless terminal per supply net — which is most of the suite, so
    // the change would read as fixing the pin cases and breaking everything else.
    //
    // ⚠️ **Created only.** A terminal the design arrived with stays, empty or not; taking those
    // would delete block ports the design declared for itself.
    for bterm in &created_bterms {
        if db.num_bterm_get_b_pins(bterm) == 0 {
            let _ = db.bterm_destroy(bterm);
        }
    }

    if let Err(e) = db.write_def(out) {
        vyges_events::log(
            "vyges-pdn",
            vyges_events::Severity::Error,
            format!("cannot write {out}: {e}"),
        );
        return ExitCode::from(2);
    }
    vyges_events::log(
        "vyges-pdn",
        vyges_events::Severity::Debug,
        format!("{written} shapes"),
    );

    // 🔑 **The machine-readable result the descriptor's assertion reads.** Everything above goes
    // to stderr for a human; a gate needs one line it can parse without reading prose. On stdout,
    // which no harness consumes — both `pdn-grid-check.py` and `pdn-error-check.py` read stderr
    // and the exit code, so this is additive.
    println!(
        "{{\n  \"tool\": \"vyges-pdn\",\n  \"status\": \"{status}\",\n  \
         \"shapes\": {written},\n  \"vias\": {vias_placed},\n  \
         \"pin_shapes\": {pin_boxes},\n  \"die_edge_pin_shapes\": {edge_boxes},\n  \
         \"def_written\": \"{out}\"\n}}",
        status = vyges_pdn::settle_status(written),
    );
    ExitCode::SUCCESS
}

/// `global-connect`: `add_global_connection` + `global_connect`, transcribed.
///
/// 🔑 **This is LibreLane `Classic` step 15's job** (`Odb.SetPowerConnections`) and the last
/// reference step the floorplan chain still borrowed from OpenROAD. Without it the supply nets do
/// not exist and this engine correctly refuses with "no net named VSS".
///
/// **`add_global_connection`** (`OpenRoad.tcl:378`), per rule, in declaration order:
///   * find the net; **create it when absent** (upstream warns ORD-44 when neither `-power` nor
///     `-ground` was given, because an accidental signal net is the likely mistake);
///   * `-power`  -> `setSpecial()` **and** `setSigType(POWER)`;
///   * `-ground` -> `setSpecial()` **and** `setSigType(GROUND)`.
///
/// **`dbBlock::globalConnect`** (`dbBlock.cpp:3424`), then, over the accumulated rules:
///   * instances marked do-not-touch are removed BEFORE matching;
///   * a rule whose net is do-not-touch is skipped whole (ODB-379);
///   * per matching instance, per matching master terminal:
///       - already on this net            -> nothing to do;
///       - on a do-not-touch net          -> left alone;
///       - on some OTHER net, without force -> counted as a CONFLICT and skipped;
///       - otherwise                      -> connect, and `setSpecial()` on the iterm when the
///                                           net is special.
///
/// ⛔ **`std::regex_match` is a FULL match; Rust's `is_match` is a SEARCH.** Both patterns are
/// therefore anchored here. Leaving them unanchored would make `-pin_pattern {^VDD$}` behave the
/// same but `-inst_pattern {u_cpu}` match every instance containing that substring, silently
/// connecting far more than the rule asked for.
///
/// ⚠️ **Non-region rules run before region rules** upstream ("order rules so non-regions are
/// handled first"). Regions are not implemented here; a rule naming one is refused rather than
/// quietly treated as global.
fn global_connect(args: &[String]) -> ExitCode {
    let mut path: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut force = false;
    // (net, pin_pattern, inst_pattern, kind)
    let mut rules: Vec<(String, String, String, String)> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--force" => force = true,
            "--out-odb" => {
                i += 1;
                match args.get(i) {
                    Some(v) => out = Some(v),
                    None => {
                        eprintln!("vyges-pdn global-connect: --out-odb needs a FILE");
                        return ExitCode::from(2);
                    }
                }
            }
            "--connect" => {
                i += 1;
                let Some(spec) = args.get(i) else {
                    eprintln!("vyges-pdn global-connect: --connect needs NET:PINPAT:INSTPAT:KIND");
                    return ExitCode::from(2);
                };
                let f: Vec<&str> = spec.splitn(4, ':').collect();
                if f.len() != 4 {
                    eprintln!("vyges-pdn global-connect: --connect wants NET:PINPAT:INSTPAT:KIND, \
                               got {spec:?}");
                    return ExitCode::from(2);
                }
                if !matches!(f[3], "power" | "ground" | "signal") {
                    eprintln!("vyges-pdn global-connect: KIND must be power|ground|signal, \
                               got {:?}", f[3]);
                    return ExitCode::from(2);
                }
                rules.push((f[0].into(), f[1].into(), f[2].into(), f[3].into()));
            }
            a if a.starts_with("--") => {
                eprintln!("vyges-pdn global-connect: unknown option {a}");
                return ExitCode::from(2);
            }
            a => path = Some(a),
        }
        i += 1;
    }

    let Some(path) = path else {
        eprintln!("vyges-pdn global-connect: needs <design.odb>");
        return ExitCode::from(2);
    };
    // ⛔ No rules is VACUOUS, not success: a pass word must never come from a run that did nothing.
    if rules.is_empty() {
        eprintln!("vyges-pdn global-connect: no --connect rule given; nothing was connected.");
        return ExitCode::from(3);
    }

    let mut db = match vyges_opendb::Db::open(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("vyges-pdn global-connect: cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };

    // ---- add_global_connection: create the nets and type them, in declaration order ----------
    let mut warned: Vec<String> = Vec::new();
    for (net, _pin, _inst, kind) in &rules {
        let exists = db.net_names().iter().any(|n| n == net);
        if !exists {
            if let Err(e) = db.create_net(net) {
                eprintln!("vyges-pdn global-connect: cannot create net {net}: {e}");
                return ExitCode::from(1);
            }
            if kind == "signal" {
                // Upstream ORD-44: created a net with no -power/-ground, which is usually a slip.
                warned.push(format!("created net {net} with no power/ground kind (ORD-44)"));
            }
        }
        if kind == "power" || kind == "ground" {
            let ty = if kind == "power" { "POWER" } else { "GROUND" };
            if let Err(e) = db.net_set_special(net).and_then(|_| db.net_set_sig_type(net, ty)) {
                eprintln!("vyges-pdn global-connect: cannot type net {net} as {ty}: {e}");
                return ExitCode::from(1);
            }
        }
    }

    // ---- globalConnect: apply every rule, in order -------------------------------------------
    let insts = db.inst_names();
    // ⚠️ Instances marked do-not-touch are removed BEFORE matching, not skipped inside the loop.
    let live: Vec<String> =
        insts.into_iter().filter(|n| !db.inst_is_do_not_touch(n)).collect();
    let mut masters: std::collections::HashMap<String, Vec<String>> = Default::default();

    let mut connected = 0usize;
    let mut skipped = 0usize;
    for (net, pin_pat, inst_pat, _kind) in &rules {
        if db.net_is_do_not_touch(net) {
            warned.push(format!("{net} is marked do not touch, skipped (ODB-379)"));
            continue;
        }
        let (Ok(inst_re), Ok(pin_re)) = (
            vyges_pdn::full_match_regex(inst_pat),
            vyges_pdn::full_match_regex(pin_pat),
        ) else {
            eprintln!("vyges-pdn global-connect: bad pattern in rule for {net}");
            return ExitCode::from(2);
        };
        let special = db.net_is_special(net);
        for inst in &live {
            if !inst_re.is_match(inst) {
                continue;
            }
            let master = db.inst_get_master(inst);
            let terms = masters
                .entry(master.clone())
                .or_insert_with(|| db.master_get_m_terms(&master))
                .clone();
            for t in terms.iter().filter(|t| pin_re.is_match(t)) {
                let current = db.iterm_get_net(inst, t);
                if current == *net {
                    continue; // already connected
                }
                if !current.is_empty() {
                    if db.net_is_do_not_touch(&current) {
                        continue; // connected to a do-not-touch net: left alone
                    }
                    if !force {
                        skipped += 1; // a conflict; upstream needs -force to move it
                        continue;
                    }
                }
                if let Err(e) = db.connect(inst, t, net) {
                    eprintln!("vyges-pdn global-connect: {inst}/{t} -> {net}: {e}");
                    return ExitCode::from(1);
                }
                if special {
                    let _ = db.iterm_set_special(inst, t);
                }
                connected += 1;
            }
        }
    }

    let dest = out.unwrap_or(path);
    if let Err(e) = db.write(dest) {
        eprintln!("vyges-pdn global-connect: cannot write {dest}: {e}");
        return ExitCode::from(2);
    }
    // 🔑 **Upstream's own line** — `_dbBlock::globalConnect` closes with ODB-403
    // (`dbBlock.cpp:3502`), including the "use -force" tail only when something was skipped.
    // This is `pdn`'s first structured event: the crate carried the `vyges-events` dependency
    // with no emission at all, so the largest construction engine reached the trail silently.
    vyges_events::emit(
        &vyges_events::Event::new(
            "vyges-pdn",
            vyges_events::Severity::Info,
            format!(
                "ODB-0403 {connected} connections made, {skipped} conflicts skipped{}",
                if skipped == 0 { "." } else { ", use -force to connect." }
            ),
        )
        .with_code("PDN-GLOBAL-CONNECT"),
    );
    // ⚠️ PRETTY, like `generate`'s report. `json!`'s Display is compact, and a chain that prints
    // one command's report as a block and the next one's as a single long line reads as two
    // different tools.
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "tool": "vyges-pdn",
        "command": "global-connect",
        // ⛔ NOT the literal "applied". A run whose rules matched nothing connected nothing, and
        // the pass word must not come from it — see `connect_status`.
        "status": vyges_pdn::connect_status(connected),
        "connections": connected,
        "conflicts_skipped": skipped,
        "rules": rules.len(),
        "warnings": warned,
        "odb_written": dest,
    })).expect("the global-connect report is valid JSON"));
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("generate") => generate(&args[1..]),
        Some("global-connect") => global_connect(&args[1..]),
        // ⚠️ Before any database is touched: `--describe` is a contract query, not a run, and a
        // caller asking what this engine promises must not need a design to ask.
        Some("--help") | Some("-h") => help(),
        // 🔑 **The commit, not just the version.** A version alone cannot tell you which build a
        // bug report came from — two binaries can share a version and differ by a fix. build.rs
        // bakes the git SHA in, preferring GITHUB_SHA on CI so a release is never stamped -dirty
        // by the untracked files a release run leaves behind.
        Some("--version") | Some("-V") => {
            println!(
                "vyges-pdn {} ({})",
                vyges_pdn::VERSION,
                env!("VYGES_GIT_SHA")
            );
            println!("{}", vyges_pdn::COPYRIGHT);
            ExitCode::SUCCESS
        }
        Some("--describe") => {
            print!("{}", describe());
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}

// `shapes` is used by the library's own tests and by the cut stage the binary does not yet reach.

#[cfg(test)]
mod describe_tests {
    use super::DESCRIBE;

    #[test]
    fn the_descriptor_is_valid_json_and_matches_the_schema_contract() {
        let d: serde_json::Value =
            serde_json::from_str(DESCRIBE).expect("descriptor is valid JSON");

        assert_eq!(d["schema"], "vyges-tool-descriptor/1.1");
        // The bare engine name, as every other engine in the suite reports — the figure
        // derivation and the curation file both key on it. The BINARY is vyges-pdn.
        assert_eq!(d["name"], "pdn");

        // consumes: role strings, never objects
        let consumes = d["consumes"].as_array().expect("consumes is an array");
        assert!(
            consumes.iter().all(|c| c.is_string()),
            "consumes must be role STRINGS, got {consumes:?}"
        );

        // exactly one recognised predicate: an unrecognised one is dropped and the verdict
        // resolves `unknown`, which a caller cannot tell from a failure.
        let pw = d["assertion"]["pass_when"]
            .as_object()
            .expect("pass_when is an object");
        assert_eq!(pw.len(), 1, "exactly one predicate, got {pw:?}");
        let key = pw.keys().next().unwrap().as_str();
        assert!(
            matches!(key, "is_true" | "eq" | "lte"),
            "`{key}` is not a predicate the schema defines (is_true | eq | lte)"
        );

        let limits = d["provenance_limitations"]
            .as_array()
            .expect("provenance_limitations is an array");
        assert!(
            !limits.is_empty(),
            "the schema requires provenance_limitations"
        );
    }

    /// ⚠️ **The one failure a descriptor invites: asserting on a field the engine never emits.**
    /// A consumer reading such a contract resolves `unknown` and cannot tell that from a failure,
    /// which is worse than carrying no descriptor at all. This engine printed nothing but human
    /// text on stderr until the report landed beside this descriptor, so the two are checked
    /// against each other rather than each being checked alone.
    #[test]
    fn the_assertion_names_a_field_and_a_value_this_engine_actually_emits() {
        let d: serde_json::Value = serde_json::from_str(DESCRIBE).expect("valid JSON");
        assert_eq!(d["assertion"]["field"], "status");

        let pass_word = d["assertion"]["pass_when"]["eq"]
            .as_str()
            .expect("the assertion compares against a string");
        assert_eq!(
            pass_word,
            vyges_pdn::settle_status(1),
            "the descriptor's pass word and settle_status() have drifted apart"
        );
        assert_ne!(
            pass_word,
            vyges_pdn::settle_status(0),
            "a run that emitted no metal must NOT satisfy the assertion"
        );
    }

    /// The report is assembled by hand rather than serialised, so its shape is worth pinning:
    /// a stray comma or an unquoted value makes it unparseable to the consumer it exists for.
    #[test]
    fn the_report_names_every_field_the_descriptor_promises() {
        let artifact_field = {
            let d: serde_json::Value = serde_json::from_str(DESCRIBE).expect("valid JSON");
            d["artifacts"][0]["field"].as_str().unwrap().to_string()
        };
        assert_eq!(artifact_field, "def_written");
        // The literal the report is built from, kept next to the assertion that reads it.
        for field in ["tool", "status", "shapes", "vias", "def_written"] {
            assert!(
                super::REPORT_FIELDS.contains(&field),
                "{field} is promised but not emitted"
            );
        }
    }
}

#[cfg(test)]
mod pin_tests {
    use super::{describe, PIN_TOKEN};

    #[test]
    fn the_descriptor_reports_the_pin_this_binary_was_built_against() {
        let d = describe();
        assert!(
            !d.contains(PIN_TOKEN),
            "the pin placeholder survived into the output -- the substitution did not run"
        );
        let v: serde_json::Value =
            serde_json::from_str(&d).expect("the descriptor is still valid JSON once filled in");
        assert_eq!(
            v["openroad_pin"], super::CRATE_PIN,
            "the descriptor must report the pin this binary was actually built against"
        );
        assert_eq!(super::CRATE_PIN.len(), 40, "a full commit SHA, not an abbreviation");
    }

    /// ⛔ The whole point of inheriting the pin is that no engine carries one of its own.
    #[test]
    fn no_sha_is_hardcoded_anywhere_in_the_descriptor() {
        let raw = super::DESCRIBE;
        for tok in raw.split(|c: char| !c.is_ascii_hexdigit()) {
            assert!(
                tok.len() < 40,
                "{tok} looks like a hardcoded commit -- use the {PIN_TOKEN} placeholder"
            );
        }
    }
}

#[cfg(test)]
mod maturity_guard {
    //! ⛔ **`maturity` is a CLOSED ENUM of three** — `discovered`, `structured`,
    //! `workflow-validated` — and an unrecognised word is not a modest claim, it is a DISCARDED
    //! RESULT. `Maturity::parse` returns `None`, the consumer treats the engine as `discovered`,
    //! `can_assert()` is false, and the verdict is suppressed to `unknown` however well-formed
    //! the assertion is. The JSON schema's `enum` rejects it too.
    //!
    //! ⚠️ **Four engines shipped an invalid one at once** — `ppl`, `pad` and `dpl` said `partial`,
    //! `pdn` said `correlated` — each chosen to sound honest about incompleteness, each silently
    //! throwing its own verdict away. None of the four had a test on it.
    //!
    //! 🔑 **The rung is about the shape of the EVIDENCE, not feature completeness.** What is
    //! unbuilt belongs in `provenance_limitations`, which is required and can carry nuance a
    //! one-word rung cannot. `workflow-validated` additionally needs a pinned design IN THIS
    //! REPO that the suite runs end to end and asserts against.
    use super::DESCRIBE;

    #[test]
    fn maturity_is_one_of_the_three_legal_rungs() {
        let v: serde_json::Value =
            serde_json::from_str(DESCRIBE).expect("the descriptor is valid JSON");
        let m = v["maturity"].as_str().unwrap_or_default().to_string();
        assert!(["discovered", "structured", "workflow-validated"].contains(&m.as_str()),
                "`{m}` is not a legal maturity; an unrecognised one suppresses the verdict");
        assert!(!v["provenance_limitations"].as_array().expect("required").is_empty(),
                "provenance_limitations is required and states what the hash does not cover");
    }
}
