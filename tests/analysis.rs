use deadwood_cli::{discover, report, resolve};
use std::fs;
use std::path::Path;

fn fixture(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("deadwood-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(path.join("src")).unwrap();
    path
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn reports_unreachable_typescript_files_and_dependencies() {
    let root = fixture("typescript");
    write(
        &root.join("package.json"),
        r#"{"main":"src/index.ts","dependencies":{"used-pkg":"1","old-pkg":"1"}}"#,
    );
    write(
        &root.join("src/index.ts"),
        "import './used';\nimport 'used-pkg';\n",
    );
    write(
        &root.join("src/used.ts"),
        "export const used = 1;\nexport const unused = 2;\n",
    );
    write(&root.join("src/old.ts"), "export const old = 1;\n");

    let project = discover::discover(&root, None, false).unwrap();
    let graph = resolve::build(&project);
    let report = report::build(&project, &graph);

    assert_eq!(report.summary.files, 3);
    assert_eq!(report.summary.reachable_files, 2);
    assert_eq!(report.summary.unused_files, 1);
    assert_eq!(report.summary.unused_dependencies, 1);
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unused_file" && finding.file == "src/old.ts" })
    );
    assert!(report.findings.iter().any(|finding| {
        finding.kind == "unused_export" && finding.file == "src/used.ts" && finding.name == "unused"
    }));
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unused_dependency" && finding.name == "old-pkg" })
    );
}

#[test]
fn reports_typescript_knip_categories() {
    let root = fixture("typescript-parity");
    write(
        &root.join("package.json"),
        r#"{"main":"src/index.ts","scripts":{"lint":"eslint src"},"dependencies":{"used-pkg":"1","old-pkg":"1"},"devDependencies":{"dev-unused":"1","typescript":"1"}}"#,
    );
    write(
        &root.join("src/index.ts"),
        "import { used } from './used';\nimport { E, N } from './types';\nimport type { UsedType } from './types';\nimport missing from 'missing';\nimport './missing';\nimport 'used-pkg';\nexport type PublicType = UsedType;\nconsole.log(used, E.A, N.used, missing);\n",
    );
    write(
        &root.join("src/types.ts"),
        "export enum E { A, B, C }\nexport type UsedType = string;\nexport type UnusedType = number;\nexport namespace N { export const used = 1; export const unused = 2; }\n",
    );
    write(
        &root.join("src/used.ts"),
        "export const used = 1;\nexport const unusedExport = 2;\n",
    );

    let project = discover::discover(&root, None, false).unwrap();
    let graph = resolve::build(&project);
    let report = report::build(&project, &graph);
    let has = |kind: &str, name: &str| {
        report
            .findings
            .iter()
            .any(|finding| finding.kind == kind && finding.name == name)
    };

    assert!(has("unlisted_binary", "eslint"));
    assert!(has("unlisted_dependency", "missing"));
    assert!(has("unresolved", "./missing"));
    assert!(has("unused_dev_dependency", "dev-unused"));
    assert!(has("unused_type", "UnusedType"));
    assert!(has("unused_enum_member", "E.B"));
    assert!(has("unused_namespace_member", "N.unused"));
    assert!(has("unused_export", "unusedExport"));
    assert!(!has("unused_dev_dependency", "typescript"));
}

#[test]
fn json_budget_keeps_output_parseable_and_bounded() {
    let root = fixture("budget");
    write(&root.join("src/index.ts"), "export * from './old';\n");
    write(&root.join("src/old.ts"), "export const old = 1;\n");
    write(&root.join("src/another.ts"), "export const another = 1;\n");

    let project = discover::discover(&root, None, false).unwrap();
    let graph = resolve::build(&project);
    let report = report::build(&project, &graph);
    let output = report::render_json(report, 40);

    serde_json::from_str::<serde_json::Value>(&output).unwrap();
    assert!(output.len().div_ceil(4) <= 40, "{output}");
}

#[test]
fn resolves_go_local_packages() {
    let root = fixture("go");
    write(
        &root.join("go.mod"),
        "module example.test/app\n\ngo 1.23\n\nrequire (\n\texample.com/used v1.0.0\n\texample.com/old v1.0.0\n)\n",
    );
    write(
        &root.join("main.go"),
        "package main\nimport (\n    usedpkg \"example.test/app/internal/used\"\n    \"example.com/missing\"\n)\nfunc main() { usedpkg.Value(); _ = missing.Name }\n",
    );
    write(
        &root.join("internal/used/used.go"),
        "package used\nfunc Value() int { return 1 }\nfunc Dead() int { return 0 }\n",
    );
    write(
        &root.join("internal/dead/dead.go"),
        "package used\nfunc Dead() int { return 0 }\n",
    );

    let project = discover::discover(&root, None, false).unwrap();
    let graph = resolve::build(&project);
    let report = report::build(&project, &graph);

    assert_eq!(report.summary.reachable_files, 2);
    assert!(report.findings.iter().any(|finding| {
        finding.kind == "unused_file" && finding.file == "internal/dead/dead.go"
    }));
    assert!(report.findings.iter().any(|finding| {
        finding.kind == "unused_dependency" && finding.name == "example.com/old"
    }));
    assert!(report.findings.iter().any(|finding| {
        finding.kind == "unlisted_dependency" && finding.name == "example.com/missing"
    }));
    assert!(report.findings.iter().any(|finding| {
        finding.kind == "unused_symbol"
            && finding.file == "internal/used/used.go"
            && finding.name == "Dead"
    }));
}

#[test]
fn resolves_rust_modules_and_lua_requires() {
    let rust = fixture("rust");
    write(
        &rust.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\nold = \"1\"\n",
    );
    write(
        &rust.join("src/lib.rs"),
        "use serde::Serialize;\nuse missing::Thing;\nmod used;\n",
    );
    write(&rust.join("src/used.rs"), "pub fn value() -> i32 { 1 }\n");
    write(&rust.join("src/dead.rs"), "pub fn dead() -> i32 { 0 }\n");
    let project = discover::discover(&rust, None, false).unwrap();
    let graph = resolve::build(&project);
    let report = report::build(&project, &graph);
    assert_eq!(report.summary.reachable_files, 2);
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unused_file" && finding.file == "src/dead.rs" })
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unused_dependency" && finding.name == "old" })
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unlisted_dependency" && finding.name == "missing" })
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unlisted_dependency" && finding.name == "missing" })
    );
    assert!(report.findings.iter().any(|finding| {
        finding.kind == "unused_symbol" && finding.file == "src/used.rs" && finding.name == "value"
    }));

    let lua = fixture("lua");
    write(
        &lua.join("fixture-1.rockspec"),
        "package = \"fixture\"\ndependencies = { \"used >= 1\", \"old >= 1\" }\n",
    );
    write(
        &lua.join("main.lua"),
        "require('used')\nrequire('missing')\n",
    );
    write(
        &lua.join("used.lua"),
        "local function dead() return 0 end\nreturn {}\n",
    );
    write(&lua.join("dead.lua"), "return {}\n");
    let project = discover::discover(&lua, None, false).unwrap();
    let graph = resolve::build(&project);
    let report = report::build(&project, &graph);
    assert_eq!(report.summary.reachable_files, 2);
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unused_file" && finding.file == "dead.lua" })
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| { finding.kind == "unused_dependency" && finding.name == "old" })
    );
    assert!(report.findings.iter().any(|finding| {
        finding.kind == "unused_symbol" && finding.file == "used.lua" && finding.name == "dead"
    }));
}
