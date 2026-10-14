mod common;
use common::{Fixture, has};

#[test]
fn unreachable_consumers_do_not_keep_dependencies_or_exports_alive() {
    let f = Fixture::new(&[
        ("package.json", r#"{"dependencies":{"dead-pkg":"*"}}"#),
        (
            "src/index.ts",
            "import {live} from './live'; console.log(live);",
        ),
        (
            "src/live.ts",
            "export const live = 1; export const dead = 2;",
        ),
        (
            "src/orphan.ts",
            "import {dead} from './live'; import 'dead-pkg'; console.log(dead);",
        ),
    ]);
    let r = f.report();
    assert!(has(&r, "unused_file", "src/orphan.ts"));
    assert!(has(&r, "unused_dependency", "dead-pkg"));
    assert!(has(&r, "unused_export", "dead"));
    assert!(!has(&r, "unused_export", "live"));
}

#[test]
fn barrel_and_namespace_usage_are_member_specific() {
    for entry in [
        "import {used} from './barrel'; console.log(used);",
        "import * as ns from './barrel'; console.log(ns.used);",
    ] {
        let f = Fixture::new(&[
            ("package.json", "{}"),
            ("src/index.ts", entry),
            ("src/barrel.ts", "export * from './leaf';"),
            (
                "src/leaf.ts",
                "export const used = 1; export const unused = 2;",
            ),
        ]);
        let r = f.report();
        assert!(has(&r, "unused_export", "unused"), "{:?}", r.findings);
        assert!(!has(&r, "unused_export", "used"));
    }
}

#[test]
fn aliases_resolve_and_nested_indexes_are_not_automatic_roots() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@app/*":["src/*"]}}}"#,
        ),
        (
            "src/index.ts",
            "import {used} from '@app/leaf'; console.log(used);",
        ),
        ("src/leaf.ts", "export const used = 1;"),
        ("src/orphan/index.ts", "export const orphan = 1;"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 2);
    assert!(has(&r, "unused_file", "src/orphan/index.ts"));
    assert!(!has(&r, "unlisted_dependency", "@app/leaf"));
}

#[test]
fn workspace_dependencies_belong_to_their_package() {
    let f = Fixture::new(&[
        (
            "package.json",
            r#"{"private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "packages/app/package.json",
            r#"{"name":"app","dependencies":{"@probe/lib":"workspace:*","dead-pkg":"*"}}"#,
        ),
        (
            "packages/app/src/index.ts",
            "import {used} from '@probe/lib'; console.log(used);",
        ),
        (
            "packages/lib/package.json",
            r#"{"name":"@probe/lib","exports":"./src/api.ts"}"#,
        ),
        ("packages/lib/src/api.ts", "export const used = 1;"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 2, "{:?}", r);
    assert!(has(&r, "unused_dependency", "dead-pkg"), "{:?}", r.findings);
    assert!(!has(&r, "unlisted_dependency", "@probe/lib"));
}

#[test]
fn test_runners_provide_test_entry_points() {
    let f = Fixture::new(&[
        ("package.json", r#"{"devDependencies":{"vitest":"*"}}"#),
        ("src/index.ts", "console.log(1);"),
        ("src/helper.ts", "export const helper = 1;"),
        (
            "src/helper.test.ts",
            "import {test} from 'vitest'; import {helper} from './helper'; test('a', () => console.log(helper));",
        ),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 3);
    assert!(!has(&r, "unused_file", "src/helper.test.ts"));
    assert!(!has(&r, "unused_file", "src/helper.ts"));
}

#[test]
fn go_packages_include_siblings_and_initializers() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\n\ngo 1.23\n"),
        ("main.go", "package main\nfunc main() { helper() }\n"),
        ("helper.go", "package main\nfunc helper() {}\n"),
        ("init.go", "package main\nfunc init() {}\n"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 3);
    assert!(!has(&r, "unused_file", "helper.go"));
    assert!(!has(&r, "unused_symbol", "helper"));
    assert!(!has(&r, "unused_symbol", "init"));
}

#[test]
fn rust_nested_modules_and_build_dependencies_resolve() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n[build-dependencies]\ncc='1'\n",
        ),
        (
            "src/main.rs",
            "mod outer; fn main() { outer::inner::used(); }",
        ),
        ("src/outer.rs", "pub mod inner;"),
        ("src/outer/inner.rs", "pub fn used() {}"),
        ("build.rs", "fn main() { cc::Build::new(); }"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 4);
    assert_eq!(r.summary.unresolved, 0);
    assert!(!has(&r, "unlisted_dependency", "cc"), "{:?}", r.findings);
    assert!(!has(&r, "unlisted_dependency", "outer"), "{:?}", r.findings);
}

#[test]
fn lua_rocks_map_to_modules_and_runtime_is_not_unused() {
    let f = Fixture::new(&[
        (
            "probe-1.rockspec",
            "package='probe'\ndependencies = { 'lua >= 5.1', 'luafilesystem >= 1.8' }\n",
        ),
        (
            "main.lua",
            "local lfs = require('lfs')\nprint(lfs.currentdir())",
        ),
    ]);
    let r = f.report();
    assert!(!has(&r, "unused_dependency", "lua"));
    assert!(!has(&r, "unused_dependency", "luafilesystem"));
    assert!(!has(&r, "unlisted_dependency", "lfs"));
    assert!(!has(&r, "unresolved", "lfs"));
}

#[test]
fn source_exports_map_build_outputs_back_to_typescript() {
    let f = Fixture::new(&[
        ("package.json", r#"{"exports":{".":"./dist/api.js"}}"#),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"rootDir":"src","outDir":"dist"}}"#,
        ),
        ("src/api.ts", "export const api = 1;"),
        ("src/orphan.ts", "export const orphan = 1;"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 1);
    assert!(has(&r, "unused_file", "src/orphan.ts"));
}

#[test]
fn import_and_export_aliases_bind_the_exported_name() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "src/index.ts",
            "import {alias} from './leaf'; console.log(alias);",
        ),
        ("src/leaf.ts", "const value = 1; export {value as alias};"),
    ]);
    let r = f.report();
    assert!(!has(&r, "unused_export", "value"), "{:?}", r.findings);
    assert!(!has(&r, "unused_export", "alias"));
}
