mod common;
use common::{Fixture, has};
#[test]
fn dynamic_references_make_analysis_incomplete_and_block_destructive_fixes() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("src/index.ts", "const path='./leaf'; import(path)"),
        ("src/leaf.ts", "export const value=1;"),
    ]);
    let r = f.report();
    assert!(!r.complete);
    assert!(r.diagnostics.iter().any(|d| d.kind == "dynamic_reference"));
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg(f.path())
        .args(["--fix", "--allow-remove-files"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(f.path().join("src/leaf.ts").exists());
}
#[test]
fn literal_globs_worker_urls_and_rust_include_create_edges() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "src/index.ts",
            "import.meta.glob('./pages/*.ts'); new Worker(new URL('./worker.ts',import.meta.url));",
        ),
        ("src/pages/one.ts", "export const page=1;"),
        ("src/worker.ts", "console.log(1)"),
    ]);
    assert_eq!(f.report().summary.reachable_files, 3);
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/main.rs",
            "include!(\"functions.rs\"); fn main(){used()}",
        ),
        ("src/functions.rs", "fn used(){}"),
    ]);
    assert_eq!(f.report().summary.reachable_files, 2);
}
#[test]
fn go_local_shadowing_does_not_keep_a_package_function_alive() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.23\n"),
        (
            "main.go",
            "package main\nfunc dead(){}\nfunc main(){dead:=1;println(dead)}",
        ),
    ]);
    assert!(has(&f.report(), "unused_symbol", "dead"));
}
#[test]
fn rust_unrelated_module_references_do_not_keep_a_symbol_alive() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", "mod one;mod two;fn main(){two::run()}"),
        ("src/one.rs", "pub fn dead(){}"),
        ("src/two.rs", "fn dead(){} pub fn run(){dead()}"),
    ]);
    assert!(
        f.report()
            .findings
            .iter()
            .any(|f| f.kind == "unused_symbol" && f.file == "src/one.rs" && f.name == "dead")
    );
}
#[test]
fn type_only_cycles_are_not_runtime_cycles() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("deadwood.toml", "include=['cycle']"),
        ("src/index.ts", "import './a'"),
        ("src/a.ts", "import type{B}from'./b';export type A=B;"),
        ("src/b.ts", "import type{A}from'./a';export type B=A;"),
    ]);
    assert!(!f.report().findings.iter().any(|f| f.kind == "cycle"));
    f.write("src/a.ts", "import './b'; export const a=1;");
    f.write("src/b.ts", "import './a'; export const b=1;");
    assert!(f.report().findings.iter().any(|f| f.kind == "cycle"));
}
#[test]
fn cargo_default_features_and_go_platform_suffixes_are_respected() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n[features]\ndefault=['extra']\nextra=[]",
        ),
        (
            "src/main.rs",
            "#[cfg(feature=\"extra\")]\nuse missing::Thing; fn main(){}",
        ),
    ]);
    assert!(has(&f.report(), "unlisted_dependency", "missing"));
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.23\n"),
        ("main.go", "package main\nfunc main(){}"),
        (
            "helper_windows.go",
            "package main\nimport \"example.test/missing\"",
        ),
    ]);
    let p = deadwood_cli::discover::discover_with(f.path(), None, true, |c| {
        c.target = Some("x86_64-unknown-linux-gnu".into())
    })
    .unwrap();
    assert_eq!(p.files.len(), 1);
}

#[test]
fn class_members_and_inline_type_only_cycles_are_checked() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "src/index.ts",
            "import{C}from'./leaf';const c=new C();c.used();",
        ),
        ("src/leaf.ts", "export class C { used(){} unused(){} }"),
    ]);
    assert!(has(&f.report(), "unused_class_member", "C.unused"));
    assert!(!has(&f.report(), "unused_class_member", "C.used"));
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("deadwood.toml", "include=['cycle']"),
        ("src/index.ts", "import './a'"),
        ("src/a.ts", "import{type B}from'./b';export type A=B;"),
        ("src/b.ts", "import{type A}from'./a';export type B=A;"),
    ]);
    assert!(!f.report().findings.iter().any(|f| f.kind == "cycle"));
    f.write(
        "src/a.ts",
        "import{type B}from'./b';import './b';export const a=1;",
    );
    f.write("src/b.ts", "import './a';export const b=1;");
    assert!(f.report().findings.iter().any(|f| f.kind == "cycle"));
}

#[test]
fn implicit_test_frameworks_and_love_callbacks_are_live() {
    let f = Fixture::new(&[
        ("package.json", r#"{"devDependencies":{"vitest":"*"}}"#),
        ("src/index.ts", "console.log('app')"),
        ("src/a.test.ts", "describe('suite',()=>it('works',()=>{}))"),
    ]);
    assert!(!has(&f.report(), "unused_dev_dependency", "vitest"));
    let f = Fixture::new(&[("main.lua", "function love.draw() print('render') end")]);
    assert!(!has(&f.report(), "unused_symbol", "draw"));
}
