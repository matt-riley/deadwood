mod common;
use common::{Fixture, has};
#[test]
fn nested_grouped_uses_aliases_and_standard_types_resolve() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/main.rs",
            "use std::{collections::{HashMap,HashSet},fs,io::Write};\nmod one; mod two; use crate::{one::{a as alias,b},two::used};\nfn main(){alias();b();used();let _=char::is_alphabetic('a');let _=fs::read(\"x\");}",
        ),
        ("src/one.rs", "pub fn a(){} pub fn b(){}"),
        ("src/two.rs", "pub fn used(){}"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 3);
    assert_eq!(r.summary.unresolved, 0, "{:?}", r.findings);
    assert!(
        !r.findings.iter().any(|f| f.kind == "unlisted_dependency"),
        "{:?}",
        r.findings
    );
    assert!(!has(&r, "unused_symbol", "a"));
    assert!(!has(&r, "unused_symbol", "b"));
}

#[test]
fn test_harnesses_and_trait_contracts_are_not_dead_symbols() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/lib.rs",
            "pub struct C;impl Drop for C {fn drop(&mut self){}}",
        ),
        (
            "tests/check.rs",
            "mod common;#[test] fn works(){common::check()}",
        ),
        ("tests/common/mod.rs", "pub fn check(){}"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.unresolved, 0);
    assert!(!has(&r, "unused_symbol", "works"));
    assert!(!has(&r, "unused_symbol", "drop"));
    assert!(!has(&r, "unused_symbol", "check"));
}
#[test]
fn missing_rust_modules_are_unresolved_even_when_the_name_is_declared() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", "mod missing;fn main(){}"),
    ]);
    assert!(has(&f.report(), "unresolved", "missing"));
}

#[test]
fn custom_library_targets_preserve_transitive_public_modules() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n[lib]\npath='lib/api.rs'\n",
        ),
        (
            "lib/api.rs",
            "pub mod one;mod private;use crate::one::public;pub fn api(){public()}",
        ),
        ("lib/one.rs", "pub mod child;pub fn public(){}"),
        ("lib/one/child.rs", "pub fn public_child(){}"),
        ("lib/private.rs", "pub fn private_api(){}"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 4);
    assert_eq!(r.summary.unresolved, 0);
    assert!(!has(&r, "unused_symbol", "api"));
    assert!(!has(&r, "unused_symbol", "public"));
    assert!(!has(&r, "unused_symbol", "public_child"));
    assert!(has(&r, "unused_symbol", "private_api"));
}
#[test]
fn cached_test_configuration_does_not_leak_into_production() {
    use std::process::Command;
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/lib.rs",
            "#[cfg(test)]mod tests{use missing_test_dependency::run;#[test]fn test(){run()}}",
        ),
    ]);
    let run = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_deadwood"))
            .arg(f.path())
            .args(["--cache", "--fail-on", "none"])
            .args(args)
            .output()
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap()
    };
    let d = run(&[]);
    assert!(
        d["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "missing_test_dependency")
    );
    let d = run(&["--production"]);
    assert!(d["findings"].as_array().unwrap().is_empty(), "{d}");
}

#[test]
fn self_and_repeated_super_use_module_directories() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", "mod one;mod two;fn main(){one::run()}"),
        (
            "src/one.rs",
            "use self::child::run as child_run;use super::two::used;pub mod child;pub fn run(){used();child_run()}",
        ),
        (
            "src/one/child.rs",
            "use super::super::two::used;pub fn run(){used()}",
        ),
        ("src/two.rs", "pub fn used(){}pub fn dead(){}"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 4);
    assert_eq!(r.summary.unresolved, 0, "{r:?}");
    assert!(has(&r, "unused_symbol", "dead"));
    assert!(!has(&r, "unused_symbol", "used"));
}
