mod common;
use common::{Fixture, has};
use deadwood_cli::{discover, report, resolve};
#[test]
fn commonjs_exports_and_destructured_require_bindings() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "index.cjs",
            "const {used}=require('./leaf.cjs'); console.log(used);",
        ),
        (
            "leaf.cjs",
            "const used=1, unused=2; module.exports={used,unused};",
        ),
    ]);
    let r = f.report();
    assert!(has(&r, "unused_export", "unused"), "{:?}", r.findings);
    assert!(!has(&r, "unused_export", "used"));
}
#[test]
fn export_shapes_use_ast_not_whitespace() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "src/index.ts",
            "import{used,alias}from'./leaf';console.log(used,alias);",
        ),
        (
            "src/leaf.ts",
            "export const used:number=1, unused=2; const value=3; export{value as alias}; export async function dead(){}",
        ),
    ]);
    let r = f.report();
    assert!(has(&r, "unused_export", "unused"));
    assert!(has(&r, "unused_export", "dead"));
    assert!(!has(&r, "unused_export", "used"));
    assert!(!has(&r, "unused_export", "alias"));
}
#[test]
fn inherited_jsonc_aliases_resolve_relative_to_their_config() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "config/base.json",
            r#"{"compilerOptions":{"baseUrl":"..","paths":{"@app/*":["src/*"],},},}"#,
        ),
        (
            "tsconfig.json",
            "{\n// inherited paths\n\"extends\":\"./config/base.json\"\n}",
        ),
        (
            "src/index.ts",
            "import{used}from'@app/leaf'; console.log(used);",
        ),
        ("src/leaf.ts", "export const used=1;"),
    ]);
    assert_eq!(f.report().summary.reachable_files, 2);
}
#[test]
fn pnpm_catalog_references_include_children() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        (
            "pnpm-workspace.yaml",
            "packages:\n  - 'packages/*'\ncatalog:\n  used: '*'\n  unused: '*'\n",
        ),
        (
            "packages/app/package.json",
            r#"{"name":"app","dependencies":{"used":"catalog:"}}"#,
        ),
        ("packages/app/src/index.ts", "import 'used';"),
    ]);
    let r = f.report();
    assert!(!has(&r, "unused_catalog", "used"));
    assert!(has(&r, "unused_catalog", "unused"));
}
#[test]
fn cargo_workspace_members_and_custom_targets_are_entries() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers=['crates/*']\n[workspace.dependencies]\nserde='1'\n",
        ),
        (
            "crates/app/Cargo.toml",
            "[package]\nname='app'\nversion='0.1.0'\nedition='2024'\n[[bin]]\nname='app'\npath='app.rs'\n[dependencies]\nserde={workspace=true}\n",
        ),
        (
            "crates/app/app.rs",
            "fn main(){ let _=serde::de::IgnoredAny; }",
        ),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 1);
    assert!(!has(&r, "unused_dependency", "serde"));
    assert!(!has(&r, "unlisted_dependency", "serde"));
}
#[test]
fn go_work_and_replace_resolve_local_modules() {
    let f = Fixture::new(&[
        ("go.work", "go 1.23\nuse (\n ./app\n ./lib\n)\n"),
        (
            "app/go.mod",
            "module example.test/app\ngo 1.23\nrequire example.test/lib v0.0.0\nreplace example.test/lib => ../lib\n",
        ),
        (
            "app/main.go",
            "package main\nimport \"example.test/lib\"\nfunc main(){lib.Used()}",
        ),
        ("lib/go.mod", "module example.test/lib\ngo 1.23\n"),
        ("lib/lib.go", "package lib\nfunc Used(){}"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 2);
    assert!(!has(&r, "unused_dependency", "example.test/lib"));
}
#[test]
fn rust_path_attributes_and_pub_uses_preserve_public_api() {
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/lib.rs",
            "#[path=\"other/api.rs\"]\nmod api;\npub use api::Api;",
        ),
        ("src/other/api.rs", "pub struct Api;"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 2);
    assert!(!has(&r, "unused_symbol", "Api"));
    assert!(!has(&r, "unlisted_dependency", "api"));
}
#[test]
fn build_tags_and_cfg_remove_disabled_imports() {
    let f = Fixture::new(&[
        ("go.mod", "module example.test/probe\ngo 1.23\n"),
        ("main.go", "package main\nfunc main(){}"),
        (
            "optional.go",
            "//go:build extra\n\npackage main\nimport \"example.test/missing\"",
        ),
    ]);
    let p = discover::discover(f.path(), None, true).unwrap();
    assert_eq!(p.files.len(), 1);
    let p =
        discover::discover_with(f.path(), None, true, |c| c.go_tags.push("extra".into())).unwrap();
    assert_eq!(p.files.len(), 2);
    let f = Fixture::new(&[
        (
            "Cargo.toml",
            "[package]\nname='probe'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/main.rs",
            "#[cfg(feature=\"extra\")]\nuse missing::Thing;\nfn main(){}",
        ),
    ]);
    assert!(!has(&f.report(), "unlisted_dependency", "missing"));
    let p =
        discover::discover_with(f.path(), None, true, |c| c.features.push("extra".into())).unwrap();
    assert!(has(
        &report::build(&p, &resolve::build(&p)),
        "unlisted_dependency",
        "missing"
    ));
}
#[test]
fn lua_module_tables_and_dofile_paths_resolve() {
    let f = Fixture::new(&[
        (
            "main.lua",
            "local m=require('module'); m.used(); dofile('scripts/setup.lua')",
        ),
        (
            "module.lua",
            "local M={}\nfunction M.used() return 1 end\nfunction M.unused() return 2 end\nreturn M",
        ),
        ("scripts/setup.lua", "print('setup')"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 3);
    assert!(!has(&r, "unused_symbol", "used"));
    assert!(has(&r, "unused_symbol", "unused"));
}
#[test]
fn malformed_tsconfig_fails_instead_of_disabling_resolution() {
    let f = Fixture::new(&[
        ("package.json", "{}"),
        ("tsconfig.json", "{not json"),
        ("src/index.ts", "console.log(1)"),
    ]);
    assert!(discover::discover(f.path(), None, true).is_err());
}

#[test]
fn package_import_conditions_and_workspace_export_subpaths_resolve() {
    let f = Fixture::new(&[
        (
            "package.json",
            r##"{"imports":{"#direct":"./src/direct.ts","#branch":{"node":"./src/node.ts","default":"./src/default.ts"},"#item/*":"./src/parts/*.ts"},"workspaces":["packages/*"]}"##,
        ),
        (
            "src/index.ts",
            "import '#direct';import '#branch';import '#item/name';import {util} from '@local/lib/util';import {thing}from '@local/lib/thing';util();thing();",
        ),
        ("src/direct.ts", "console.log('direct')"),
        ("src/node.ts", "console.log('node')"),
        ("src/default.ts", "console.log('default')"),
        ("src/parts/name.ts", "console.log('part')"),
        (
            "packages/lib/package.json",
            r#"{"private":true,"name":"@local/lib","exports":{"./util":{"import":"./dist/util.js"},"./*":"./dist/*.js"}}"#,
        ),
        (
            "packages/lib/tsconfig.json",
            r#"{"compilerOptions":{"rootDir":"src","outDir":"dist"}}"#,
        ),
        ("packages/lib/src/util.ts", "export function util(){}"),
        ("packages/lib/src/thing.ts", "export function thing(){}"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 7, "{r:?}");
    assert_eq!(r.summary.unresolved, 0);
    assert!(!has(&r, "unused_export", "util"));
    assert!(!has(&r, "unused_export", "thing"));
}
#[test]
fn configured_global_and_workspace_aliases_resolve_and_missing_ones_are_unresolved() {
    let f = Fixture::new(&[
        ("package.json", r#"{"workspaces":["packages/*"]}"#),
        (
            "deadwood.toml",
            "[paths]\n'@global/*'=['shared/*']\n[workspaces.'packages/app'.paths]\n'@local/*'=['src/*']\n",
        ),
        ("shared/global.ts", "export function global(){}"),
        ("packages/app/package.json", "{}"),
        (
            "packages/app/src/index.ts",
            "import{global}from'@global/global';import{local}from'@local/local';global();local();import'@local/missing'",
        ),
        ("packages/app/src/local.ts", "export function local(){}"),
    ]);
    let r = f.report();
    assert_eq!(r.summary.reachable_files, 3);
    assert!(has(&r, "unresolved", "@local/missing"));
    assert!(!has(&r, "unlisted_dependency", "@local"));
}
