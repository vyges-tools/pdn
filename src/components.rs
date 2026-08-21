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
/// 2. **Then `straps_` in insertion order** — and a pad direct connection is inserted by
///    `define_pdn_grid -connect_to_pads`, which necessarily precedes every `add_pdn_stripe` for
///    that grid. So pad connections lead the straps, and followpins and stripes follow
///    interleaved exactly as written.
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
    // ⚠️ **Once per connection, however many times the flag was given.** The flags together
    // decide WHICH pads connect and on which layers; the components come from the answer, not
    // from the flags.
    if values.iter().any(|(k, _)| k == "connect-to-pads") {
        for i in 0..pad_connections {
            out.push(Component::PadConnect(i));
        }
    }
    for (k, v) in values {
        match k.as_str() {
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
    fn pad_connections_precede_every_strap() {
        // The order the reference's own trace shows: ring, then all 16 pad connections, then the
        // followpin, then the straps.
        let v = opts(&[
            ("ring", "Metal4,Metal3"),
            ("followpins", "Metal1"),
            ("stripe", "Metal4"),
            ("stripe", "Metal5"),
            ("connect-to-pads", "all"),
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
