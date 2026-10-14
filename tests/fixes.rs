mod common;
use common::Fixture;
use std::process::Command;
fn run(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(["--fail-on", "none", "--max-tokens", "100000"])
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn fix_preview_does_not_write_and_apply_preserves_side_effects() {
    let f = Fixture::new(&[
        (
            "package.json",
            r#"{"dependencies":{"dead":"*"},"scripts":{"start":"node src/index.ts"}}"#,
        ),
        ("src/index.ts", "import './leaf';"),
        ("src/leaf.ts", "export const dead=sideEffect();\n"),
    ]);
    let before = std::fs::read(f.path().join("src/leaf.ts")).unwrap();
    let o = run(&f, &["--fix", "--dry-run"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let d: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(d["fixes"].as_array().unwrap().len() >= 2);
    assert_eq!(std::fs::read(f.path().join("src/leaf.ts")).unwrap(), before);
    assert!(!f.path().join(".deadwood-backup").exists());
    let o = run(&f, &["--fix"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(f.path().join("src/leaf.ts")).unwrap(),
        "const dead=sideEffect();\n"
    );
    let pkg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path().join("package.json")).unwrap()).unwrap();
    assert!(pkg["dependencies"].get("dead").is_none());
    assert!(pkg["scripts"].get("start").is_some());
    assert!(f.path().join(".deadwood-backup").is_dir());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap()["summary"]["unused_files"],
        0
    );
}
#[test]
fn removing_files_requires_opt_in_and_backups_are_byte_exact() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("src/index.ts", "console.log(1)"),
        ("src/dead.ts", "// keep exact\r\nexport const dead=1;\r\n"),
    ]);
    assert_eq!(run(&f, &["--fix"]).status.code(), Some(0));
    assert!(f.path().join("src/dead.ts").exists());
    let expected = std::fs::read(f.path().join("src/dead.ts")).unwrap();
    assert_eq!(
        run(&f, &["--fix", "--allow-remove-files"]).status.code(),
        Some(0)
    );
    assert!(!f.path().join("src/dead.ts").exists());
    let run = std::fs::read_dir(f.path().join(".deadwood-backup"))
        .unwrap()
        .last()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(run.join("src/dead.ts")).unwrap(), expected);
}
#[test]
fn incomplete_analysis_refuses_fixes_even_with_fail_on_none() {
    let f = Fixture::new(&[
        ("package.json", r#"{"dependencies":{"dead":"*"}}"#),
        ("src/index.ts", "import './missing'"),
    ]);
    let before = std::fs::read(f.path().join("package.json")).unwrap();
    let o = run(&f, &["--fix"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("refusing fixes"));
    assert_eq!(
        std::fs::read(f.path().join("package.json")).unwrap(),
        before
    );
}
#[test]
fn failed_verification_rolls_back_all_changes() {
    let f = Fixture::new(&[
        ("package.json", r#"{"dependencies":{"dead":"*"}}"#),
        ("src/index.ts", "console.log(1)"),
    ]);
    let before = std::fs::read(f.path().join("package.json")).unwrap();
    let o = run(&f, &["--fix", "--verify-command", "false"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("rolled back"));
    assert_eq!(
        std::fs::read(f.path().join("package.json")).unwrap(),
        before
    );
}
#[test]
fn named_exports_and_catalog_fixes_do_not_delete_used_bindings() {
    let f = Fixture::new(&[
        ("package.json", r#"{"dependencies":{"used":"catalog:"}}"#),
        (
            "pnpm-workspace.yaml",
            "packages: []\ncatalog:\n  used: '*'\n  dead: '*'\n",
        ),
        (
            "src/index.ts",
            "import{used}from'./leaf'; import 'used'; console.log(used)",
        ),
        ("src/leaf.ts", "const used=1,dead=2; export{used,dead};"),
    ]);
    let o = run(&f, &["--fix"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let leaf = std::fs::read_to_string(f.path().join("src/leaf.ts")).unwrap();
    assert!(leaf.contains("const used=1,dead=2;"));
    assert!(!leaf.contains("export{used,dead}"));
    let yaml = std::fs::read_to_string(f.path().join("pnpm-workspace.yaml")).unwrap();
    let doc: serde_json::Value = serde_yaml_ng::from_str(&yaml).unwrap();
    assert!(doc["catalog"].get("dead").is_none());
    assert!(doc["catalog"].get("used").is_some());
}

#[test]
fn stale_plans_and_path_escapes_are_refused() {
    use deadwood_cli::{discover, fix, report, resolve};
    let f = Fixture::new(&[
        ("package.json", r#"{"dependencies":{"dead":"*"}}"#),
        ("src/index.ts", "console.log(1)"),
    ]);
    let p = discover::discover(f.path(), None, true).unwrap();
    let r = report::build(&p, &resolve::build(&p));
    let plan = fix::plan(&p, &r, false).unwrap();
    assert!(!plan.changes.is_empty());
    f.write("src/index.ts", "import 'dead';");
    assert!(fix::apply(&plan, None).unwrap_err().contains("stale"));
    assert!(fix::plan(&p, &r, false).unwrap_err().contains("changed"));
    let p = discover::discover(f.path(), None, true).unwrap();
    let mut r = report::build(&p, &resolve::build(&p));
    r.findings.push(report::finding(
        "unused_file",
        "../outside.ts",
        1,
        "outside",
        "high",
        "test",
        vec![],
    ));
    assert!(fix::plan(&p, &r, true).unwrap_err().contains("unsafe"));
}
#[test]
fn catalogs_are_fixed_per_namespace_without_removing_used_dependencies() {
    let f = Fixture::new(&[
        (
            "package.json",
            r#"{"dependencies":{"same":"catalog:"},"catalog":{"same":"*"},"catalogs":{"other":{"same":"*"}}}"#,
        ),
        ("src/index.ts", "import 'same';"),
    ]);
    let o = run(&f, &["--fix"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let d: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path().join("package.json")).unwrap()).unwrap();
    assert!(d["dependencies"].get("same").is_some());
    assert!(d["catalog"].get("same").is_some());
    assert!(d["catalogs"]["other"].get("same").is_none());
}

#[test]
fn native_dependency_fixes_are_gated_by_explicit_verification() {
    for (manifest, content, source, code, name) in [
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n[dependencies]\ndead='1'\n",
            "src/main.rs",
            "fn main(){}",
            "dead",
        ),
        (
            "go.mod",
            "module example.test/probe\ngo 1.23\nrequire example.test/dead v1.0.0\n",
            "main.go",
            "package main\nfunc main(){}",
            "example.test/dead",
        ),
        (
            "probe.rockspec",
            "package='probe'\nversion='1.0-1'\ndependencies={'lua >= 5.1','dead >= 1.0'}\n",
            "main.lua",
            "print('main')",
            "dead",
        ),
    ] {
        let f = Fixture::new(&[(manifest, content), (source, code)]);
        let o = run(&f, &["--fix"]);
        assert_eq!(o.status.code(), Some(0));
        assert_eq!(
            std::fs::read_to_string(f.path().join(manifest)).unwrap(),
            content
        );
        let verify = if manifest == "Cargo.toml" {
            "cargo check --offline --target-dir target"
        } else if manifest == "go.mod"
            && Command::new("go")
                .arg("version")
                .output()
                .is_ok_and(|o| o.status.success())
        {
            "go test ./..."
        } else {
            "true"
        };
        let o = run(&f, &["--fix", "--verify-command", verify]);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            !std::fs::read_to_string(f.path().join(manifest))
                .unwrap()
                .contains(name)
        );
    }
}
