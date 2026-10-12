use crate::model::{
    CatalogEntry, CatalogReference, Config, Dependency, Diagnostic, Language, ManifestKind,
    PackageManifest, Project, Root,
};
use crate::parse;
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    entries: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    include_tests: bool,
}

pub fn discover(
    input: &Path,
    config_path: Option<&Path>,
    include_tests: bool,
) -> Result<Project, String> {
    let input = fs::canonicalize(input)
        .map_err(|error| format!("cannot read {}: {error}", input.display()))?;
    let scan_root = if input.is_file() {
        input
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("cannot determine parent of {}", input.display()))?
    } else {
        input.clone()
    };
    let root = project_root(&scan_root);
    let config = load_config(&root, config_path, include_tests)?;
    let (manifest, additional_manifests) = load_manifests(&root);

    let candidates = collect_candidates(&scan_root, &root, &config)?;
    let parsed: Vec<_> = candidates
        .par_iter()
        .filter_map(|path| {
            let source = match fs::read_to_string(path) {
                Ok(source) => source,
                Err(_) => return None,
            };
            let display = relative_display(&root, path);
            Some(parse::parse(path.clone(), display, &source))
        })
        .collect();

    let mut diagnostics = Vec::new();
    for path in &candidates {
        if fs::read_to_string(path).is_err() {
            diagnostics.push(Diagnostic {
                kind: "unreadable_file".to_string(),
                message: format!("could not read {}", relative_display(&root, path)),
            });
        }
    }
    let parse_errors = parsed.iter().filter(|file| file.parse_error).count();
    if parse_errors > 0 {
        diagnostics.push(Diagnostic {
            kind: "parse_error".to_string(),
            message: format!("{parse_errors} file(s) contain syntax errors"),
        });
    }

    Ok(Project {
        root,
        files: parsed,
        manifest,
        additional_manifests,
        config,
        diagnostics,
    })
}

pub fn roots(project: &Project) -> (Vec<Root>, Vec<Diagnostic>) {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    let mut diagnostics = Vec::new();

    for entry in &project.config.entries {
        if let Some(index) = find_entry(project, entry) {
            add_root(&mut roots, &mut seen, index, "configured entry");
        } else {
            diagnostics.push(Diagnostic {
                kind: "missing_entry".to_string(),
                message: format!("configured entry does not match a scanned file: {entry}"),
            });
        }
    }

    if let Some(manifest) = &project.manifest {
        for entry in &manifest.entries {
            if let Some(index) = find_entry(project, entry) {
                add_root(&mut roots, &mut seen, index, "package manifest entry");
            }
        }
    }

    for (index, file) in project.files.iter().enumerate() {
        if file.is_test && !project.config.include_tests {
            continue;
        }
        let path = file.display_path.as_str();
        let basename = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");

        let reason = match file.language {
            Language::Go if file.package.as_deref() == Some("main") && file.has_main => {
                Some("Go main package")
            }
            Language::Rust if file.has_main => Some("Rust main function"),
            Language::Rust
                if matches!(path, "src/lib.rs" | "src/main.rs" | "build.rs")
                    || path.starts_with("src/bin/")
                    || path.starts_with("examples/")
                    || path.starts_with("benches/") =>
            {
                Some("Rust target")
            }
            Language::TypeScript | Language::Tsx
                if matches!(basename, "index.ts" | "index.tsx" | "main.ts" | "main.tsx") =>
            {
                Some("conventional TypeScript entry")
            }
            Language::Lua if matches!(basename, "init.lua" | "main.lua") => {
                Some("conventional Lua entry")
            }
            _ => None,
        };

        if let Some(reason) = reason {
            add_root(&mut roots, &mut seen, index, reason);
        }
    }

    if roots.is_empty() {
        let available: Vec<_> = project
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| project.config.include_tests || !file.is_test)
            .map(|(index, _)| index)
            .collect();
        if available.is_empty() {
            diagnostics.push(Diagnostic {
                kind: "no_source_files".to_string(),
                message: "no source files were found".to_string(),
            });
        } else {
            for index in available {
                add_root(&mut roots, &mut seen, index, "fallback: no entry point");
            }
            diagnostics.push(Diagnostic {
                kind: "no_entrypoints".to_string(),
                message: "no entry points were inferred; treating scanned files as roots"
                    .to_string(),
            });
        }
    }

    (roots, diagnostics)
}

fn add_root(roots: &mut Vec<Root>, seen: &mut HashSet<usize>, file: usize, reason: &str) {
    if seen.insert(file) {
        roots.push(Root {
            file,
            reason: reason.to_string(),
        });
    }
}

fn find_entry(project: &Project, entry: &str) -> Option<usize> {
    let entry = entry.trim_start_matches("./").replace('\\', "/");
    let entry_path = Path::new(&entry);
    let candidates = if entry_path.extension().is_some() {
        vec![entry.clone()]
    } else {
        vec![
            format!("{entry}.ts"),
            format!("{entry}.tsx"),
            format!("{entry}.go"),
            format!("{entry}.rs"),
            format!("{entry}.lua"),
            format!("{entry}/index.ts"),
            format!("{entry}/index.tsx"),
            format!("{entry}/mod.rs"),
            format!("{entry}/init.lua"),
        ]
    };

    project
        .files
        .iter()
        .position(|file| candidates.contains(&file.display_path))
}

fn collect_candidates(
    scan_root: &Path,
    root: &Path,
    config: &Config,
) -> Result<Vec<PathBuf>, String> {
    let mut candidates = Vec::new();
    let mut walker = WalkBuilder::new(scan_root);
    walker.standard_filters(true).hidden(false);
    for result in walker.build() {
        let entry = result.map_err(|error| format!("walking {}: {error}", scan_root.display()))?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some("target" | "node_modules" | "dist" | "build" | "out" | "vendor" | "coverage")
            )
        }) {
            continue;
        }
        if crate::lang::from_path(path).is_none() {
            continue;
        }
        let display = relative_display(root, path);
        if config
            .exclude
            .iter()
            .any(|pattern| matches_exclude(&display, pattern))
        {
            continue;
        }
        candidates.push(path.to_path_buf());
    }
    candidates.sort();
    Ok(candidates)
}

fn matches_exclude(path: &str, pattern: &str) -> bool {
    let pattern = pattern.trim_start_matches("./").replace('\\', "/");
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

fn load_config(
    root: &Path,
    config_path: Option<&Path>,
    include_tests: bool,
) -> Result<Config, String> {
    let path = config_path
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("deadwood.toml"));
    let mut config = if path.exists() {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let parsed: FileConfig = toml::from_str(&text)
            .map_err(|error| format!("invalid {}: {error}", path.display()))?;
        Config {
            entries: parsed.entries,
            exclude: parsed.exclude,
            include_tests: parsed.include_tests,
        }
    } else {
        Config::default()
    };
    config.include_tests |= include_tests;
    Ok(config)
}

fn load_manifests(root: &Path) -> (Option<PackageManifest>, Vec<PackageManifest>) {
    let npm = load_package_manifest(root);
    let mut additional = Vec::new();
    if let Some(manifest) = load_cargo_manifest(root) {
        additional.push(manifest);
    }
    if let Some(manifest) = load_go_manifest(root) {
        additional.push(manifest);
    }
    if let Some(manifest) = load_lua_manifest(root) {
        additional.push(manifest);
    }
    (npm, additional)
}

fn load_package_manifest(root: &Path) -> Option<PackageManifest> {
    let path = root.join("package.json");
    let text = fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;

    let mut entries = Vec::new();
    for key in ["main", "module"] {
        if let Some(value) = value.get(key).and_then(Value::as_str) {
            entries.push(value.to_string());
        }
    }
    if let Some(bin) = value.get("bin") {
        collect_strings(bin, &mut entries, false);
    }
    if let Some(exports) = value.get("exports") {
        collect_strings(exports, &mut entries, true);
    }

    let mut dependencies = Vec::new();
    for (key, kind) in [
        ("dependencies", "runtime"),
        ("optionalDependencies", "optional"),
        ("peerDependencies", "peer"),
        ("devDependencies", "development"),
    ] {
        if let Some(values) = value.get(key).and_then(Value::as_object) {
            dependencies.extend(values.keys().map(|name| Dependency {
                name: name.clone(),
                kind,
            }));
        }
    }

    let optional_peers = value
        .get("peerDependencies")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|peers| peers.keys())
        .filter(|name| {
            value
                .get("peerDependenciesMeta")
                .and_then(Value::as_object)
                .and_then(|meta| meta.get(*name))
                .and_then(Value::as_object)
                .and_then(|entry| entry.get("optional"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    let catalog_references = catalog_references(&value);
    let (catalog_path, catalog_entries) = catalog_entries(root)
        .map(|(path, entries)| (Some(path), entries))
        .unwrap_or_else(|| (Some(path.clone()), manifest_catalog_entries(&value)));

    let binaries = value
        .get("scripts")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|scripts| scripts.values())
        .filter_map(Value::as_str)
        .flat_map(script_binaries)
        .collect();

    Some(PackageManifest {
        kind: ManifestKind::Npm,
        path,
        package_name: value
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        entries,
        dependencies,
        optional_peers,
        binaries,
        catalog_path,
        catalog_entries,
        catalog_references,
    })
}

fn catalog_references(value: &Value) -> Vec<CatalogReference> {
    let mut references = Vec::new();
    for key in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
        "resolutions",
    ] {
        let Some(entries) = value.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (name, version) in entries {
            let Some(version) = version.as_str() else {
                continue;
            };
            let Some(namespace) = version.strip_prefix("catalog:") else {
                continue;
            };
            references.push(CatalogReference {
                namespace: if namespace.is_empty() {
                    "default".to_string()
                } else {
                    namespace.to_string()
                },
                name: name.clone(),
            });
        }
    }
    references
}

fn manifest_catalog_entries(value: &Value) -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    if let Some(catalog) = value.get("catalog").and_then(Value::as_object) {
        entries.extend(catalog.keys().map(|name| CatalogEntry {
            namespace: "default".to_string(),
            name: name.clone(),
            line: 1,
        }));
    }
    if let Some(catalogs) = value.get("catalogs").and_then(Value::as_object) {
        for (namespace, catalog) in catalogs {
            if let Some(catalog) = catalog.as_object() {
                entries.extend(catalog.keys().map(|name| CatalogEntry {
                    namespace: namespace.clone(),
                    name: name.clone(),
                    line: 1,
                }));
            }
        }
    }
    entries
}

fn catalog_entries(root: &Path) -> Option<(PathBuf, Vec<CatalogEntry>)> {
    let path = [
        root.join("pnpm-workspace.yaml"),
        root.join("pnpm-workspace.yml"),
        root.join(".yarnrc.yml"),
    ]
    .into_iter()
    .find(|path| path.is_file())?;
    let text = fs::read_to_string(&path).ok()?;
    let mut entries = Vec::new();
    let mut section = None;
    let mut namespace = None;
    for (line_index, line) in text.lines().enumerate() {
        let without_comment = line.split('#').next().unwrap_or("");
        let trimmed = without_comment.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = without_comment.len() - without_comment.trim_start().len();
        if indent == 0 && trimmed == "catalog:" {
            section = Some("default");
            namespace = Some("default".to_string());
            continue;
        }
        if indent == 0 && trimmed == "catalogs:" {
            section = Some("named");
            namespace = None;
            continue;
        }
        let Some((name, _)) = trimmed.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name == "packages" {
            continue;
        }
        match (section, indent) {
            (Some("default"), 2..) => entries.push(CatalogEntry {
                namespace: "default".to_string(),
                name: name.to_string(),
                line: line_index as u32 + 1,
            }),
            (Some("named"), 2) => namespace = Some(name.to_string()),
            (Some("named"), 4..) => {
                if let Some(namespace) = &namespace {
                    entries.push(CatalogEntry {
                        namespace: namespace.clone(),
                        name: name.to_string(),
                        line: line_index as u32 + 1,
                    });
                }
            }
            _ => {}
        }
    }
    Some((path, entries))
}

fn load_cargo_manifest(root: &Path) -> Option<PackageManifest> {
    let path = root.join("Cargo.toml");
    let text = fs::read_to_string(&path).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    let mut dependencies = Vec::new();
    if let Some(table) = value.get("dependencies").and_then(toml::Value::as_table) {
        dependencies.extend(table.keys().map(|name| Dependency {
            name: name.replace('-', "_"),
            kind: "runtime",
        }));
    }
    if let Some(table) = value
        .get("build-dependencies")
        .and_then(toml::Value::as_table)
    {
        dependencies.extend(table.keys().map(|name| Dependency {
            name: name.replace('-', "_"),
            kind: "build",
        }));
    }
    if let Some(table) = value
        .get("dev-dependencies")
        .and_then(toml::Value::as_table)
    {
        dependencies.extend(table.keys().map(|name| Dependency {
            name: name.replace('-', "_"),
            kind: "development",
        }));
    }
    Some(PackageManifest {
        kind: ManifestKind::Cargo,
        path,
        package_name: value
            .get("package")
            .and_then(toml::Value::as_table)
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .map(|name| name.replace('-', "_")),
        entries: Vec::new(),
        dependencies,
        optional_peers: Vec::new(),
        binaries: Vec::new(),
        catalog_path: None,
        catalog_entries: Vec::new(),
        catalog_references: Vec::new(),
    })
}

fn load_go_manifest(root: &Path) -> Option<PackageManifest> {
    let path = root.join("go.mod");
    let text = fs::read_to_string(&path).ok()?;
    let mut dependencies = Vec::new();
    let mut in_require = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("require (") {
            in_require = true;
            continue;
        }
        if in_require && trimmed == ")" {
            in_require = false;
            continue;
        }
        let candidate = if in_require {
            trimmed
        } else if let Some(rest) = trimmed.strip_prefix("require ") {
            rest
        } else {
            continue;
        };
        let Some(name) = candidate.split_whitespace().next() else {
            continue;
        };
        if name.starts_with("//") || name == "(" {
            continue;
        }
        let kind = if candidate.contains("// indirect") {
            "indirect"
        } else {
            "runtime"
        };
        dependencies.push(Dependency {
            name: name.to_string(),
            kind,
        });
    }
    let package_name = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("module ").map(str::to_string));
    Some(PackageManifest {
        kind: ManifestKind::Go,
        path,
        package_name,
        entries: Vec::new(),
        dependencies,
        optional_peers: Vec::new(),
        binaries: Vec::new(),
        catalog_path: None,
        catalog_entries: Vec::new(),
        catalog_references: Vec::new(),
    })
}

fn load_lua_manifest(root: &Path) -> Option<PackageManifest> {
    let path = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rockspec"))?;
    let text = fs::read_to_string(&path).ok()?;
    let start = text.find("dependencies")?;
    let open = text[start..].find('{')? + start + 1;
    let close = text[open..].find('}')? + open;
    let dependencies = text[open..close]
        .split(',')
        .filter_map(|item| {
            let item = item.trim().trim_matches(|ch| ch == '\'' || ch == '"');
            let name = item.split_whitespace().next()?.to_string();
            (!name.is_empty()).then_some(Dependency {
                name,
                kind: "runtime",
            })
        })
        .collect();
    Some(PackageManifest {
        kind: ManifestKind::Lua,
        path,
        package_name: None,
        entries: Vec::new(),
        dependencies,
        optional_peers: Vec::new(),
        binaries: Vec::new(),
        catalog_path: None,
        catalog_entries: Vec::new(),
        catalog_references: Vec::new(),
    })
}

fn script_binaries(script: &str) -> Vec<String> {
    let ignored = [
        "cd", "command", "echo", "env", "exec", "export", "false", "node", "npm", "npx", "pnpm",
        "printf", "pwd", "true", "yarn",
    ];
    script
        .split(['&', '|', ';'])
        .flat_map(|command| {
            let mut words = command.split_whitespace();
            let first = words.next().unwrap_or("").trim_start_matches("./");
            if first.is_empty() || ignored.contains(&first) {
                return Vec::new();
            }
            let mut binaries = vec![first.to_string()];
            if (first == "cross-env" || first == "env-cmd")
                && let Some(binary) =
                    words.find(|token| !token.contains('=') && !token.starts_with('-'))
            {
                binaries.push(binary.trim_start_matches("./").to_string());
            }
            binaries
        })
        .collect()
}

fn collect_strings(value: &Value, output: &mut Vec<String>, only_relative: bool) {
    match value {
        Value::String(value) if !only_relative || value.starts_with('.') => {
            output.push(value.clone());
        }
        Value::Array(values) => {
            for value in values {
                collect_strings(value, output, only_relative);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_strings(value, output, only_relative);
            }
        }
        _ => {}
    }
}

fn project_root(start: &Path) -> PathBuf {
    let mut current = start;
    loop {
        if [
            "package.json",
            "Cargo.toml",
            "go.mod",
            "go.work",
            "deadwood.toml",
        ]
        .iter()
        .any(|name| current.join(name).exists())
        {
            return current.to_path_buf();
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => return start.to_path_buf(),
        }
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn package_manifest_entries_and_dependencies_are_discovered() {
        let root = std::env::temp_dir().join("deadwood-discover-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"main":"src/main.ts","dependencies":{"used":"1","old":"1"}}"#,
        )
        .unwrap();
        fs::write(root.join("src/main.ts"), "import 'used';").unwrap();
        let project = discover(&root, None, false).unwrap();
        assert_eq!(
            project.manifest.as_ref().unwrap().entries,
            vec!["src/main.ts"]
        );
        assert_eq!(project.manifest.as_ref().unwrap().dependencies.len(), 2);
        let (roots, _) = roots(&project);
        assert_eq!(roots.len(), 1);
        assert_eq!(project.files.len(), 1);
    }
}
