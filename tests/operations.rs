mod common;
use common::Fixture;
use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};
fn run(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(["--fail-on", "none", "--max-tokens", "100000"])
        .args(args)
        .output()
        .unwrap()
}
fn json(o: &std::process::Output) -> serde_json::Value {
    assert_ne!(
        o.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}
#[test]
fn cache_is_equivalent_after_source_config_manifest_and_file_changes() {
    let f = Fixture::new(&[
        ("package.json", r#"{"dependencies":{"unused":"*"}}"#),
        ("src/index.ts", "console.log(1)"),
        ("src/dead.ts", "export const dead=1;"),
    ]);
    let check = || {
        let uncached = json(&run(&f, &[]));
        let cached = json(&run(&f, &["--cache"]));
        assert_eq!(cached, uncached);
        assert!(cached["summary"]["files"].as_u64().unwrap() > 0);
    };
    check();
    check();
    let cached = std::fs::read_dir(f.path().join(".deadwood-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&cached, "corrupt cache").unwrap();
    check();
    f.write(
        "src/index.ts",
        "import{dead}from'./dead';console.log(dead);",
    );
    check();
    f.write("package.json", r#"{"dependencies":{"other":"*"}}"#);
    check();
    f.write("deadwood.toml", "include_entry_exports=true\n");
    check();
    f.write("src/new.ts", "export const added=1;");
    check();
    std::fs::remove_file(f.path().join("src/new.ts")).unwrap();
    check();
}
#[test]
fn watch_reanalyses_changed_source_and_stops_after_bounded_runs() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("src/index.ts", "console.log(1)"),
        ("src/leaf.ts", "export const used=1;"),
    ]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args([
            "--watch",
            "--watch-count",
            "2",
            "--watch-interval",
            "20",
            "--fail-on",
            "none",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let first = rx.recv_timeout(Duration::from_secs(10));
    if first.is_err() {
        let _ = child.kill();
    }
    let first: serde_json::Value = serde_json::from_str(&first.unwrap()).unwrap();
    assert_eq!(first["summary"]["unused_files"], 1);
    f.write("src/index.ts", "import{used}from'./leaf';console.log(used)");
    let second = rx.recv_timeout(Duration::from_secs(10));
    if second.is_err() {
        let _ = child.kill();
    }
    let second: serde_json::Value = serde_json::from_str(&second.unwrap()).unwrap();
    assert_eq!(second["summary"]["unused_files"], 0);
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
}
#[test]
fn custom_compilers_require_explicit_trust_and_retain_source_identity() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "deadwood.toml",
            "entries=['src/index.widget']\n[compilers]\nwidget='cat'\n",
        ),
        (
            "src/index.widget",
            "import{used}from'./leaf';console.log(used)",
        ),
        ("src/leaf.ts", "export const used=1;"),
    ]);
    let o = run(&f, &[]);
    assert_eq!(o.status.code(), Some(2));
    let d = json(&run(&f, &["--allow-compilers"]));
    assert_eq!(d["summary"]["reachable_files"], 2);
    assert_eq!(d["summary"]["unused_files"], 0);
}
#[test]
fn reporter_preprocessors_do_not_change_analysis_exit_status() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("src/index.ts", "console.log(1)"),
        ("src/dead.ts", "export const dead=1;"),
    ]);
    let o = Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(["--preprocessor", "cat", "--reporter", "cat"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(json(&o)["summary"]["unused_files"], 1);
}
