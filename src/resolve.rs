use crate::discover;
use crate::model::{Graph, ImportKind, Language, PackageUse, Project, Unresolved};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub fn build(project: &Project) -> Graph {
    let (roots, root_diagnostics) = discover::roots(project);
    let mut edges = vec![Vec::new(); project.files.len()];
    let mut unresolved = Vec::new();
    let mut used_packages = HashSet::new();
    let mut used_modules = HashSet::new();
    let mut package_uses = Vec::new();
    let mut used_symbols = HashSet::new();
    let mut used_enum_members = HashSet::new();
    let mut used_namespace_members = HashSet::new();

    let by_display: HashMap<String, Vec<usize>> =
        project
            .files
            .iter()
            .enumerate()
            .fold(HashMap::new(), |mut map, (index, file)| {
                map.entry(file.display_path.clone())
                    .or_default()
                    .push(index);
                map
            });

    for (source_index, file) in project.files.iter().enumerate() {
        for import in &file.imports {
            let mut package_use_index = None;
            if matches!(import.kind, ImportKind::Package | ImportKind::RustUse)
                && !is_local_rust_use(&import.raw)
            {
                let name = package_name(&import.raw);
                used_packages.insert(name.clone());
                used_modules.insert(import.raw.clone());
                package_use_index = Some(package_uses.len());
                package_uses.push(PackageUse {
                    file: source_index,
                    line: import.line,
                    name: if file.language == Language::Go {
                        import.raw.clone()
                    } else {
                        name
                    },
                    raw: import.raw.clone(),
                    resolved: false,
                });
            } else if file.language == Language::Lua && import.kind == ImportKind::Module {
                used_modules.insert(import.raw.clone());
                package_use_index = Some(package_uses.len());
                package_uses.push(PackageUse {
                    file: source_index,
                    line: import.line,
                    name: package_name(&import.raw),
                    raw: import.raw.clone(),
                    resolved: false,
                });
            }

            let targets = match import.kind {
                ImportKind::Module => {
                    resolve_module(project, &by_display, source_index, &import.raw)
                }
                ImportKind::RustModule => {
                    resolve_rust_module(project, &by_display, source_index, &import.raw)
                }
                ImportKind::RustUse if is_local_rust_use(&import.raw) => {
                    resolve_rust_use(project, &by_display, source_index, &import.raw)
                }
                ImportKind::Package if file.language == Language::Go => {
                    resolve_go_package(project, &import.raw)
                }
                ImportKind::Package | ImportKind::RustUse => Vec::new(),
            };

            if targets.is_empty() {
                if should_report_unresolved(file.language, &import.kind, &import.raw) {
                    unresolved.push(Unresolved {
                        file: source_index,
                        line: import.line,
                        raw: import.raw.clone(),
                        reason: "no scanned target matched this reference".to_string(),
                    });
                }
            } else {
                if let Some(package_use_index) = package_use_index {
                    package_uses[package_use_index].resolved = true;
                }
                for target in targets {
                    if target != source_index && !edges[source_index].contains(&target) {
                        edges[source_index].push(target);
                    }
                }
            }
        }
    }

    for root in &roots {
        for export in &project.files[root.file].exports {
            used_symbols.insert((root.file, export.name.clone()));
        }
    }
    for (index, file) in project.files.iter().enumerate() {
        for reference in &file.member_references {
            if file.enum_members.iter().any(|member| {
                member.namespace == reference.namespace && member.name == reference.member
            }) {
                used_enum_members.insert((
                    index,
                    reference.namespace.clone(),
                    reference.member.clone(),
                ));
            }
            if file.namespace_members.iter().any(|member| {
                member.namespace == reference.namespace && member.name == reference.member
            }) {
                used_namespace_members.insert((
                    index,
                    reference.namespace.clone(),
                    reference.member.clone(),
                ));
            }
        }
        for export in &file.exports {
            if file
                .symbol_references
                .iter()
                .filter(|reference| *reference == &export.name)
                .count()
                > 1
            {
                used_symbols.insert((index, export.name.clone()));
            }
        }
        for symbol_import in &file.symbol_imports {
            let targets = resolve_module(project, &by_display, index, &symbol_import.raw);
            for target in targets {
                for binding in &symbol_import.bindings {
                    if binding.imported == "*" {
                        for export in &project.files[target].exports {
                            used_symbols.insert((target, export.name.clone()));
                        }
                    } else {
                        used_symbols.insert((target, binding.imported.clone()));
                    }
                    for reference in &file.member_references {
                        if reference.namespace != binding.local {
                            continue;
                        }
                        if binding.imported == "*" {
                            continue;
                        }
                        if project.files[target].enum_members.iter().any(|member| {
                            member.namespace == binding.imported && member.name == reference.member
                        }) {
                            used_enum_members.insert((
                                target,
                                binding.imported.clone(),
                                reference.member.clone(),
                            ));
                        }
                        if project.files[target]
                            .namespace_members
                            .iter()
                            .any(|member| {
                                member.namespace == binding.imported
                                    && member.name == reference.member
                            })
                        {
                            used_namespace_members.insert((
                                target,
                                binding.imported.clone(),
                                reference.member.clone(),
                            ));
                        }
                    }
                }
            }
        }
    }

    let mut reachable = vec![false; project.files.len()];
    let mut stack: Vec<usize> = roots.iter().map(|root| root.file).collect();
    while let Some(file) = stack.pop() {
        if reachable[file] {
            continue;
        }
        reachable[file] = true;
        stack.extend(edges[file].iter().copied());
    }

    for (index, file) in project.files.iter().enumerate() {
        if file.language.is_typescript() {
            continue;
        }
        for symbol in &file.symbols {
            let own_definitions = file
                .symbols
                .iter()
                .filter(|definition| definition.name == symbol.name)
                .count();
            let own_references = file
                .symbol_references
                .iter()
                .filter(|reference| *reference == &symbol.name)
                .count();
            let referenced_elsewhere =
                project
                    .files
                    .iter()
                    .enumerate()
                    .any(|(source, source_file)| {
                        if source == index || !reachable[source] {
                            return false;
                        }
                        if file.language == Language::Rust && source_file.language == Language::Rust
                        {
                            let definitions = source_file
                                .symbols
                                .iter()
                                .filter(|definition| definition.name == symbol.name)
                                .count();
                            let references = source_file
                                .symbol_references
                                .iter()
                                .filter(|reference| *reference == &symbol.name)
                                .count();
                            if references > definitions {
                                return true;
                            }
                        }
                        if file.language == Language::Go
                            && source_file.language == Language::Go
                            && file.package == source_file.package
                        {
                            let definitions = source_file
                                .symbols
                                .iter()
                                .filter(|definition| definition.name == symbol.name)
                                .count();
                            let references = source_file
                                .symbol_references
                                .iter()
                                .filter(|reference| *reference == &symbol.name)
                                .count();
                            return references > definitions;
                        }
                        if !edges[source].contains(&index) {
                            return false;
                        }
                        source_file.member_references.iter().any(|reference| {
                            reference.member == symbol.name
                                && (file.language != Language::Go
                                    || go_import_matches_target(
                                        project,
                                        source,
                                        index,
                                        &reference.namespace,
                                    ))
                                && (file.language != Language::Rust
                                    || reference
                                        .namespace
                                        .split("::")
                                        .any(|part| rust_module_name(&file.display_path) == part))
                        })
                    });
            if own_references > own_definitions || referenced_elsewhere {
                used_symbols.insert((index, symbol.name.clone()));
            }
        }
    }

    let mut diagnostics = root_diagnostics;
    diagnostics.extend(unresolved.iter().map(|item| crate::model::Diagnostic {
        kind: "unresolved_reference".to_string(),
        message: format!(
            "{}:{} could not resolve {}",
            project.files[item.file].display_path, item.line, item.raw
        ),
    }));

    Graph {
        edges,
        unresolved,
        used_packages: used_packages.into_iter().collect(),
        package_uses,
        used_modules: used_modules.into_iter().collect(),
        used_symbols: used_symbols.into_iter().collect(),
        used_enum_members: used_enum_members.into_iter().collect(),
        used_namespace_members: used_namespace_members.into_iter().collect(),
        roots,
        reachable,
        diagnostics,
    }
}

fn go_import_matches_target(
    project: &Project,
    source: usize,
    target: usize,
    namespace: &str,
) -> bool {
    project.files[source].imports.iter().any(|import| {
        import.kind == ImportKind::Package
            && resolve_go_package(project, &import.raw).contains(&target)
            && import
                .alias
                .as_deref()
                .or_else(|| import.raw.rsplit('/').next())
                == Some(namespace)
    })
}

fn rust_module_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

fn resolve_go_package(project: &Project, raw: &str) -> Vec<usize> {
    let Some(module) = go_module(&project.root) else {
        return Vec::new();
    };
    let suffix = if raw == module {
        ""
    } else if let Some(suffix) = raw.strip_prefix(&(module.clone() + "/")) {
        suffix
    } else {
        return Vec::new();
    };
    project
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| {
            if file.language != Language::Go {
                return false;
            }
            let parent = Path::new(&file.display_path)
                .parent()
                .map(|path| {
                    path.to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                })
                .unwrap_or_default();
            parent == suffix
        })
        .map(|(index, _)| index)
        .collect()
}

fn resolve_module(
    project: &Project,
    by_display: &HashMap<String, Vec<usize>>,
    source_index: usize,
    raw: &str,
) -> Vec<usize> {
    let source = &project.files[source_index];
    if !matches!(
        source.language,
        crate::model::Language::TypeScript
            | crate::model::Language::Tsx
            | crate::model::Language::Lua
    ) {
        return Vec::new();
    }

    let candidates = if source.language.is_typescript() {
        let base = source.path.parent().unwrap_or(&project.root).join(raw);
        file_candidates(&base, &["ts", "tsx", "mts", "cts"])
    } else {
        let module = raw.replace('.', "/");
        let bases = [
            project.root.join(&module),
            project.root.join("lua").join(&module),
        ];
        bases
            .into_iter()
            .flat_map(|base| [base.with_extension("lua"), base.join("init.lua"), base])
            .collect()
    };
    match_candidates(by_display, project.root.as_path(), candidates)
}

fn resolve_rust_module(
    project: &Project,
    by_display: &HashMap<String, Vec<usize>>,
    source_index: usize,
    raw: &str,
) -> Vec<usize> {
    let source = &project.files[source_index];
    let base = source.path.parent().unwrap_or(&project.root).join(raw);
    let candidates = [base.with_extension("rs"), base.join("mod.rs")];
    match_candidates(
        by_display,
        project.root.as_path(),
        candidates.into_iter().collect(),
    )
}

fn resolve_rust_use(
    project: &Project,
    by_display: &HashMap<String, Vec<usize>>,
    _source_index: usize,
    raw: &str,
) -> Vec<usize> {
    let Some(first) = raw
        .strip_prefix("crate::")
        .and_then(|value| value.split("::").next())
    else {
        return Vec::new();
    };
    let base = project.root.join("src").join(first);
    match_candidates(
        by_display,
        project.root.as_path(),
        file_candidates(&base, &["rs"]),
    )
}

fn match_candidates(
    by_display: &HashMap<String, Vec<usize>>,
    root: &Path,
    candidates: Vec<PathBuf>,
) -> Vec<usize> {
    let mut found = Vec::new();
    for candidate in candidates {
        let candidate = fs::canonicalize(&candidate).unwrap_or(candidate);
        let display = candidate
            .strip_prefix(root)
            .unwrap_or(&candidate)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if let Some(indices) = by_display.get(&display) {
            for index in indices {
                if !found.contains(index) {
                    found.push(*index);
                }
            }
        }
    }
    found
}

fn file_candidates(base: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut candidates = vec![base.to_path_buf()];
    for extension in extensions {
        candidates.push(base.with_extension(extension));
        candidates.push(base.join(format!("index.{extension}")));
        candidates.push(base.join(format!("mod.{extension}")));
    }
    candidates
}

fn should_report_unresolved(
    language: crate::model::Language,
    kind: &ImportKind,
    raw: &str,
) -> bool {
    match kind {
        ImportKind::Module | ImportKind::RustModule => true,
        ImportKind::RustUse => {
            language == crate::model::Language::Rust && raw.starts_with("crate::")
        }
        ImportKind::Package => false,
    }
}

fn is_local_rust_use(raw: &str) -> bool {
    raw.starts_with("crate::") || raw.starts_with("self::") || raw.starts_with("super::")
}

fn package_name(raw: &str) -> String {
    let raw = raw.split("::").next().unwrap_or(raw);
    if let Some(rest) = raw.strip_prefix('@') {
        format!("@{}", rest.split('/').take(2).collect::<Vec<_>>().join("/"))
    } else {
        raw.split('/').next().unwrap_or(raw).to_string()
    }
}

fn go_module(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("go.mod")).ok()?;
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("module ").map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::package_name;

    #[test]
    fn package_names_preserve_scopes() {
        assert_eq!(package_name("lodash/map"), "lodash");
        assert_eq!(package_name("@scope/pkg/subpath"), "@scope/pkg");
    }
}
