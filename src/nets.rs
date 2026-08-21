// SPDX-License-Identifier: Apache-2.0
//! Which nets a grid component builds for, and in what order.
//!
//! The order is not cosmetic. A ring emits one loop per net, innermost first, so the order decides
//! which net gets the tightest loop; a strap set emits one stripe per net in the same order. Get
//! the order backwards and every shape is on the wrong net — geometrically perfect and electrically
//! inverted.
//!
//! Nothing here touches a database.

/// The nets a voltage domain carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Domain {
    pub power: String,
    pub ground: String,
    /// Present only where a power switch has been defined.
    pub switched_power: Option<String>,
    pub secondary: Vec<String>,
}

/// **N1** — the domain's nets, in build order.
///
/// ⚠️ **This is not a reversal, and treating it as one is wrong.** With power first the order is
/// `power, switched, ground`; with ground first it is `ground, power, switched`. Ground moves to
/// the front and the other two keep their relative order — reversing the list would put the
/// switched supply *before* the unswitched one, which is a different grid.
///
/// Secondary nets always come last, in their own order, whichever way the first three are arranged.
/// The name the core voltage domain carries — `VoltageDomain`'s own constructor sets it.
///
pub const CORE_DOMAIN: &str = "Core";

/// **N3** — a voltage domain's name as the database holds it.
///
/// 🔑 **`CORE` and `Core` are the same domain.** `pdn::modify_voltage_domain_name` rewrites the
/// conventional user spelling to the name the core domain was constructed with, and every lookup
/// goes through it — so `define_pdn_grid -voltage_domains {CORE}` resolves to the domain the grid
/// would have used anyway and asks for nothing extra.
///
/// ⚠️ **Only that one name is aliased.** Every other name is used as written, and a domain named
/// after a region keeps the region's name unless `-name` overrides it.
pub fn domain_name(user: &str) -> &str {
    if user == "CORE" {
        CORE_DOMAIN
    } else {
        user
    }
}

pub fn build_order(d: &Domain, starts_with_power: bool) -> Vec<String> {
    let mut out = Vec::new();
    if starts_with_power {
        out.push(d.power.clone());
        out.extend(d.switched_power.clone());
        out.push(d.ground.clone());
    } else {
        out.push(d.ground.clone());
        out.push(d.power.clone());
        out.extend(d.switched_power.clone());
    }
    out.extend(d.secondary.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_the_only_aliased_domain_name() {
        assert_eq!(domain_name("CORE"), "Core");
        assert_eq!(domain_name("Core"), "Core");
        assert_eq!(domain_name("TEMP_ANALOG"), "TEMP_ANALOG");
        // ⚠️ Not case-insensitive, and not a general upper-case rule.
        assert_eq!(domain_name("core"), "core");
    }

    fn domain() -> Domain {
        Domain {
            power: "VDD".into(),
            ground: "VSS".into(),
            ..Default::default()
        }
    }

    #[test]
    fn ground_first_is_what_a_plain_grid_asks_for() {
        // ⚠️ Verified against the reference, not assumed. A ring built with
        // `-power VDD -ground VSS` puts **VSS on the inner loop and VDD on the outer**, which only
        // happens if ground is the first net. Assuming power-first swaps every net in the ring and
        // looks entirely plausible in the output.
        assert_eq!(build_order(&domain(), false), vec!["VSS", "VDD"]);
    }

    #[test]
    fn power_first_puts_power_on_the_inner_loop() {
        assert_eq!(build_order(&domain(), true), vec!["VDD", "VSS"]);
    }

    #[test]
    fn a_switched_supply_is_not_placed_by_reversing_the_list() {
        // ⚠️ The whole point. Power-first is (power, switched, ground); ground-first is
        // (ground, power, switched). Reversing power-first would give (ground, switched, power),
        // putting the switched supply ahead of the unswitched one.
        let d = Domain {
            switched_power: Some("VDD_SW".into()),
            ..domain()
        };
        assert_eq!(build_order(&d, true), vec!["VDD", "VDD_SW", "VSS"]);
        assert_eq!(build_order(&d, false), vec!["VSS", "VDD", "VDD_SW"]);
    }

    #[test]
    fn secondary_nets_come_last_either_way_and_keep_their_own_order() {
        let d = Domain {
            secondary: vec!["VAUX".into(), "VIO".into()],
            ..domain()
        };
        assert_eq!(build_order(&d, true), vec!["VDD", "VSS", "VAUX", "VIO"]);
        assert_eq!(build_order(&d, false), vec!["VSS", "VDD", "VAUX", "VIO"]);
    }

    #[test]
    fn a_domain_with_no_switched_supply_leaves_no_gap() {
        let d = domain();
        assert_eq!(
            build_order(&d, true).len(),
            2,
            "no empty slot where the switch would be"
        );
    }
}
