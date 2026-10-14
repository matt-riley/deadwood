mod common;
use common::{Fixture, has};
use deadwood_cli::{discover, model::Report, report, resolve};
fn analyze(
    f: &Fixture,
    configure: impl FnOnce(&mut deadwood_cli::model::Config),
) -> (
    deadwood_cli::model::Project,
    deadwood_cli::model::Graph,
    Report,
) {
    let p = discover::discover_with(f.path(), None, true, |c| {
        c.semantic_go = true;
        configure(c)
    })
    .unwrap();
    let g = resolve::build(&p);
    let r = report::build(&p, &g);
    assert!(r.complete, "{:?}", r.diagnostics);
    assert!(!g.semantic.is_empty());
    assert!(!g.semantic[0].declarations.is_empty());
    assert!(!g.semantic[0].references.is_empty());
    assert!(!g.semantic[0].packages.is_empty());
    (p, g, r)
}
#[test]
fn compiler_bindings_replace_go_name_counting_and_trace_real_callers() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        (
            "main.go",
            r#"package main
type A struct{}
type B struct{}
func (A) ping(){used()}
func (B) ping(){dead1()}
func used(){}
func dead1(){dead2()}
func dead2(){dead1()}
func shadowed(){}
func main(){shadowed:=func(){};shadowed(); A{}.ping(); _=B{}}
"#,
        ),
    ]);
    let (p, g, r) = analyze(&f, |_| {});
    for name in ["B.ping", "dead1", "dead2", "shadowed"] {
        assert!(has(&r, "unused_symbol", name), "{:?}", r.findings)
    }
    for name in ["A.ping", "used", "main"] {
        assert!(!has(&r, "unused_symbol", name))
    }
    let traces = deadwood_cli::output::traces(&p, &g, None, Some("A.ping"), None);
    assert_eq!(traces.len(), 1);
    assert!(
        traces[0]
            .references
            .iter()
            .any(|r| r.contains("main.go:") && r.contains("main")),
        "{traces:?}"
    );
    let json = serde_json::to_value(r).unwrap();
    assert!(json["analysis"][0]["declarations"].as_u64().unwrap() > 0);
}
#[test]
fn actual_go_build_profiles_exclude_inactive_files_and_tests() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        (
            "main.go",
            "package main\nfunc main(){alive()}\nfunc alive(){}\nfunc testOnly(){}\n",
        ),
        (
            "main_test.go",
            "package main\nimport \"testing\"\nfunc TestLive(t *testing.T){testOnly()}\nfunc helper(){dead()}\nfunc dead(){}\n",
        ),
        (
            "feature.go",
            "//go:build feature\n\npackage main\nfunc feature(){}\n",
        ),
        ("other_windows.go", "package main\nfunc windows(){}\n"),
    ]);
    let (_, _, r) = analyze(&f, |c| c.target = Some("linux/amd64".into()));
    assert!(!has(&r, "unused_symbol", "testOnly"));
    assert!(has(&r, "unused_symbol", "helper"));
    assert!(!has(&r, "unused_file", "other_windows.go"));
    assert!(!has(&r, "unused_symbol", "feature"));
    let (_, g, r) = analyze(&f, |c| {
        c.production = true;
        c.go_tags = vec!["feature".into()];
        c.target = Some("linux/amd64".into())
    });
    assert!(has(&r, "unused_symbol", "testOnly"));
    assert!(has(&r, "unused_symbol", "feature"));
    assert_eq!(g.semantic[0].profile["tests"], "false");
    assert_eq!(g.semantic[0].profile["target"], "linux/amd64");
}
#[test]
fn compiler_loading_errors_are_incomplete_and_refuse_fixes() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        (
            "main.go",
            "package main\nfunc main(){missing()}\nfunc dead(){}\n",
        ),
    ]);
    let p = discover::discover_with(f.path(), None, true, |c| c.semantic_go = true).unwrap();
    let r = report::build(&p, &resolve::build(&p));
    assert!(!r.complete);
    assert!(r.diagnostics.iter().any(|d| d.kind == "semantic_error"));
    assert!(!has(&r, "unused_symbol", "dead"));
    assert!(deadwood_cli::fix::plan(&p, &r, true).is_err());
}
fn run(f: &Fixture, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn cli_exposes_profile_and_handles_cache_changes_without_stale_semantics() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        (
            "main.go",
            "package main\nfunc main(){used()}\nfunc used(){}\n",
        ),
    ]);
    let first = run(&f, &["--semantic-go", "--cache", "--fail-on", "none"]);
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["analysis"][0]["language"], "Go");
    f.write("main.go", "package main\nfunc main(){}\nfunc used(){}\n");
    let second = run(&f, &["--semantic-go", "--cache", "--fail-on", "none"]);
    assert_eq!(second.status.code(), Some(0));
    let second: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert!(
        second["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["name"] == "used")
    );
}
#[cfg(unix)]
#[test]
fn subprocess_timeout_is_bounded_and_does_not_fall_back() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.26\n"),
        ("main.go", "package main\nfunc main(){}\n"),
        ("bin/go", "#!/bin/sh\n/bin/sleep 30\n"),
    ]);
    std::fs::set_permissions(
        f.path().join("bin/go"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let start = std::time::Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(["--semantic-go", "--go-timeout", "1", "--fail-on", "none"])
        .env("PATH", f.path().join("bin"))
        .output()
        .unwrap();
    assert!(start.elapsed().as_secs() < 5);
    assert_eq!(out.status.code(), Some(2));
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["complete"], false);
    assert!(
        r["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["message"].as_str().unwrap().contains("timed out"))
    );
}
