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

/// The nets left with no special wire once the grid is written, as the reference warns of them
/// (PDN-0213): it makes a special wire for EVERY net of every domain before any grid writes, then
/// destroys and warns of each one still empty. The walk is over a map keyed by net, so the
/// warnings come in NET-ID order, each net once — not in domain or build order. `id` is a net's
/// database id; a net the database does not hold is never warned of (nothing was made for it).
pub fn unwritten_nets<'a>(domains: &[&'a Domain], written: &dyn Fn(&str) -> bool, id: &dyn Fn(&str) -> Option<u32>) -> Vec<&'a str> {
    let mut nets: Vec<(u32, &str)> = Vec::new();
    for d in domains {
        let all = std::iter::once(&d.power).chain(&d.switched_power).chain(std::iter::once(&d.ground)).chain(&d.secondary);
        for n in all {
            if let Some(i) = id(n) {
                if !nets.iter().any(|&(j, _)| j == i) {
                    nets.push((i, n));
                }
            }
        }
    }
    nets.sort_by_key(|&(i, _)| i);
    nets.into_iter().map(|(_, n)| n).filter(|n| !written(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwritten_nets_are_warned_of_once_each_in_net_id_order() {
        let core = Domain { power: "VDD".into(), ground: "VSS".into(), switched_power: None, secondary: vec!["VCC".into(), "VAUX".into()] };
        let region = Domain { power: "VREG".into(), ground: "VSS".into(), switched_power: Some("VSW".into()), secondary: vec![] };
        let id = |n: &str| match n { "VSS" => Some(1), "VDD" => Some(2), "VCC" => Some(7), "VAUX" => Some(3), "VREG" => Some(5), "VSW" => Some(4), _ => None };
        let written = |n: &str| n == "VDD" || n == "VSS" || n == "VREG";
        // VSS is in both domains and written: never warned of. The rest by id, not domain order.
        assert_eq!(unwritten_nets(&[&core, &region], &written, &id), vec!["VAUX", "VSW", "VCC"]);
        // A net the database does not hold made no special wire, so there is nothing to warn of.
        let ghost = Domain { secondary: vec!["NOPE".into()], ..core.clone() };
        assert_eq!(unwritten_nets(&[&ghost], &written, &id), Vec::<&str>::new());
    }

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
