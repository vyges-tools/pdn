//! The order a grid's components are built in.
//!
//! This module holds *only* the sequence — which component runs when — kept apart from what each
//! component builds. The reference makes the same separation: `Grid::getGridComponents()` returns
//! the list, and `Grid::makeShapes` walks it without knowing what any entry is.
//!
//! ```text
//! for (auto* component : getGridComponents())          // <- plan(), below
//!   if (!component->make(shapes, obstructions))        // <- one make_* function per variant
//!     deferred.push_back(component);
//! for (auto* component : deferred)
//!   component->make(shapes, obstructions);             // <- the single retry
//! ```
//!
//! 🔑 **Order is the answer, not an implementation detail.** `GridComponent::make` is
//! `makeShapes → cutShapes → getObstructions → getShapes`: each component is cut against
//! everything built before it, then becomes an obstruction for everything after. Move one
//! component and its neighbours change shape.

/// One component of a grid, holding the option text that describes it.
///
/// The borrow is of the parsed command line, so a plan costs no allocation beyond the vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component<'a> {
    /// `--ring <layers>:<rest>`
    Ring(&'a str),
    /// One pad direct connection, identified by its position in the list the database yields.
    ///
    /// 🔑 **One component per connection, not one per `--connect-to-pads` flag.** The reference
    /// builds a `PadDirectConnectionStraps` per (instance, terminal) and each defers on its own;
    /// a single bulk component cannot express that, because a bulk call that builds *something*
    /// never reports failure and the connections inside it that built nothing never retry.
    PadConnect(usize),
    /// `--followpins <layer>[:<extend>[:<width>]]`
    Followpin(&'a str),
    /// `--stripe <layer>:<rest>`
    Strap(&'a str),
}

impl Component<'_> {
    /// The reference's own name for this component, as `GridComponent::typeToString` gives it.
    ///
    /// Used by the trace so our sequence can be diffed line-for-line against a run of the
    /// reference under `set_debug_level PDN Make 1`.
    pub fn kind(&self) -> &'static str {
        match self {
            Component::Ring(_) => "Ring",
            Component::PadConnect(_) => "Direct connect pin",
            Component::Followpin(_) => "Followpin",
            Component::Strap(_) => "Strap",
        }
    }

    /// The option text this component was described by, or its index for a pad connection.
    pub fn spec(&self) -> String {
        match self {
            Component::Ring(s) | Component::Followpin(s) | Component::Strap(s) => s.to_string(),
            Component::PadConnect(i) => format!("#{i}"),
        }
    }
}

/// The grid's components in build order, from the grid's own options in command-line order.
///
/// Two rules, both taken from the reference rather than chosen:
///
/// 1. **Rings first, always.** They live in `rings_`, a vector distinct from `straps_`, and
///    `getGridComponents` concatenates `rings_` ahead of `straps_` whatever order the commands
///    came in.
///
/// 2. **Then `straps_` in insertion order — and a pad direct connection is inserted where its
///    `-connect_to_pads` was WRITTEN.** `CoreGrid::setupDirectConnect` is what calls `addStrap`
///    for each pad, and it has two call sites: `PdnGen::makeCoreGrid`, reached by
///    `define_pdn_grid -connect_to_pads`, and `PdnGen::makeRing`, reached by
///    `add_pdn_ring -connect_to_pads`. The first runs before any `add_pdn_stripe` for that grid
///    can, so those pads lead every strap; the second runs at the ring statement's own position,
///    so pads written after a stripe follow it. Followpins and stripes stay interleaved exactly
///    as written either way.
///
/// ⛔ **"Pad connections always lead" is wrong, and it cost four vias.**
/// `pads_ihp_sg13g2_balance` is the one case in the corpus whose `add_pdn_ring -connect_to_pads`
/// comes AFTER its `add_pdn_stripe`s. Building the pads first made them obstructions for the core
/// straps, so four TopMetal2 straps were clipped 2000 — the TopMetal2 spacing — short of the VSS
/// ring instead of reaching it, made no via to it, and were trimmed back to their outermost
/// surviving via. Every downstream difference on that case followed from those four vias.
///
/// ⚠️ **Followpins and stripes must not be bucketed separately.** Both are `add_pdn_stripe`, so a
/// command line reading stripe M4, followpins, stripe M5 builds them in that order — not
/// followpins first. Walking the parsed options in order is what preserves that; collecting
/// `all("followpins")` and `all("stripe")` into two lists silently reorders them.
pub fn plan(values: &[(String, String)], pad_connections: usize) -> Vec<Component<'_>> {
    let mut out = Vec::new();
    for (k, v) in values {
        if k == "ring" {
            out.push(Component::Ring(v));
        }
    }
    // ⚠️ **Once per connection, however many times the flag was given, and at the position of
    // the FIRST occurrence.** The flags together decide WHICH pads connect and on which layers;
    // the components come from the answer, not from the flags. Emitting them here — inside the
    // walk rather than ahead of it — is what puts them at their written position.
    let mut pads_placed = false;
    for (k, v) in values {
        match k.as_str() {
            "connect-to-pads" if !pads_placed => {
                pads_placed = true;
                for i in 0..pad_connections {
                    out.push(Component::PadConnect(i));
                }
            }
            "followpins" => out.push(Component::Followpin(v)),
            "stripe" => out.push(Component::Strap(v)),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn kinds(plan: &[Component<'_>]) -> Vec<&'static str> {
        plan.iter().map(Component::kind).collect()
    }

    #[test]
    fn rings_lead_however_they_were_written() {
        // ⚠️ The ring jumps the queue; the stripe and the followpin do NOT swap. Bucketing by
        // option name would have made this "Ring, Followpin, Strap" — the reordering rule 2 exists
        // to prevent.
        let v = opts(&[("stripe", "M4"), ("ring", "M4,M3"), ("followpins", "M1")]);
        assert_eq!(kinds(&plan(&v, 0)), ["Ring", "Strap", "Followpin"]);
    }

    #[test]
    fn a_grid_spelling_puts_pad_connections_ahead_of_every_strap() {
        // `define_pdn_grid -connect_to_pads` reaches `setupDirectConnect` from
        // `PdnGen::makeCoreGrid`, before any `add_pdn_stripe` for the grid can run — the order the
        // reference's own trace shows on `pads_black_parrot_grid_define`: ring, then every pad
        // connection, then the followpin, then the straps.
        let v = opts(&[
            ("connect-to-pads", "all"),
            ("ring", "Metal4,Metal3"),
            ("followpins", "Metal1"),
            ("stripe", "Metal4"),
            ("stripe", "Metal5"),
        ]);
        // ⚠️ Three pads, three components — one each, so each can defer on its own.
        assert_eq!(
            kinds(&plan(&v, 3)),
            [
                "Ring",
                "Direct connect pin",
                "Direct connect pin",
                "Direct connect pin",
                "Followpin",
                "Strap",
                "Strap"
            ]
        );
    }

    #[test]
    fn a_ring_spelling_written_after_the_stripes_builds_the_pads_after_them() {
        // ⛔ The rule this test exists for. `add_pdn_ring -connect_to_pads` reaches
        // `setupDirectConnect` from `PdnGen::makeRing`, so its straps enter `straps_` at the ring
        // statement's own position — AFTER stripes declared above it. `pads_ihp_sg13g2_balance` is
        // written this way, and building the pads first made them obstructions that clipped four
        // TopMetal2 core straps short of the VSS ring.
        //
        // 🔑 The ring still leads: `getGridComponents` concatenates `rings_` ahead of `straps_`
        // however the commands were written. Only the PAD components move.
        let v = opts(&[
            ("stripe", "TopMetal1"),
            ("stripe", "TopMetal2"),
            ("ring", "TopMetal1,TopMetal2"),
            ("connect-to-pads", "all:ring"),
        ]);
        assert_eq!(
            kinds(&plan(&v, 2)),
            [
                "Ring",
                "Strap",
                "Strap",
                "Direct connect pin",
                "Direct connect pin"
            ]
        );
    }

    #[test]
    fn a_ring_spelling_written_before_the_stripes_still_leads_them() {
        // Every other `add_pdn_ring -connect_to_pads` case in the corpus is written this way, so
        // the position rule leaves them exactly where they were.
        let v = opts(&[
            ("ring", "metal8,metal9"),
            ("connect-to-pads", "all:ring"),
            ("stripe", "metal7"),
            ("stripe", "metal8"),
        ]);
        assert_eq!(
            kinds(&plan(&v, 1)),
            ["Ring", "Direct connect pin", "Strap", "Strap"]
        );
    }

    #[test]
    fn the_pads_land_once_however_many_times_the_flag_was_given() {
        // ⚠️ At the FIRST occurrence. `setupDirectConnect`'s answer is one list of connections;
        // repeating the flag names more layers, it does not build the pads a second time.
        let v = opts(&[
            ("connect-to-pads", "metal9"),
            ("stripe", "metal7"),
            ("connect-to-pads", "metal10"),
        ]);
        assert_eq!(
            kinds(&plan(&v, 1)),
            ["Direct connect pin", "Strap"]
        );
    }

    #[test]
    fn followpins_and_stripes_keep_their_written_order() {
        let v = opts(&[
            ("stripe", "M4"),
            ("followpins", "M1"),
            ("stripe", "M5"),
        ]);
        let p = plan(&v, 0);
        assert_eq!(kinds(&p), ["Strap", "Followpin", "Strap"]);
        assert_eq!(p[0].spec(), "M4");
        assert_eq!(p[2].spec(), "M5");
    }

    #[test]
    fn a_pad_flag_with_no_connections_behind_it_builds_nothing() {
        // ⚠️ The flag says the grid connects to pads; the database says which. None found, none
        // built — the flag on its own is not a component.
        let v = opts(&[("connect-to-pads", "all"), ("stripe", "M4")]);
        assert_eq!(kinds(&plan(&v, 0)), ["Strap"]);
    }

    #[test]
    fn unrelated_options_contribute_nothing() {
        let v = opts(&[("power", "VDD"), ("ground", "VSS"), ("trim", "0")]);
        assert!(plan(&v, 0).is_empty());
    }

    #[test]
    fn every_ring_is_kept_in_order() {
        let v = opts(&[("ring", "M4,M3"), ("ring", "M6,M5")]);
        let p = plan(&v, 0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].spec(), "M4,M3");
        assert_eq!(p[1].spec(), "M6,M5");
    }
}
