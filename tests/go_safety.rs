mod common;
use common::Fixture;
fn run(f: &Fixture, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn cold_offline_adapter_failure_is_explicit_and_downloads_require_opt_in() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        ("main.go", "package main\nfunc main(){}\n"),
    ]);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(["--semantic-go", "--fail-on", "none"])
        .env("GOMODCACHE", f.path().join("empty-modules"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["complete"], false);
    assert_eq!(r["analysis"][0]["complete"], false);
    assert!(
        r["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["message"].as_str().unwrap().contains("--go-downloads"))
    );
    assert_eq!(run(&f, &["--go-downloads"]).status.code(), Some(2));
}
#[test]
fn compiler_profiles_override_inactive_parser_errors_and_workspace_api_policy() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        (
            "lib.go",
            "package probe\nfunc Public(){live()}\nfunc live(){}\n",
        ),
        (
            "inactive.go",
            "//go:build never_selected\n\npackage probe\nfunc !!! broken\n",
        ),
        (
            "deadwood.toml",
            "[workspaces.'.']\ninclude_entry_exports=true\n",
        ),
    ]);
    let out = run(&f, &["--semantic-go", "--fail-on", "none"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["complete"], true);
    assert_eq!(r["summary"]["parse_errors"], 0);
    for name in ["Public", "live"] {
        assert!(
            r["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x["kind"] == "unused_symbol" && x["name"] == name)
        );
    }
}
#[test]
fn local_replace_dependencies_and_embedded_sources_are_reachable() {
    let f = Fixture::new(&[
        (
            "go.mod",
            "module example.test/app\ngo 1.26\nrequire example.test/lib v0.0.0\nreplace example.test/lib => ./lib\n",
        ),
        (
            "main.go",
            "package main\nimport (_ \"embed\"; _ \"example.test/lib\")\n//go:embed asset.lua\nvar asset string\nfunc main(){_=asset}\n",
        ),
        ("asset.lua", "return {}"),
        ("lib/go.mod", "module example.test/lib\ngo 1.26\n"),
        (
            "lib/lib.go",
            "package lib\nfunc init(){used()}\nfunc used(){}\nfunc dead(){}\n",
        ),
    ]);
    let out = run(&f, &["--semantic-go", "--fail-on", "none"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        !r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["kind"] == "unused_file" || x["name"] == "used")
    );
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["name"] == "dead")
    );
}
#[test]
fn go_file_removal_needs_verification_and_native_symbols_are_not_deleted() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        ("main.go", "package main\nfunc main(){}\nfunc dead(){}\n"),
        ("unused/unused.go", "package unused\nfunc dead(){}\n"),
    ]);
    let original = std::fs::read(f.path().join("main.go")).unwrap();
    let out = run(&f, &["--semantic-go", "--fix", "--allow-remove-files"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--verify-command"));
    assert!(f.path().join("unused/unused.go").exists());
    let out = run(
        &f,
        &[
            "--semantic-go",
            "--fix",
            "--allow-remove-files",
            "--verify-command",
            "go test ./...",
            "--fail-on",
            "none",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!f.path().join("unused/unused.go").exists());
    assert_eq!(std::fs::read(f.path().join("main.go")).unwrap(), original);
}
#[test]
fn source_assets_consumed_by_other_languages_are_not_deleted() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/main.rs",
            "fn main(){let _=include_str!(\"../assets/data.go\");}",
        ),
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        ("main.go", "package main\nfunc main(){}"),
        ("assets/data.go", "package data\nfunc payload(){}"),
    ]);
    let out = run(&f, &["--semantic-go", "--fail-on", "none"]);
    assert_eq!(out.status.code(), Some(0));
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        !r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["file"] == "assets/data.go"),
        "{r}"
    );
}
#[cfg(unix)]
#[test]
fn exited_driver_with_inherited_pipes_still_times_out() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        ("main.go", "package main\nfunc main(){}\n"),
        ("bin/go", "#!/bin/sh\n/bin/sleep 30 &\nexit 0\n"),
    ]);
    std::fs::set_permissions(
        f.path().join("bin/go"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let start = std::time::Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(["--semantic-go", "--go-timeout", "1"])
        .env("PATH", f.path().join("bin"))
        .output()
        .unwrap();
    assert!(start.elapsed().as_secs() < 5);
    assert_eq!(out.status.code(), Some(2));
}
