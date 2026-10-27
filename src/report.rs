// SPDX-License-Identifier: Apache-2.0
//! The run's message lines, as `(warning, id, text)`: the Core domain's nets when they are found
//! rather than given (PDN-101 / PDN-102), and what a macro grid's definition finds (PDN-231,
//! PDN-1051). The binary writes them through the suite logger.

/// One message: whether it is a warning, its id, its text.
pub type Line = (bool, u16, String);

/// The Core domain's net of one kind, found because none was given. `power` picks the line.
pub fn domain_net(power: bool, net: &str, domain: &str) -> Line {
    let (id, kind) = if power { (101, "power") } else { (102, "ground") };
    (false, id, format!("Using {net} as {kind} net for {domain} domain."))
}

/// One instance a macro grid's definition selected.
#[derive(Debug, Clone)]
pub struct Selected {
    pub name: String,
    /// Connected to one of the grid's supply nets.
    pub has_supply: bool,
    /// Has any terminal at all.
    pub has_terms: bool,
}

/// What defining one macro grid reports, in order.
///
/// Rules:
/// - per selected instance, one connected to no supply net is warned about (PDN-231) — but only
///   when it has a terminal that could have been connected; it still counts as a grid made;
/// - when nothing was selected (pattern, orientation and fixed filters all applied), PDN-1051.
pub fn macro_grid(grid: &str, selected: &[Selected]) -> Vec<Line> {
    let mut out: Vec<Line> = selected
        .iter()
        .filter(|s| !s.has_supply && s.has_terms)
        .map(|s| (true, 231, format!("{} is not connected to any power/ground nets.", s.name)))
        .collect();
    if selected.is_empty() {
        out.push((true, 1051, format!("No instances found for grid ({grid}).")));
    }
    out
}

/// PDN-0213: a domain net the grid left without a single shape.
pub fn no_shapes(net: &str) -> Line {
    (true, 213, format!("No shapes were created for net {net}."))
}

/// The line that opens each grid's build: its name, and for an instance grid the instance too.
pub fn inserting_grid(grid: &str, instance: &str) -> Line {
    let name = if instance.is_empty() { grid.to_string() } else { format!("{grid} - {instance}") };
    (false, 1, format!("Inserting grid: {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(name: &str, has_supply: bool, has_terms: bool) -> Selected {
        Selected { name: name.into(), has_supply, has_terms }
    }

    /// Rules (determinePowerGroundNets, define_pdn_grid_macro, InstanceGrid::isValid): the found
    /// nets name the domain; an unconnected instance with terminals is warned about, one without
    /// is silent, and neither is "no instances"; an empty selection is.
    #[test]
    fn domain_and_macro_grid_lines() {
        assert_eq!(domain_net(true, "VDD", "Core"), (false, 101, "Using VDD as power net for Core domain.".to_string()));
        assert_eq!(domain_net(false, "VSS", "Core"), (false, 102, "Using VSS as ground net for Core domain.".to_string()));
        let got = macro_grid("sram", &[sel("u1", true, true), sel("u2", false, true), sel("u3", false, false)]);
        assert_eq!(got, [(true, 231, "u2 is not connected to any power/ground nets.".to_string())]);
        assert_eq!(macro_grid("sram", &[]), [(true, 1051, "No instances found for grid (sram).".to_string())]);
        assert!(macro_grid("sram", &[sel("u3", false, false)]).is_empty());
        assert_eq!(inserting_grid("macro", "u1").2, "Inserting grid: macro - u1");
        assert_eq!(inserting_grid("stdcell", "").2, "Inserting grid: stdcell");
    }
}
