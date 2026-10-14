// SPDX-License-Identifier: Apache-2.0
//! The command line refuses what it does not read.

use std::process::Command;

// Rule: `generate` refuses an option it does not read, naming it, with exit 2 — before opening the
// database. `--out-odb` is a `global-connect` option; it used to be dropped silently and the run
// reported success having written no database.
#[test]
fn generate_refuses_an_unknown_option() {
    let out = Command::new(env!("CARGO_BIN_EXE_vyges-pdn"))
        .args(["generate", "missing.odb", "--out-def", "x.def", "--power", "VDD", "--ground", "VSS", "--out-odb", "y.odb"])
        .output()
        .expect("run vyges-pdn");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option --out-odb"));
}

// The check has teeth only if a known option set is NOT refused for that reason.
#[test]
fn generate_accepts_its_own_options() {
    let out = Command::new(env!("CARGO_BIN_EXE_vyges-pdn"))
        .args(["generate", "missing.odb", "--out-def", "x.def", "--power", "VDD", "--ground", "VSS", "--trim", "1"])
        .output()
        .expect("run vyges-pdn");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("unknown option"));
}
