use crate::model::{
    CatalogEntry, CatalogReference, Config, Dependency, Diagnostic, Language, ManifestKind,
    PackageManifest, Project, Root,
};
use globset::GlobBuilder;
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub fn discover(
    input: &Path,
    config_path: Option<&Path>,
    include_tests: bool,
) -> Result<Project, String> {
    discover_with(input, config_path, include_tests, |_| {})
}

pub fn discover_with(
    input: &Path,
    config_path: Option<&Path>,
    include_tests: bool,
    configure: impl FnOnce(&mut Config),
) -> Result<Project, String> {
    let input =
        fs::canonicalize(input).map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let scan_root = if input.is_file() {
        input.parent().unwrap().to_path_buf()
    } else {
        input
    };
    let root = project_root(&scan_root);
    let mut config = load_config(&root, config_path, include_tests)?;
    configure(&mut config);
    config.production |= config.strict;
    validate_config(&config)?;
    let stamp = crate::cache::snapshot_with(&root, config.cache.as_deref());
    let (manifest, additional_manifests) = load_manifests(&root, &config)?;
    let candidates = collect_candidates(&scan_root, &root, &config)?;
    let results: Vec<_> = candidates
        .par_iter()
        .map(|path| {
            let source =
                fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let display = relative_display(&root, path);
            let mut parse_config = config.clone();
            if crate::lang::from_path(path) == Some(Language::Rust)
                && let Some(manifest) = manifest
                    .iter()
                    .chain(&additional_manifests)
                    .filter(|m| {
                        m.kind == ManifestKind::Cargo
                            && path.starts_with(m.path.parent().unwrap_or(&root))
                    })
                    .max_by_key(|m| m.path.components().count())
                && let Ok(value) = toml::from_str::<toml::Value>(&manifest.source)
                && let Some(features) = value.get("features").and_then(toml::Value::as_table)
            {
                let mut enabled = config.features.clone();
                if !config.no_default_features {
                    enabled.push("default".into());
                }
                if config.all_features {
                    enabled.extend(features.keys().cloned());
                }
                let mut index = 0;
                while index < enabled.len() {
                    if let Some(values) = features
                        .get(&enabled[index])
                        .and_then(toml::Value::as_array)
                    {
                        for feature in values.iter().filter_map(toml::Value::as_str) {
                            if !enabled.iter().any(|f| f == feature) {
                                enabled.push(feature.into());
                            }
                        }
                    }
                    index += 1;
                }
                parse_config.features = enabled;
            }
            if let Some(command) = path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|e| config.compilers.get(e))
            {
                let compiled = crate::output::command(command, &source, &root)?;
                let mut record = crate::cache::parse(
                    path,
                    format!("{display}.ts"),
                    &compiled,
                    config.cache.as_deref(),
                    &parse_config,
                );
                record.display_path = display;
                record.source = source;
                record.compiled = true;
                Ok(record)
            } else {
                Ok(crate::cache::parse(
                    path,
                    display,
                    &source,
                    config.cache.as_deref(),
                    &parse_config,
                ))
            }
        })
        .collect::<Vec<Result<_, String>>>();
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    for result in results {
        match result {
            Ok(file) => files.push(file),
            Err(message) => diagnostics.push(Diagnostic {
                kind: "unreadable_file".into(),
                message,
            }),
        }
    }
    files.retain(|f| {
        (!config.production || !f.is_test)
            && selected(&f.display_path, &config.project)
            && native_file_enabled(f, &config)
    });
    for file in &files {
        for message in &file.dynamic_references {
            diagnostics.push(Diagnostic {
                kind: "dynamic_reference".into(),
                message: format!("{}: {message}", file.display_path),
            });
        }
        if file.parse_error {
            diagnostics.push(Diagnostic {
                kind: "parse_error".into(),
                message: format!("{} contains syntax errors", file.display_path),
            });
        }
    }
    let mut project = Project {
        stamp,
        root,
        files,
        manifest,
        additional_manifests,
        config,
        diagnostics,
    };
    project.files = std::mem::take(&mut project.files)
        .into_iter()
        .filter(|f| {
            let mut scopes: Vec<_> = project
                .config
                .workspaces
                .iter()
                .filter(|(pattern, _)| {
                    let dir = Path::new(&f.display_path).parent().unwrap_or(Path::new(""));
                    dir.ancestors()
                        .any(|p| matches_pattern(&p.to_string_lossy(), pattern))
                })
                .collect();
            scopes.sort_by_key(|(pattern, _)| pattern.len());
            scopes.last().is_none_or(|(pattern, cfg)| {
                let dir = project
                    .manifests()
                    .filter_map(|m| m.path.parent())
                    .filter_map(|p| p.strip_prefix(&project.root).ok())
                    .find(|p| matches_pattern(&p.to_string_lossy(), pattern));
                let relative = dir
                    .and_then(|dir| Path::new(&f.display_path).strip_prefix(dir).ok())
                    .unwrap_or(Path::new(&f.display_path))
                    .to_string_lossy();
                selected(&relative, &cfg.project)
                    && !cfg.exclude.iter().any(|p| matches_pattern(&relative, p))
            })
        })
        .collect();
    Ok(project)
}

pub fn roots(project: &Project) -> (Vec<Root>, Vec<Diagnostic>) {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    let mut diagnostics = Vec::new();
    for entry in &project.config.entries {
        let matches = entry_matches(project, "", entry);
        if matches.is_empty() && !entry.starts_with('!') {
            diagnostics.push(Diagnostic {
                kind: "missing_entry".into(),
                message: format!("configured entry matched no scanned file: {entry}"),
            });
        }
        for index in matches {
            add_root(&mut roots, &mut seen, index, "configured entry");
        }
    }
    for manifest in project.manifests() {
        let dir = manifest.path.parent().unwrap_or(&project.root);
        let relative = dir
            .strip_prefix(&project.root)
            .unwrap_or(dir)
            .to_string_lossy()
            .replace('\\', "/");
        let ws_name = if relative.is_empty() { "." } else { &relative };
        let workspace = project
            .config
            .workspaces
            .iter()
            .filter(|(p, _)| matches_pattern(ws_name, p))
            .max_by_key(|(p, _)| p.len())
            .map(|(_, c)| c);
        let entries = workspace
            .filter(|c| !c.entries.is_empty())
            .map(|c| &c.entries)
            .unwrap_or(&manifest.entries);
        for entry in entries {
            for index in entry_matches(project, &relative, entry) {
                add_root(&mut roots, &mut seen, index, "package entry");
            }
        }
        if !project.config.production {
            for entry in &manifest.development_entries {
                for index in entry_matches(project, &relative, entry) {
                    add_root(&mut roots, &mut seen, index, "tool/script entry");
                }
            }
        }
    }
    for (index, file) in project.files.iter().enumerate() {
        if file.is_test {
            if project.config.include_tests && !project.config.production {
                add_root(&mut roots, &mut seen, index, "test entry");
            }
            continue;
        }
        let dir = project
            .owner(index)
            .and_then(|m| m.path.parent())
            .unwrap_or(&project.root);
        let path = file
            .path
            .strip_prefix(dir)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        let configured = !project.config.entries.is_empty()
            || project
                .workspace(index)
                .is_some_and(|w| !w.entries.is_empty());
        let reason = conventional_entry(file, &path, configured);
        if let Some(reason) = reason {
            add_root(&mut roots, &mut seen, index, reason);
        }
        if tool_entry(project, index, &path) {
            add_root(&mut roots, &mut seen, index, "ecosystem entry");
        }
    }
    go_library_roots(project, &mut roots, &mut seen);
    if roots.is_empty() {
        diagnostics.push(Diagnostic {
            kind: if project.files.is_empty() {
                "no_source_files"
            } else {
                "no_entrypoints"
            }
            .into(),
            message: "no entry points found; configure entries before trusting the analysis".into(),
        });
        for (index, file) in project
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| project.config.include_tests || !f.is_test)
        {
            let _ = file;
            add_root(
                &mut roots,
                &mut seen,
                index,
                "fallback: incomplete entry discovery",
            );
        }
    }
    roots.retain(|r| {
        !project
            .config
            .entries
            .iter()
            .filter_map(|p| p.strip_prefix('!'))
            .any(|p| matches_pattern(&project.files[r.file].display_path, p))
    });
    (roots, diagnostics)
}

fn tool_entry(project: &Project, index: usize, path: &str) -> bool {
    let basename = Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    if basename.contains(".config.")
        || basename.starts_with("jest.")
        || basename.starts_with("vitest.")
    {
        return !project.config.production;
    }
    project
        .config
        .plugins
        .iter()
        .map(String::as_str)
        .chain(
            project
                .owner(index)
                .into_iter()
                .flat_map(|m| m.dependencies.iter().map(|d| d.name.as_str())),
        )
        .filter(|name| !project.config.production || *name != "storybook")
        .any(|name| plugin_entry(name, path))
}
pub(crate) fn plugin_entry(name: &str, path: &str) -> bool {
    let basename = Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    match name {
        "next" => {
            path.starts_with("pages/")
                || path.starts_with("src/pages/")
                || [
                    "middleware.",
                    "instrumentation.",
                    "src/middleware.",
                    "src/instrumentation.",
                ]
                .iter()
                .any(|p| path.starts_with(p))
                || (path.starts_with("app/") || path.starts_with("src/app/"))
                    && [
                        "page",
                        "layout",
                        "route",
                        "loading",
                        "error",
                        "not-found",
                        "template",
                        "default",
                        "global-error",
                    ]
                    .iter()
                    .any(|n| basename.starts_with(&format!("{n}.")))
        }
        "@remix-run/dev" | "react-router" => path.starts_with("app/routes/"),
        "astro" => path.starts_with("src/pages/"),
        "@sveltejs/kit" => path.starts_with("src/routes/") && basename.starts_with('+'),
        "nuxt" => ["pages/", "server/", "middleware/", "plugins/"]
            .iter()
            .any(|p| path.starts_with(p)),
        "storybook" => path.contains(".stories."),
        "neovim" => path.starts_with("plugin/") || path.starts_with("after/"),
        "love" => path == "conf.lua" || path == "main.lua",
        _ => false,
    }
}

fn add_root(roots: &mut Vec<Root>, seen: &mut HashSet<usize>, file: usize, reason: &str) {
    if seen.insert(file) {
        roots.push(Root {
            file,
            reason: reason.into(),
        });
    }
}
fn entry_matches(project: &Project, dir: &str, entry: &str) -> Vec<usize> {
    if entry.starts_with('!') {
        return Vec::new();
    }
    let entry = entry.trim_start_matches("./");
    let joined = if dir.is_empty() {
        entry.to_string()
    } else {
        format!("{dir}/{entry}")
    };
    project
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            let stem = Path::new(&joined);
            matches_pattern(&f.display_path, &joined)
                || stem.extension().is_none()
                    && f.path
                        .strip_prefix(&project.root)
                        .is_ok_and(|p| p.with_extension("") == stem)
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn matches_pattern(path: &str, pattern: &str) -> bool {
    let pattern = pattern.trim_start_matches("./").replace('\\', "/");
    path == pattern
        || path.starts_with(&format!("{pattern}/"))
        || GlobBuilder::new(&pattern)
            .literal_separator(true)
            .build()
            .is_ok_and(|g| g.compile_matcher().is_match(path))
}
pub fn selected(path: &str, patterns: &[String]) -> bool {
    let positives = patterns.iter().filter(|p| !p.starts_with('!'));
    (patterns.iter().all(|p| p.starts_with('!'))
        || positives.clone().any(|p| matches_pattern(path, p)))
        && !patterns
            .iter()
            .filter_map(|p| p.strip_prefix('!'))
            .any(|p| matches_pattern(path, p))
}

fn collect_candidates(
    scan_root: &Path,
    root: &Path,
    config: &Config,
) -> Result<Vec<PathBuf>, String> {
    let mut walker = WalkBuilder::new(scan_root);
    walker.standard_filters(!config.no_gitignore).hidden(false);
    walker.filter_entry(|entry| {
        !entry.file_type().is_some_and(|t| t.is_dir())
            || !matches!(
                entry.file_name().to_str(),
                Some(
                    "target"
                        | "node_modules"
                        | "dist"
                        | "build"
                        | "out"
                        | "vendor"
                        | "coverage"
                        | ".git"
                        | ".deadwood-cache"
                        | ".deadwood-backup"
                )
            )
    });
    let mut paths = Vec::new();
    for entry in walker.build() {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if crate::lang::from_path(entry.path()).is_none()
            && !entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| config.compilers.contains_key(e))
        {
            continue;
        }
        let path = relative_display(root, entry.path());
        if config.exclude.iter().any(|p| matches_pattern(&path, p)) {
            continue;
        }
        paths.push(entry.path().to_path_buf());
    }
    paths.sort();
    Ok(paths)
}
fn load_config(
    root: &Path,
    config_path: Option<&Path>,
    include_tests: bool,
) -> Result<Config, String> {
    let path = config_path
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("deadwood.toml"));
    if config_path.is_some() && !path.is_file() {
        return Err(format!("configuration file not found: {}", path.display()));
    }
    let mut config = if path.exists() {
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let value: toml::Value =
            toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
        let explicit_tests = value.get("include_tests").is_some();
        let mut config: Config = value
            .try_into()
            .map_err(|e| format!("invalid {}: {e}", path.display()))?;
        if !explicit_tests {
            config.include_tests = true;
        }
        config
    } else {
        Config {
            include_tests: true,
            ..Default::default()
        }
    };
    config.include_tests |= include_tests;
    Ok(config)
}
fn validate_config(config: &Config) -> Result<(), String> {
    for pattern in config
        .entries
        .iter()
        .chain(&config.project)
        .chain(&config.exclude)
        .chain(config.workspaces.keys())
    {
        GlobBuilder::new(pattern.strip_prefix('!').unwrap_or(pattern))
            .build()
            .map_err(|e| format!("invalid pattern {pattern}: {e}"))?;
    }
    for kind in config
        .rules
        .keys()
        .chain(&config.include)
        .chain(config.ignore_issues.keys())
    {
        if !crate::report::ISSUE_TYPES.contains(&kind.as_str()) {
            return Err(format!("unknown issue type: {kind}"));
        }
    }
    if !config.compilers.is_empty() && !config.allow_compilers {
        return Err(
            "custom compilers require --allow-compilers (they execute configured commands)".into(),
        );
    }
    Ok(())
}
fn load_manifests(
    root: &Path,
    config: &Config,
) -> Result<(Option<PackageManifest>, Vec<PackageManifest>), String> {
    let mut directories = vec![root.to_path_buf()];
    let mut seen = HashSet::new();
    let mut manifests = Vec::new();
    let mut index = 0;
    while index < directories.len() {
        let dir = directories[index].clone();
        index += 1;
        let dir =
            fs::canonicalize(&dir).map_err(|e| format!("workspace {}: {e}", dir.display()))?;
        if !seen.insert(dir.clone()) {
            continue;
        }
        for name in ["package.json", "Cargo.toml", "go.mod"] {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if name == "package.json" {
                serde_json::from_str::<Value>(&text)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
            if name == "Cargo.toml" {
                toml::from_str::<toml::Value>(&text)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        if dir.join("tsconfig.json").exists() {
            ts_settings(&dir)?;
        }
        let mut patterns = workspace_patterns(&dir)?;
        if dir == root {
            patterns.extend(
                config
                    .workspaces
                    .keys()
                    .filter(|p| p.as_str() != ".")
                    .cloned(),
            );
        }
        if !patterns.is_empty() {
            let mut walker = WalkBuilder::new(&dir);
            walker.hidden(false).filter_entry(|e| {
                !matches!(
                    e.file_name().to_str(),
                    Some("target" | "node_modules" | ".git" | "vendor")
                )
            });
            for entry in walker.build() {
                let entry = entry.map_err(|e| e.to_string())?;
                if !entry.file_type().is_some_and(|t| t.is_dir()) || entry.path() == dir {
                    continue;
                }
                let name = relative_display(&dir, entry.path());
                if selected(&name, &patterns)
                    && !config
                        .ignore_workspaces
                        .iter()
                        .any(|p| matches_pattern(&name, p))
                {
                    directories.push(entry.path().to_path_buf());
                }
            }
        }
        for manifest in [
            load_package_manifest(&dir),
            load_cargo_manifest(&dir),
            load_go_manifest(&dir),
            load_lua_manifest(&dir),
        ]
        .into_iter()
        .flatten()
        {
            manifests.push(manifest);
        }
    }
    let primary = manifests
        .iter()
        .position(|m| m.kind == ManifestKind::Npm && m.path.parent() == Some(root))
        .map(|i| manifests.remove(i));
    Ok((primary, manifests))
}
fn workspace_patterns(root: &Path) -> Result<Vec<String>, String> {
    let mut patterns = Vec::new();
    if let Ok(text) = fs::read_to_string(root.join("package.json")) {
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if let Some(workspaces) = v.get("workspaces") {
            let values = workspaces
                .as_array()
                .or_else(|| workspaces.get("packages").and_then(Value::as_array));
            if let Some(values) = values {
                patterns.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
            }
        }
    }
    if let Ok(text) = fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        let v: Value = serde_yaml_ng::from_str(&text)
            .map_err(|e| format!("invalid pnpm-workspace.yaml: {e}"))?;
        if let Some(values) = v.get("packages").and_then(Value::as_array) {
            patterns.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    if let Ok(text) = fs::read_to_string(root.join("Cargo.toml")) {
        let v: toml::Value = toml::from_str(&text).map_err(|e| e.to_string())?;
        if let Some(values) = v
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(toml::Value::as_array)
        {
            patterns.extend(
                values
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_string),
            );
        }
        if let Some(values) = v
            .get("workspace")
            .and_then(|w| w.get("exclude"))
            .and_then(toml::Value::as_array)
        {
            patterns.extend(
                values
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(|s| format!("!{s}")),
            );
        }
    }
    if let Ok(text) = fs::read_to_string(root.join("go.work")) {
        let mut block = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with("use (") {
                block = true;
                continue;
            }
            if block && line == ")" {
                block = false;
                continue;
            }
            let candidate = if block {
                Some(line)
            } else {
                line.strip_prefix("use ")
            };
            if let Some(path) = candidate.filter(|p| !p.is_empty() && !p.starts_with("//")) {
                patterns.push(path.trim_matches('"').trim_start_matches("./").into());
            }
        }
    }
    Ok(patterns)
}
fn native_file_enabled(file: &crate::model::FileRecord, config: &Config) -> bool {
    file.language == Language::Go && config.semantic_go
        || crate::conditions::go_enabled(file, config)
}

pub fn read_jsonc(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    jsonc_parser::parse_to_serde_value(&text, &Default::default())
        .map_err(|e| format!("{}: {e}", path.display()))?
        .ok_or_else(|| format!("{} is empty", path.display()))
}
struct TsSettings {
    aliases: BTreeMap<String, Vec<String>>,
    source_dir: String,
    output_dir: Option<String>,
}
fn ts_settings(root: &Path) -> Result<TsSettings, String> {
    fn load(path: &Path, seen: &mut HashSet<PathBuf>) -> Result<Value, String> {
        let path = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !seen.insert(path.clone()) {
            return Err(format!("cyclic tsconfig extends: {}", path.display()));
        }
        let mut value = read_jsonc(&path)?;
        let parent = path.parent().unwrap_or(Path::new("."));
        let mut inherited = Value::Object(Default::default());
        let extends: Vec<_> = match value.get("extends") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
        for entry in extends {
            let base = if entry.starts_with('.') || Path::new(&entry).is_absolute() {
                parent.join(&entry)
            } else {
                parent
                    .ancestors()
                    .map(|p| p.join("node_modules").join(&entry))
                    .find(|p| p.exists() || p.with_extension("json").exists())
                    .unwrap_or_else(|| parent.join(entry))
            };
            let base = if base.is_dir() {
                base.join("tsconfig.json")
            } else if base.extension().is_none() {
                base.with_extension("json")
            } else {
                base
            };
            merge_json(&mut inherited, load(&base, seen)?);
        }
        if let Some(options) = value
            .get_mut("compilerOptions")
            .and_then(Value::as_object_mut)
        {
            let base = options
                .get("baseUrl")
                .and_then(Value::as_str)
                .map(|s| parent.join(s))
                .unwrap_or_else(|| parent.to_path_buf());
            if let Some(paths) = options.get_mut("paths").and_then(Value::as_object_mut) {
                for values in paths.values_mut().filter_map(Value::as_array_mut) {
                    for v in values {
                        if let Some(s) = v.as_str() {
                            *v = Value::String(base.join(s).to_string_lossy().into());
                        }
                    }
                }
            }
            for key in ["rootDir", "outDir"] {
                if let Some(path) = options.get(key).and_then(Value::as_str) {
                    options.insert(
                        key.into(),
                        Value::String(parent.join(path).to_string_lossy().into()),
                    );
                }
            }
        }
        merge_json(&mut inherited, value);
        seen.remove(&path);
        Ok(inherited)
    }
    let config = if root.join("tsconfig.json").exists() {
        load(&root.join("tsconfig.json"), &mut HashSet::new())?
    } else {
        Value::Null
    };
    let compiler = config.get("compilerOptions");
    let aliases = compiler
        .and_then(|v| v.get("paths"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let relative = |path: &str| {
        Path::new(path)
            .strip_prefix(root)
            .unwrap_or(Path::new(path))
            .to_string_lossy()
            .replace('\\', "/")
    };
    let source = compiler
        .and_then(|v| v.get("rootDir"))
        .and_then(Value::as_str)
        .map(relative)
        .unwrap_or_else(|| {
            if root.join("src").is_dir() {
                "src".into()
            } else {
                ".".into()
            }
        });
    let out = compiler
        .and_then(|v| v.get("outDir"))
        .and_then(Value::as_str)
        .map(relative);
    Ok(TsSettings {
        aliases,
        source_dir: source,
        output_dir: out,
    })
}
fn merge_json(target: &mut Value, value: Value) {
    if let (Some(a), Some(b)) = (target.as_object_mut(), value.as_object()) {
        for (key, value) in b {
            merge_json(a.entry(key.clone()).or_insert(Value::Null), value.clone());
        }
    } else {
        *target = value;
    }
}
pub(crate) fn map_source_entry(entry: &str, source: &str, out: Option<&str>) -> String {
    let path = entry.trim_start_matches("./");
    let mapped = out
        .and_then(|out| Path::new(path).strip_prefix(out).ok())
        .map(|p| Path::new(source).join(p))
        .unwrap_or_else(|| PathBuf::from(path));
    let extension = mapped.extension().and_then(|e| e.to_str()).unwrap_or("");
    if matches!(extension, "js" | "jsx" | "mjs" | "cjs") {
        // Patterns are intentionally broad enough to support source-first and mixed JS/TS packages.
        let stem = mapped
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        format!("{stem}.{{ts,tsx,mts,cts,js,jsx,mjs,cjs}}")
    } else {
        mapped.to_string_lossy().replace('\\', "/")
    }
}
fn script_entries(value: &Value) -> Vec<String> {
    value
        .get("scripts")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|m| m.values())
        .filter_map(Value::as_str)
        .flat_map(|s| shlex::split(s).unwrap_or_default())
        .filter(|s| {
            !s.starts_with('-')
                && matches!(
                    Path::new(s).extension().and_then(|e| e.to_str()),
                    Some("ts" | "tsx" | "mts" | "cts" | "js" | "mjs" | "cjs" | "jsx")
                )
        })
        .collect()
}
fn cargo_entries(root: &Path, value: &toml::Value) -> Vec<String> {
    let mut entries = Vec::new();
    if let Some(path) = value
        .get("lib")
        .and_then(|v| v.get("path"))
        .and_then(toml::Value::as_str)
    {
        entries.push(path.into());
    }
    for section in ["bin", "example", "bench", "test"] {
        if let Some(targets) = value.get(section).and_then(toml::Value::as_array) {
            entries.extend(
                targets
                    .iter()
                    .filter_map(|t| t.get("path"))
                    .filter_map(toml::Value::as_str)
                    .map(str::to_string),
            );
        }
    }
    if let Some(path) = value
        .get("package")
        .and_then(|v| v.get("build"))
        .and_then(toml::Value::as_str)
    {
        entries.push(path.into());
    }
    for dir in ["src/bin", "examples", "benches"] {
        if root.join(dir).is_dir() {
            entries.push(format!("{dir}/*.rs"));
            entries.push(format!("{dir}/*/main.rs"));
        }
    }
    entries
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

    let TsSettings {
        aliases,
        source_dir,
        output_dir,
    } = ts_settings(root).ok()?;
    let entries = entries
        .into_iter()
        .map(|entry| map_source_entry(&entry, &source_dir, output_dir.as_deref()))
        .collect();
    Some(PackageManifest {
        source: text.clone(),
        private: value
            .get("private")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        aliases,
        source_dir,
        output_dir,
        development_entries: script_entries(&value),
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
        ..PackageManifest::default()
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
    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for (section, kind) in [
                ("dependencies", "runtime"),
                ("build-dependencies", "build"),
                ("dev-dependencies", "development"),
            ] {
                if let Some(table) = target.get(section).and_then(toml::Value::as_table) {
                    dependencies.extend(table.keys().map(|name| Dependency {
                        name: name.replace('-', "_"),
                        kind,
                    }));
                }
            }
        }
    }
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
        source: text.clone(),
        kind: ManifestKind::Cargo,
        library: value
            .get("lib")
            .and_then(|l| l.get("path"))
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                root.join("src/lib.rs")
                    .exists()
                    .then(|| "src/lib.rs".into())
            }),
        path,
        package_name: value
            .get("package")
            .and_then(toml::Value::as_table)
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .map(|name| name.replace('-', "_")),
        entries: cargo_entries(root, &value),
        dependencies,
        optional_peers: Vec::new(),
        binaries: Vec::new(),
        catalog_path: None,
        catalog_entries: Vec::new(),
        catalog_references: Vec::new(),
        ..PackageManifest::default()
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
        source: text.clone(),
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
        ..PackageManifest::default()
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
        source: text.clone(),
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
        ..PackageManifest::default()
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

pub(crate) fn collect_strings(value: &Value, output: &mut Vec<String>, only_relative: bool) {
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

pub fn project_root(start: &Path) -> PathBuf {
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

fn conventional_entry(
    file: &crate::model::FileRecord,
    path: &str,
    configured: bool,
) -> Option<&'static str> {
    match file.language {
        Language::Go if file.package.as_deref() == Some("main") && file.has_main => {
            Some("Go main package")
        }
        Language::Rust
            if file.has_main
                && (path == "src/main.rs"
                    || path.starts_with("src/bin/")
                    || path.starts_with("examples/")) =>
        {
            Some("Rust executable")
        }
        Language::Rust if matches!(path, "src/lib.rs" | "src/main.rs" | "build.rs") => {
            Some("Rust target")
        }
        Language::TypeScript | Language::Tsx
            if !configured
                && matches!(
                    path,
                    "index.ts"
                        | "index.tsx"
                        | "index.js"
                        | "index.mjs"
                        | "index.cjs"
                        | "src/index.ts"
                        | "src/index.tsx"
                        | "src/index.js"
                        | "main.ts"
                        | "src/main.ts"
                        | "src/main.tsx"
                ) =>
        {
            Some("conventional package entry")
        }
        Language::Lua
            if !configured && matches!(path, "init.lua" | "main.lua" | "lua/init.lua") =>
        {
            Some("Lua entry")
        }
        _ => None,
    }
}

fn go_library_roots(project: &Project, roots: &mut Vec<Root>, seen: &mut HashSet<usize>) {
    // A library without an executable has a public package boundary, not a guessed executable.
    for manifest in project.manifests().filter(|m| m.kind == ManifestKind::Go) {
        if roots.iter().any(|r| {
            r.reason != "test entry"
                && project
                    .owner(r.file)
                    .is_some_and(|owner| owner.path == manifest.path)
        }) {
            continue;
        }
        for (index, file) in project.files.iter().enumerate() {
            if file.language == Language::Go
                && project
                    .owner(index)
                    .is_some_and(|m| m.path == manifest.path)
                && file.path.parent() == manifest.path.parent()
                && (!file.is_test || project.config.include_tests)
            {
                add_root(roots, seen, index, "Go library API");
            }
        }
    }
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
