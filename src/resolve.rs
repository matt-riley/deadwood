use crate::{
    discover,
    model::{Graph, ImportKind, Language, PackageUse, Project, Unresolved},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub fn build(project: &Project) -> Graph {
    let (roots, mut diagnostics) = discover::roots(project);
    let mut edges = vec![Vec::new(); project.files.len()];
    let mut unresolved = Vec::new();
    let mut package_uses = Vec::new();
    let mut used_packages = HashSet::new();
    let mut used_modules = HashSet::new();
    let mut resolved = HashMap::new();
    collect_edges(
        project,
        &mut edges,
        &mut resolved,
        &mut package_uses,
        &mut unresolved,
    );
    let mut reachable = vec![false; project.files.len()];
    let mut stack: Vec<_> = roots.iter().map(|r| r.file).collect();
    while let Some(file) = stack.pop() {
        if reachable[file] {
            continue;
        }
        reachable[file] = true;
        stack.extend(edges[file].iter().copied());
    }
    package_uses.retain(|p| reachable[p.file]);
    for usage in &package_uses {
        used_packages.insert(usage.name.clone());
        used_modules.insert(usage.raw.clone());
        if project.files[usage.file].language == Language::Lua
            && let Some(provider) = lua_provider(project, usage.file, &usage.raw)
        {
            used_packages.insert(provider);
        }
    }
    unresolved.retain(|u| reachable[u.file]);

    let mut used_symbols = HashSet::new();
    let mut used_enum_members = HashSet::new();
    let mut used_namespace_members = HashSet::new();
    // Public entry exports are an API boundary. Private applications can opt in to checking them.
    for root in &roots {
        if !include_entry_exports(project, root.file) {
            for export in &project.files[root.file].exports {
                propagate_symbol(
                    project,
                    &resolved,
                    root.file,
                    &export.name,
                    &mut used_symbols,
                    &mut HashSet::new(),
                );
            }
        }
    }
    mark_imported_symbols(
        project,
        &resolved,
        &reachable,
        &mut used_symbols,
        &mut used_enum_members,
        &mut used_namespace_members,
    );
    for (index, file) in project
        .files
        .iter()
        .enumerate()
        .filter(|(i, f)| reachable[*i] && !f.language.is_typescript())
    {
        let _ = file;
        native_usage(project, &edges, &reachable, index, &mut used_symbols);
    }
    diagnostics.extend(unresolved.iter().map(|u| crate::model::Diagnostic {
        kind: "unresolved_reference".into(),
        message: format!(
            "{}:{} could not resolve {}",
            project.files[u.file].display_path, u.line, u.raw
        ),
    }));
    let cycles = runtime_cycles(project, &edges, &reachable);
    let mut graph = Graph {
        semantic: Vec::new(),
        edges,
        cycles,
        unresolved,
        used_packages: used_packages.into_iter().collect(),
        used_modules: used_modules.into_iter().collect(),
        package_uses,
        used_symbols: used_symbols.into_iter().collect(),
        used_enum_members: used_enum_members.into_iter().collect(),
        used_namespace_members: used_namespace_members.into_iter().collect(),
        roots,
        reachable,
        diagnostics,
    };
    if project.config.semantic_go {
        crate::go::analyze(project, &mut graph);
    }
    graph
}

pub fn include_entry_exports(project: &Project, file: usize) -> bool {
    project
        .workspace(file)
        .and_then(|w| w.include_entry_exports)
        .unwrap_or(project.config.include_entry_exports)
}
fn exported_names(
    project: &Project,
    resolved: &HashMap<(usize, String), Vec<usize>>,
    file: usize,
    seen: &mut HashSet<usize>,
) -> Vec<String> {
    if !seen.insert(file) {
        return Vec::new();
    }
    let mut names: Vec<_> = project.files[file]
        .exports
        .iter()
        .map(|e| e.name.clone())
        .collect();
    for import in project.files[file]
        .symbol_imports
        .iter()
        .filter(|i| i.reexport && i.bindings.iter().any(|b| b.imported == "*"))
    {
        for target in resolved
            .get(&(file, import.raw.clone()))
            .into_iter()
            .flatten()
        {
            names.extend(exported_names(project, resolved, *target, seen));
        }
    }
    names.sort();
    names.dedup();
    names
}
fn propagate_symbol(
    project: &Project,
    resolved: &HashMap<(usize, String), Vec<usize>>,
    file: usize,
    name: &str,
    used: &mut HashSet<(usize, String)>,
    seen: &mut HashSet<(usize, String)>,
) {
    if !seen.insert((file, name.into())) {
        return;
    }
    used.insert((file, name.into()));
    let explicit = project.files[file].exports.iter().any(|e| e.name == name);
    for import in project.files[file]
        .symbol_imports
        .iter()
        .filter(|i| i.reexport)
    {
        for binding in &import.bindings {
            if binding.local != name && (binding.imported != "*" || explicit) {
                continue;
            }
            let original = if binding.imported == "*" {
                name
            } else {
                &binding.imported
            };
            for target in resolved
                .get(&(file, import.raw.clone()))
                .into_iter()
                .flatten()
            {
                propagate_symbol(project, resolved, *target, original, used, seen);
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn propagate_member(
    project: &Project,
    resolved: &HashMap<(usize, String), Vec<usize>>,
    file: usize,
    namespace: &str,
    name: &str,
    enums: &mut HashSet<(usize, String, String)>,
    namespaces: &mut HashSet<(usize, String, String)>,
    seen: &mut HashSet<usize>,
) {
    if !seen.insert(file) {
        return;
    }
    enums.insert((file, namespace.into(), name.into()));
    namespaces.insert((file, namespace.into(), name.into()));
    for import in project.files[file]
        .symbol_imports
        .iter()
        .filter(|i| i.reexport)
    {
        for binding in &import.bindings {
            if binding.local != namespace && binding.imported != "*" {
                continue;
            }
            for target in resolved
                .get(&(file, import.raw.clone()))
                .into_iter()
                .flatten()
            {
                propagate_member(
                    project,
                    resolved,
                    *target,
                    if binding.imported == "*" {
                        namespace
                    } else {
                        &binding.imported
                    },
                    name,
                    enums,
                    namespaces,
                    seen,
                );
            }
        }
    }
}

pub fn resolve_import(project: &Project, source: usize, raw: &str, kind: ImportKind) -> Vec<usize> {
    let file = &project.files[source];
    if kind == ImportKind::Resource {
        let mut pattern = PathBuf::new();
        for part in file
            .path
            .parent()
            .unwrap_or(&project.root)
            .join(raw)
            .components()
        {
            match part {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    pattern.pop();
                }
                other => pattern.push(other.as_os_str()),
            }
        }
        return project
            .files
            .iter()
            .enumerate()
            .filter(|(_, target)| {
                discover::matches_pattern(
                    &target.path.to_string_lossy(),
                    &pattern.to_string_lossy(),
                )
            })
            .map(|(i, _)| i)
            .collect();
    }
    let dir = project
        .owner(source)
        .and_then(|m| m.path.parent())
        .unwrap_or(&project.root);
    let mut candidates = Vec::new();
    match file.language {
        Language::TypeScript | Language::Tsx => {
            candidates.extend(typescript_candidates(project, source, raw, dir))
        }
        Language::Go => return go_import(project, raw, dir),
        Language::Rust => candidates.extend(rust_candidates(project, source, raw, kind, dir)),
        Language::Lua => candidates.extend(lua_candidates(project, source, raw, dir)),
    }
    candidates
        .into_iter()
        .filter_map(|p| {
            let canonical = fs::canonicalize(&p).unwrap_or(p);
            project.files.iter().position(|f| f.path == canonical)
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}
fn file_candidates(base: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut candidates = vec![base.to_path_buf()];
    let stem = if base.extension().is_some() {
        base.with_extension("")
    } else {
        base.to_path_buf()
    };
    for ext in extensions {
        candidates.push(stem.with_extension(ext));
        candidates.push(base.join(format!("index.{ext}")));
    }
    candidates
}
fn alias_match(raw: &str, pattern: &str) -> Option<String> {
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        raw.strip_prefix(prefix)?
            .strip_suffix(suffix)
            .map(str::to_string)
    } else {
        (raw == pattern).then(String::new)
    }
}
fn is_local_rust_use(raw: &str) -> bool {
    raw.starts_with("crate::") || raw.starts_with("self::") || raw.starts_with("super::")
}
fn local_rust_prefix(project: &Project, source: usize, raw: &str) -> bool {
    if !project.files[source].language.eq(&Language::Rust) {
        return false;
    }
    let first = raw.split("::").next().unwrap_or(raw);
    project.files[source]
        .imports
        .iter()
        .any(|i| i.kind == ImportKind::RustModule && i.raw == first)
}
fn rust_path_attribute(source: &str, name: &str) -> Option<String> {
    let declaration = format!("mod {name}");
    let before = source.split(&declaration).next()?;
    let attr = before.rsplit("#[path").next()?;
    if !attr.contains('=') {
        return None;
    }
    attr.split('"').nth(1).map(str::to_string)
}
pub fn package_name(raw: &str) -> String {
    let raw = raw.split("::").next().unwrap_or(raw);
    if raw.starts_with('@') {
        raw.split('/').take(2).collect::<Vec<_>>().join("/")
    } else {
        raw.split('/').next().unwrap_or(raw).into()
    }
}
pub fn lua_provider(project: &Project, source: usize, module: &str) -> Option<String> {
    if let Some(name) = project.config.lua_modules.get(module) {
        return Some(name.clone());
    }
    let manifest = project.owner(source)?;
    let mappings = [
        ("luafilesystem", "lfs"),
        ("luasocket", "socket"),
        ("luasocket", "mime"),
        ("luasec", "ssl"),
        ("lua-cjson", "cjson"),
        ("luabitop", "bit"),
        ("lpeg", "lpeg"),
    ];
    manifest
        .dependencies
        .iter()
        .find(|d| {
            d.name == module
                || module.starts_with(&format!("{}.", d.name))
                || mappings.iter().any(|(rock, prefix)| {
                    d.name == *rock
                        && (module == *prefix || module.starts_with(&format!("{prefix}.")))
                })
        })
        .map(|d| d.name.clone())
}
fn runtime_cycles(project: &Project, edges: &[Vec<usize>], reachable: &[bool]) -> Vec<Vec<usize>> {
    // Reachability-equivalent SCCs; sibling Go files are one compilation unit, not import cycles.
    let mut order = Vec::new();
    let mut visited = vec![false; edges.len()];
    fn visit(i: usize, edges: &[Vec<usize>], visited: &mut [bool], order: &mut Vec<usize>) {
        if visited[i] {
            return;
        }
        visited[i] = true;
        for next in &edges[i] {
            visit(*next, edges, visited, order);
        }
        order.push(i);
    }
    let filtered: Vec<Vec<usize>> = edges
        .iter()
        .enumerate()
        .map(|(i, targets)| {
            targets
                .iter()
                .copied()
                .filter(|target| {
                    if !reachable[*target]
                        || !reachable[i]
                        || project.files[i].language == Language::Go
                    {
                        return false;
                    }
                    project.files[i].imports.iter().any(|import| {
                        !import.type_only
                            && resolve_import(project, i, &import.raw, import.kind).contains(target)
                    })
                })
                .collect()
        })
        .collect();
    for (i, is_reachable) in reachable.iter().enumerate() {
        if *is_reachable {
            visit(i, &filtered, &mut visited, &mut order);
        }
    }
    let mut reverse = vec![Vec::new(); edges.len()];
    for (i, targets) in filtered.iter().enumerate() {
        for target in targets {
            reverse[*target].push(i);
        }
    }
    visited.fill(false);
    let mut result = Vec::new();
    while let Some(i) = order.pop() {
        if visited[i] {
            continue;
        }
        let mut group = Vec::new();
        let mut stack = vec![i];
        while let Some(i) = stack.pop() {
            if visited[i] {
                continue;
            }
            visited[i] = true;
            group.push(i);
            stack.extend(&reverse[i]);
        }
        if group.len() > 1 {
            group.sort();
            result.push(group);
        }
    }
    result
}

fn collect_edges(
    project: &Project,
    edges: &mut [Vec<usize>],
    resolved: &mut HashMap<(usize, String), Vec<usize>>,
    package_uses: &mut Vec<PackageUse>,
    unresolved: &mut Vec<Unresolved>,
) {
    for (source, file) in project.files.iter().enumerate() {
        if file.is_test && !project.config.include_tests {
            continue;
        }
        for import in &file.imports {
            let local_ts =
                file.language.is_typescript() && local_typescript_use(project, source, &import.raw);
            let targets = resolve_import(project, source, &import.raw, import.kind);
            resolved.insert((source, import.raw.clone()), targets.clone());
            for target in &targets {
                if *target != source && !edges[source].contains(target) {
                    edges[source].push(*target);
                }
            }
            let package = !local_ts
                && matches!(import.kind, ImportKind::Package | ImportKind::RustUse)
                && !is_local_rust_use(&import.raw)
                && !local_rust_prefix(project, source, &import.raw)
                || file.language == Language::Lua && targets.is_empty();
            if package {
                package_uses.push(PackageUse {
                    file: source,
                    line: import.line,
                    name: if file.language == Language::Go {
                        import.raw.clone()
                    } else {
                        package_name(&import.raw)
                    },
                    raw: import.raw.clone(),
                    resolved: !targets.is_empty(),
                });
            }
            let external_lua = file.language == Language::Lua
                && lua_provider(project, source, &import.raw).is_some();
            let local_rust = file.language == Language::Rust
                && import.kind != ImportKind::RustModule
                && (!targets.is_empty() || local_rust_prefix(project, source, &import.raw));
            if targets.is_empty()
                && !external_lua
                && !local_rust
                && (local_ts || matches!(import.kind, ImportKind::Module | ImportKind::RustModule))
                || targets.is_empty()
                    && import.kind == ImportKind::RustUse
                    && import.raw.starts_with("crate::")
            {
                unresolved.push(Unresolved {
                    file: source,
                    line: import.line,
                    raw: import.raw.clone(),
                    reason: "no source target matched this reference".into(),
                });
            }
        }
        if file.language == Language::Go {
            go_siblings(project, source, edges);
        }
    }
}

fn native_usage(
    project: &Project,
    edges: &[Vec<usize>],
    reachable: &[bool],
    index: usize,
    used_symbols: &mut HashSet<(usize, String)>,
) {
    let file = &project.files[index];
    for symbol in &file.symbols {
        let own_definitions = file
            .symbols
            .iter()
            .filter(|s| s.name == symbol.name)
            .count();
        let used_locally = file
            .symbol_references
            .iter()
            .filter(|r| *r == &symbol.name)
            .count()
            > own_definitions;
        let used_elsewhere = project.files.iter().enumerate().any(|(source, consumer)| {
            if source == index || !reachable[source] || consumer.language != file.language {
                return false;
            }
            if file.language == Language::Go && consumer.path.parent() == file.path.parent() {
                return consumer
                    .symbol_references
                    .iter()
                    .filter(|r| *r == &symbol.name)
                    .count()
                    > consumer
                        .symbols
                        .iter()
                        .filter(|s| s.name == symbol.name)
                        .count();
            }
            if file.language == Language::Rust
                && edges[source].contains(&index)
                && project.owner(source).map(|m| &m.path) == project.owner(index).map(|m| &m.path)
            {
                return consumer
                    .symbol_references
                    .iter()
                    .filter(|r| *r == &symbol.name)
                    .count()
                    > consumer
                        .symbols
                        .iter()
                        .filter(|s| s.name == symbol.name)
                        .count();
            }
            edges[source].contains(&index)
                && consumer
                    .member_references
                    .iter()
                    .any(|r| r.member == symbol.name)
        });
        if used_locally || used_elsewhere || symbol.name == "init" && file.language == Language::Go
        {
            used_symbols.insert((index, symbol.name.clone()));
        }
    }
}

fn mark_imported_symbols(
    project: &Project,
    resolved: &HashMap<(usize, String), Vec<usize>>,
    reachable: &[bool],
    used_symbols: &mut HashSet<(usize, String)>,
    used_enum_members: &mut HashSet<(usize, String, String)>,
    used_namespace_members: &mut HashSet<(usize, String, String)>,
) {
    for (index, file) in project
        .files
        .iter()
        .enumerate()
        .filter(|(i, _)| reachable[*i])
    {
        for import in &file.symbol_imports {
            if import.reexport {
                continue;
            }
            let targets = resolved
                .get(&(index, import.raw.clone()))
                .cloned()
                .unwrap_or_default();
            for target in targets {
                for binding in &import.bindings {
                    if binding.imported == "*" {
                        let members: Vec<_> = file
                            .member_references
                            .iter()
                            .filter(|r| r.namespace == binding.local)
                            .map(|r| r.member.clone())
                            .collect();
                        if members.is_empty() {
                            for name in
                                exported_names(project, resolved, target, &mut HashSet::new())
                            {
                                propagate_symbol(
                                    project,
                                    resolved,
                                    target,
                                    &name,
                                    used_symbols,
                                    &mut HashSet::new(),
                                );
                            }
                        } else {
                            for member in members {
                                propagate_symbol(
                                    project,
                                    resolved,
                                    target,
                                    &member,
                                    used_symbols,
                                    &mut HashSet::new(),
                                );
                            }
                        }
                    } else {
                        propagate_symbol(
                            project,
                            resolved,
                            target,
                            &binding.imported,
                            used_symbols,
                            &mut HashSet::new(),
                        );
                        for reference in file
                            .member_references
                            .iter()
                            .filter(|r| r.namespace == binding.local)
                        {
                            propagate_member(
                                project,
                                resolved,
                                target,
                                &binding.imported,
                                &reference.member,
                                used_enum_members,
                                used_namespace_members,
                                &mut HashSet::new(),
                            );
                        }
                    }
                }
            }
        }
        for reference in &file.member_references {
            if file
                .enum_members
                .iter()
                .any(|m| m.namespace == reference.namespace && m.name == reference.member)
            {
                used_enum_members.insert((
                    index,
                    reference.namespace.clone(),
                    reference.member.clone(),
                ));
            }
            if file
                .namespace_members
                .iter()
                .any(|m| m.namespace == reference.namespace && m.name == reference.member)
            {
                used_namespace_members.insert((
                    index,
                    reference.namespace.clone(),
                    reference.member.clone(),
                ));
            }
        }
        if file.language.is_typescript() {
            if project.config.ignore_exports_used_in_file {
                for export in &file.exports {
                    if file
                        .symbol_references
                        .iter()
                        .filter(|r| *r == &export.name)
                        .count()
                        > 1
                    {
                        propagate_symbol(
                            project,
                            resolved,
                            index,
                            &export.name,
                            used_symbols,
                            &mut HashSet::new(),
                        );
                    }
                }
            }
            continue;
        }
    }
}

fn typescript_candidates(project: &Project, source: usize, raw: &str, dir: &Path) -> Vec<PathBuf> {
    let file = &project.files[source];
    let mut candidates = Vec::new();
    if raw.starts_with('.') || Path::new(raw).is_absolute() {
        candidates.extend(file_candidates(
            &file.path.parent().unwrap_or(dir).join(raw),
            &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        ));
    } else {
        let mut aliases = project
            .owner(source)
            .map(|m| m.aliases.clone())
            .unwrap_or_default();
        aliases.extend(project.config.paths.iter().map(|(k, v)| {
            (
                k.clone(),
                v.iter()
                    .map(|p| project.root.join(p).to_string_lossy().into())
                    .collect(),
            )
        }));
        if let Some(w) = project.workspace(source) {
            aliases.extend(w.paths.iter().map(|(k, v)| {
                (
                    k.clone(),
                    v.iter()
                        .map(|p| dir.join(p).to_string_lossy().into())
                        .collect(),
                )
            }));
        }
        for (pattern, values) in aliases {
            if let Some(capture) = alias_match(raw, &pattern) {
                for value in values {
                    candidates.extend(file_candidates(
                        &dir.join(value.replace('*', &capture)),
                        &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
                    ));
                }
            }
        }
        for manifest in project
            .manifests()
            .filter(|m| m.kind == crate::model::ManifestKind::Npm)
        {
            let Some(name) = &manifest.package_name else {
                continue;
            };
            if raw == name {
                let base = manifest.path.parent().unwrap_or(&project.root);
                for entry in &manifest.entries {
                    for (i, target) in project.files.iter().enumerate() {
                        if target
                            .path
                            .strip_prefix(base)
                            .is_ok_and(|p| discover::matches_pattern(&p.to_string_lossy(), entry))
                        {
                            candidates.push(project.files[i].path.clone());
                        }
                    }
                }
                if manifest.entries.is_empty() {
                    candidates.extend(file_candidates(
                        &base.join("src/index"),
                        &["ts", "tsx", "js"],
                    ));
                }
            } else if let Some(subpath) = raw.strip_prefix(&format!("{name}/")) {
                candidates.extend(npm_subpath(project, manifest, subpath));
            }
        }

        if raw.starts_with('#')
            && let Ok(value) = discover::read_jsonc(&dir.join("package.json"))
            && let Some(imports) = value.get("imports").and_then(serde_json::Value::as_object)
        {
            for (pattern, value) in imports {
                if let Some(capture) = alias_match(raw, pattern) {
                    let targets: Vec<_> = match value {
                        serde_json::Value::String(s) => vec![s.as_str()],
                        serde_json::Value::Object(m) => {
                            m.values().filter_map(serde_json::Value::as_str).collect()
                        }
                        _ => Vec::new(),
                    };
                    for target in targets {
                        candidates.extend(file_candidates(
                            &dir.join(target.replace('*', &capture)),
                            &["ts", "tsx", "js"],
                        ));
                    }
                }
            }
        }
    }
    candidates
}

fn rust_candidates(
    project: &Project,
    source: usize,
    raw: &str,
    kind: ImportKind,
    dir: &Path,
) -> Vec<PathBuf> {
    let file = &project.files[source];
    let mut candidates = Vec::new();
    if kind == ImportKind::RustModule {
        let parent = file.path.parent().unwrap_or(dir);
        let base = rust_module_dir(project, source, dir);
        candidates.extend([
            base.join(format!("{raw}.rs")),
            base.join(raw).join("mod.rs"),
        ]);
        // #[path] is relative to the declaring source file.
        if let Some(path) = rust_path_attribute(&file.source, raw) {
            candidates.push(parent.join(path));
        }
    } else {
        let raw = raw
            .split("::{")
            .next()
            .unwrap_or(raw)
            .split(" as ")
            .next()
            .unwrap_or(raw);
        let parts: Vec<_> = raw.split("::").collect();
        let mut base = rust_root_dir(project, source, dir);
        let mut start = 0;
        if parts.first() == Some(&"crate") {
            start = 1;
        } else if parts.first() == Some(&"self") {
            base = rust_module_dir(project, source, dir);
            start = 1;
        } else if parts.first() == Some(&"super") {
            base = rust_module_dir(project, source, dir);
            while parts.get(start) == Some(&"super") {
                base = base.parent().unwrap_or(dir).to_path_buf();
                start += 1;
            }
        } else if let Some(manifest) = project.manifests().find(|m| {
            m.kind == crate::model::ManifestKind::Cargo
                && m.package_name.as_deref() == parts.first().copied()
        }) {
            let parent = manifest.path.parent().unwrap_or(dir);
            base = manifest
                .library
                .as_ref()
                .and_then(|lib| parent.join(lib).parent().map(Path::to_path_buf))
                .unwrap_or_else(|| parent.join("src"));
            start = 1;
        }
        for part in parts.iter().skip(start) {
            base = base.join(part);
            candidates.extend([base.with_extension("rs"), base.join("mod.rs")]);
        }
    }
    candidates
}

fn lua_candidates(project: &Project, source: usize, raw: &str, dir: &Path) -> Vec<PathBuf> {
    let file = &project.files[source];
    let mut candidates = Vec::new();
    if raw.ends_with(".lua") || raw.contains('/') {
        candidates.extend([dir.join(raw), file.path.parent().unwrap_or(dir).join(raw)]);
    } else {
        let module = raw.replace('.', "/");
        let mut paths = vec![
            "?.lua".to_string(),
            "?/init.lua".to_string(),
            "lua/?.lua".to_string(),
            "lua/?/init.lua".to_string(),
        ];
        paths.extend(project.config.lua_paths.clone());
        for pattern in paths {
            candidates.push(dir.join(pattern.replace('?', &module)));
        }
        if let Some(path) = project
            .owner(source)
            .and_then(|m| m.provided_modules.get(raw))
            && path.ends_with(".lua")
        {
            candidates.push(dir.join(path));
        }
    }
    candidates
}

fn go_import(project: &Project, raw: &str, dir: &Path) -> Vec<usize> {
    for manifest in project
        .manifests()
        .filter(|m| m.kind == crate::model::ManifestKind::Go)
    {
        let Some(module) = &manifest.package_name else {
            continue;
        };
        let suffix = if raw == module {
            Some("")
        } else {
            raw.strip_prefix(&format!("{module}/"))
        };
        if let Some(suffix) = suffix {
            let base = manifest.path.parent().unwrap_or(dir).join(suffix);
            return project
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.language == Language::Go
                        && f.path.parent() == Some(base.as_path())
                        && (!f.is_test || project.config.include_tests)
                })
                .map(|(i, _)| i)
                .collect();
        }
    }
    Vec::new()
}

fn go_siblings(project: &Project, source: usize, edges: &mut [Vec<usize>]) {
    let file = &project.files[source];
    for (target, sibling) in project.files.iter().enumerate() {
        if sibling.language == Language::Go
            && sibling.path.parent() == file.path.parent()
            && sibling.package == file.package
            && (!sibling.is_test || project.config.include_tests)
            && target != source
            && !edges[source].contains(&target)
        {
            edges[source].push(target);
        }
    }
}

fn npm_subpath(
    project: &Project,
    manifest: &crate::model::PackageManifest,
    subpath: &str,
) -> Vec<PathBuf> {
    let dir = manifest.path.parent().unwrap_or(&project.root);
    let value: serde_json::Value = serde_json::from_str(&manifest.source).unwrap_or_default();
    let Some(exports) = value.get("exports").and_then(serde_json::Value::as_object) else {
        return file_candidates(&dir.join(subpath), &["ts", "tsx", "js"]);
    };
    let mut candidates = Vec::new();
    for (key, value) in exports {
        let Some(capture) = alias_match(&format!("./{subpath}"), key) else {
            continue;
        };
        let mut targets = Vec::new();
        discover::collect_strings(value, &mut targets, true);
        for target in targets {
            let pattern = discover::map_source_entry(
                &target.replace('*', &capture),
                &manifest.source_dir,
                manifest.output_dir.as_deref(),
            );
            candidates.extend(
                project
                    .files
                    .iter()
                    .filter(|f| {
                        f.path.strip_prefix(dir).is_ok_and(|p| {
                            discover::matches_pattern(&p.to_string_lossy(), &pattern)
                        })
                    })
                    .map(|f| f.path.clone()),
            );
        }
    }
    candidates
}
fn local_typescript_use(project: &Project, source: usize, raw: &str) -> bool {
    raw.starts_with('#')
        || Path::new(raw).is_absolute()
        || project
            .config
            .paths
            .keys()
            .any(|p| alias_match(raw, p).is_some())
        || project
            .owner(source)
            .is_some_and(|m| m.aliases.keys().any(|p| alias_match(raw, p).is_some()))
        || project
            .workspace(source)
            .is_some_and(|m| m.paths.keys().any(|p| alias_match(raw, p).is_some()))
        || project.manifests().any(|m| {
            m.kind == crate::model::ManifestKind::Npm
                && m.package_name.as_deref() == Some(package_name(raw).as_str())
        })
}

fn rust_module_dir(project: &Project, source: usize, dir: &Path) -> PathBuf {
    let file = &project.files[source];
    let parent = file.path.parent().unwrap_or(dir);
    let name = file.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if matches!(name, "lib" | "main" | "mod" | "build")
        || parent == dir.join("tests")
        || project
            .owner(source)
            .is_some_and(|m| m.entries.iter().any(|e| dir.join(e) == file.path))
    {
        parent.into()
    } else {
        parent.join(name)
    }
}
fn rust_root_dir(project: &Project, source: usize, dir: &Path) -> PathBuf {
    project
        .owner(source)
        .into_iter()
        .flat_map(|m| m.entries.iter().chain(&m.library))
        .filter_map(|entry| dir.join(entry).parent().map(Path::to_path_buf))
        .filter(|base| project.files[source].path.starts_with(base))
        .max_by_key(|base| base.components().count())
        .unwrap_or_else(|| dir.join("src"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn package_names_preserve_scopes() {
        assert_eq!(super::package_name("lodash/map"), "lodash");
        assert_eq!(super::package_name("@scope/pkg/subpath"), "@scope/pkg");
    }
}
