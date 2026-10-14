mod common;
use common::Fixture;
use std::process::{Command, Output};
fn cli(f: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(args)
        .output()
        .unwrap()
}
fn doc(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {:?} / {:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
fn fixture() -> Fixture {
    Fixture::new(&[
        (
            "package.json",
            r#"{"private":true,"dependencies":{"old":"*"},"devDependencies":{"vitest":"*"}}"#,
        ),
        (
            "src/index.ts",
            "import {used} from './leaf'; console.log(used);",
        ),
        (
            "src/leaf.ts",
            "export const used = 1; export const unused = 2;",
        ),
        (
            "src/a.test.ts",
            "import {test} from 'vitest'; test('a', () => {});",
        ),
    ])
}
#[test]
fn filters_and_severity_are_independent_of_confidence() {
    let f = fixture();
    let o = cli(
        &f,
        &["--include", "unused_dependency", "--fail-on", "error"],
    );
    assert_eq!(o.status.code(), Some(1));
    let d = doc(&o);
    assert_eq!(d["findings"].as_array().unwrap().len(), 1);
    assert_eq!(d["findings"][0]["name"], "old");
    f.write(
        "deadwood.toml",
        "[rules]\nunused_dependency='warn'\nunused_export='off'\n",
    );
    let o = cli(&f, &["--fail-on", "error"]);
    assert!(o.status.success());
    assert_eq!(doc(&o)["findings"][0]["severity"], "warn");
}
#[test]
fn production_excludes_test_roots_and_development_dependencies() {
    let f = fixture();
    let d = doc(&cli(&f, &["--production", "--fail-on", "none"]));
    assert_eq!(d["summary"]["files"], 2);
    assert!(
        !d["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "unused_dev_dependency")
    );
}
#[test]
fn sarif_trace_and_github_annotations_are_real_outputs() {
    let f = fixture();
    let d = doc(&cli(&f, &["--format", "sarif", "--fail-on", "none"]));
    assert_eq!(d["version"], "2.1.0");
    assert!(!d["runs"][0]["results"].as_array().unwrap().is_empty());
    let o = cli(&f, &["--trace-export", "used", "--fail-on", "none"]);
    assert!(
        doc(&o)["traces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["file"] == "src/leaf.ts")
    );
    let o = cli(&f, &["--format", "github-actions", "--fail-on", "none"]);
    assert!(
        String::from_utf8(o.stdout)
            .unwrap()
            .contains("::error file=")
    );
}
#[test]
fn missing_entries_are_incomplete_and_invalid_config_is_an_error() {
    let f = Fixture::new(&[("worker.ts", "export const worker=1;")]);
    let o = cli(&f, &[]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(doc(&o)["complete"], false);
    let o = cli(&f, &["--config", "missing.toml"]);
    assert_eq!(o.status.code(), Some(2));
    f.write("deadwood.toml", "typo=true\n");
    let o = cli(&f, &[]);
    assert_eq!(o.status.code(), Some(2));
}
#[test]
fn globs_suppressions_and_entry_exports_work() {
    let f = Fixture::new(&[
        ("package.json", r#"{"private":true}"#),
        (
            "deadwood.toml",
            "entries=['src/*.ts','!src/excluded.ts']\ninclude_entry_exports=true\n",
        ),
        (
            "src/api.ts",
            "// deadwood-ignore: unused_export\nexport const api=1;",
        ),
        ("src/excluded.ts", "export const excluded=1;"),
    ]);
    let d = doc(&cli(&f, &["--fail-on", "none"]));
    let findings = d["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f["kind"] == "unused_file" && f["name"] == "src/excluded.ts")
    );
    assert!(!findings.iter().any(|f| f["name"] == "api"));
}
#[test]
fn strict_workspace_dependencies_cannot_leak_from_parent() {
    let f = Fixture::new(&[
        (
            "package.json",
            r#"{"private":true,"workspaces":["packages/*"],"dependencies":{"shared":"*"}}"#,
        ),
        ("packages/app/package.json", r#"{"name":"app"}"#),
        ("packages/app/src/index.ts", "import 'shared';"),
    ]);
    let relaxed = doc(&cli(&f, &["--fail-on", "none"]));
    assert!(
        !relaxed["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "unlisted_dependency")
    );
    let strict = doc(&cli(&f, &["--strict", "--fail-on", "none"]));
    assert!(
        strict["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "unlisted_dependency" && f["name"] == "shared")
    );
}
#[test]
fn custom_preprocessors_are_explicit_and_validate_output() {
    let f = fixture();
    let o = cli(&f, &["--preprocessor", "printf '{}'", "--fail-on", "none"]);
    assert_eq!(o.status.code(), Some(2), "invalid report must be rejected");
    assert!(String::from_utf8_lossy(&o.stderr).contains("invalid preprocessor"));
}

#[test]
fn every_human_reporter_traces_and_telemetry_work() {
    let f = fixture();
    for format in ["text", "compact", "markdown"] {
        let o = cli(
            &f,
            &[
                "--format",
                format,
                "--trace",
                "--debug",
                "--performance",
                "--duration",
                "--memory",
                "--fail-on",
                "none",
            ],
        );
        assert!(o.status.success(), "{:?}", o);
        let text = String::from_utf8(o.stdout).unwrap();
        assert!(text.contains(if format == "markdown" {
            "| Rule |"
        } else {
            "unused_export"
        }));
        let stderr = String::from_utf8(o.stderr).unwrap();
        assert!(stderr.contains("discover="));
        assert!(stderr.contains("entry "));
        assert!(stderr.contains("duration="));
    }
    let d = doc(&cli(
        &f,
        &["--trace-file", "src/leaf.ts", "--fail-on", "none"],
    ));
    assert!(d["traces"].as_array().unwrap().iter().any(|t| {
        t["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "src/index.ts")
    }));
    let d = doc(&cli(
        &f,
        &["--trace-dependency", "vitest", "--fail-on", "none"],
    ));
    assert!(
        d["traces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "vitest" && !t["references"].as_array().unwrap().is_empty())
    );
}
#[test]
fn runtime_ecosystem_entries_survive_production_mode() {
    let f = Fixture::new(&[
        ("package.json", r#"{"dependencies":{"next":"*"}}"#),
        (
            "src/app/page.tsx",
            "export default function Page(){return <div/>}",
        ),
    ]);
    let d = doc(&cli(&f, &["--production", "--fail-on", "none"]));
    assert_eq!(d["complete"], true);
    assert_eq!(d["summary"]["roots"], 1);
}

#[test]
fn dependency_traces_respect_workspace_ownership() {
    let f = Fixture::new(&[
        ("package.json", r#"{"workspaces":["packages/*"]}"#),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"shared":"*"}}"#,
        ),
        ("packages/a/src/index.ts", "import 'shared'"),
        (
            "packages/b/package.json",
            r#"{"name":"b","dependencies":{"shared":"*"}}"#,
        ),
        ("packages/b/src/index.ts", "console.log('no shared use')"),
    ]);
    let d = doc(&cli(
        &f,
        &[
            "--strict",
            "--trace-dependency",
            "shared",
            "--fail-on",
            "none",
        ],
    ));
    let traces = d["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 2);
    assert!(
        traces
            .iter()
            .find(|t| t["file"] == "packages/b/package.json")
            .unwrap()["references"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn ecosystem_entries_are_package_relative_and_cover_supported_adapters() {
    for (plugin, path) in [
        ("next", "src/app/page.tsx"),
        ("next", "pages/view.tsx"),
        ("@remix-run/dev", "app/routes/view.tsx"),
        ("react-router", "app/routes/view.tsx"),
        ("astro", "src/pages/page.ts"),
        ("@sveltejs/kit", "src/routes/+page.ts"),
        ("nuxt", "server/index.ts"),
        ("storybook", "src/component.stories.tsx"),
    ] {
        let f = Fixture::new(&[
            ("package.json", r#"{"workspaces":["packages/*"]}"#),
            (
                "packages/web/package.json",
                &format!(r#"{{"dependencies":{{"{plugin}":"*"}}}}"#),
            ),
            (&format!("packages/web/{path}"), "export const view=1"),
            ("packages/web/src/unused.ts", "export const unused=1"),
        ]);
        let d = doc(&cli(&f, &["--fail-on", "none"]));
        assert_eq!(d["summary"]["roots"], 1, "{plugin}: {d}");
        assert_eq!(d["summary"]["unused_files"], 1);
        assert!(
            !d["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["name"] == plugin && f["kind"] == "unused_dependency"),
            "{d}"
        );
    }
    for (plugin, entry) in [
        ("neovim", "plugin/start.lua"),
        ("neovim", "after/plugin/start.lua"),
        ("love", "conf.lua"),
    ] {
        let f = Fixture::new(&[
            ("deadwood.toml", &format!("plugins=['{plugin}']")),
            (entry, "print('entry')"),
            ("dead.lua", "return 1"),
        ]);
        let d = doc(&cli(&f, &["--fail-on", "none"]));
        assert_eq!(d["summary"]["roots"], 1, "{d}");
        assert_eq!(d["summary"]["unused_files"], 1);
    }
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("vite.config.ts", "export default {}"),
        ("jest.setup.ts", "console.log('setup')"),
        ("vitest.setup.ts", "console.log('setup')"),
    ]);
    let d = doc(&cli(&f, &["--fail-on", "none"]));
    assert_eq!(d["summary"]["roots"], 3);
}

#[test]
fn configured_test_exclusion_and_workspace_traces_are_honored() {
    let f = fixture();
    f.write("deadwood.toml", "include_tests=false");
    let d = doc(&cli(&f, &["--fail-on", "none"]));
    assert_eq!(d["summary"]["roots"], 1);
    let d = doc(&cli(&f, &["--include-tests", "--fail-on", "none"]));
    assert_eq!(d["summary"]["roots"], 2);
    let f = Fixture::new(&[
        ("package.json", r#"{"workspaces":["packages/*"]}"#),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("packages/a/src/index.ts", "console.log('a')"),
        ("packages/b/package.json", r#"{"name":"b"}"#),
        ("packages/b/src/index.ts", "console.log('b')"),
    ]);
    let d = doc(&cli(
        &f,
        &["--trace", "--workspace", "a", "--fail-on", "none"],
    ));
    assert!(!d["traces"].as_array().unwrap().is_empty());
    assert!(
        d["traces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["file"].as_str().unwrap().starts_with("packages/a/"))
    );
}
