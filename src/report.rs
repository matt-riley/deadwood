use crate::model::{
    Diagnostic, Dropped, Finding, Graph, ManifestKind, OutputDiagnostic, Project, Report, Summary,
    SymbolKind,
};
use serde_json::json;

pub fn build(project: &Project, graph: &Graph) -> Report {
    let mut findings = Vec::new();

    for (index, file) in project.files.iter().enumerate() {
        if graph.reachable[index] || (file.is_test && !project.config.include_tests) {
            continue;
        }
        findings.push(Finding {
            kind: "unused_file".to_string(),
            file: file.display_path.clone(),
            line: 1,
            name: file.display_path.clone(),
            confidence: "high".to_string(),
            message: "file is not reachable from an inferred or configured entry point".to_string(),
            evidence: vec![
                "no reachable imports point to this file".to_string(),
                "file is not an entry point".to_string(),
            ],
        });
    }

    for file in project
        .files
        .iter()
        .filter(|file| file.language.is_typescript())
    {
        let exported: std::collections::HashSet<_> = file
            .exports
            .iter()
            .map(|export| export.name.as_str())
            .collect();
        let mut aliases = std::collections::BTreeMap::<String, Vec<String>>::new();
        for alias in &file.export_aliases {
            if exported.contains(alias.source.as_str())
                && exported.contains(alias.alias.as_str())
                && alias.source != alias.alias
            {
                let names = aliases
                    .entry(alias.source.clone())
                    .or_insert_with(|| vec![alias.source.clone()]);
                if !names.contains(&alias.alias) {
                    names.push(alias.alias.clone());
                }
            }
        }
        for names in aliases.into_values() {
            if names.len() > 1 {
                findings.push(Finding {
                    kind: "duplicate_export".to_string(),
                    file: file.display_path.clone(),
                    line: file
                        .export_aliases
                        .iter()
                        .find(|alias| alias.source == names[0])
                        .map(|alias| alias.line)
                        .unwrap_or(1),
                    name: names.join("|"),
                    confidence: "high".to_string(),
                    message: "multiple exports refer to the same local value".to_string(),
                    evidence: names
                        .iter()
                        .map(|name| format!("exported symbol: {name}"))
                        .collect(),
                });
            }
        }
    }

    let used_symbols: std::collections::HashSet<_> = graph.used_symbols.iter().collect();
    let used_enum_members: std::collections::HashSet<_> = graph.used_enum_members.iter().collect();
    let used_namespace_members: std::collections::HashSet<_> =
        graph.used_namespace_members.iter().collect();
    let root_files: std::collections::HashSet<_> =
        graph.roots.iter().map(|root| root.file).collect();
    let is_used_symbol = |index: usize, name: &str| {
        used_symbols
            .iter()
            .any(|(file_index, symbol)| *file_index == index && symbol == name)
    };
    for (index, file) in project.files.iter().enumerate() {
        if !file.language.is_typescript() || !graph.reachable[index] || root_files.contains(&index)
        {
            continue;
        }
        for export in &file.exports {
            if is_used_symbol(index, &export.name) {
                continue;
            }
            let (kind, message) = match export.kind {
                SymbolKind::Value => (
                    "unused_export",
                    "export is not referenced by a scanned entry point",
                ),
                SymbolKind::Type => (
                    "unused_type",
                    "exported type is not referenced by a scanned entry point",
                ),
            };
            findings.push(Finding {
                kind: kind.to_string(),
                file: file.display_path.clone(),
                line: export.line,
                name: export.name.clone(),
                confidence: "medium".to_string(),
                message: message.to_string(),
                evidence: vec![
                    "exported from a reachable file".to_string(),
                    "no import or local reference was found".to_string(),
                ],
            });
        }
        for member in &file.enum_members {
            if !is_used_symbol(index, &member.namespace)
                || used_enum_members
                    .iter()
                    .any(|(file_index, namespace, name)| {
                        *file_index == index
                            && namespace == &member.namespace
                            && name == &member.name
                    })
            {
                continue;
            }
            findings.push(member_finding(
                "unused_enum_member",
                file.display_path.clone(),
                member.line,
                &member.namespace,
                &member.name,
                "enum member is not referenced",
            ));
        }
        for member in &file.namespace_members {
            if !is_used_symbol(index, &member.namespace)
                || used_namespace_members
                    .iter()
                    .any(|(file_index, namespace, name)| {
                        *file_index == index
                            && namespace == &member.namespace
                            && name == &member.name
                    })
            {
                continue;
            }
            findings.push(member_finding(
                "unused_namespace_member",
                file.display_path.clone(),
                member.line,
                &member.namespace,
                &member.name,
                "namespace member is not referenced",
            ));
        }
    }

    for (index, file) in project.files.iter().enumerate() {
        if file.language.is_typescript()
            || !graph.reachable[index]
            || root_files.contains(&index)
            || (file.is_test && !project.config.include_tests)
        {
            continue;
        }
        for symbol in &file.symbols {
            if graph
                .used_symbols
                .iter()
                .any(|(file_index, name)| *file_index == index && name == &symbol.name)
            {
                continue;
            }
            findings.push(Finding {
                kind: "unused_symbol".to_string(),
                file: file.display_path.clone(),
                line: symbol.line,
                name: symbol.name.clone(),
                confidence: "medium".to_string(),
                message: "symbol is not referenced by a reachable scanned file".to_string(),
                evidence: vec![
                    format!("declared as a {:?} symbol", symbol.kind),
                    "no matching reference was found".to_string(),
                ],
            });
        }
    }

    let used: std::collections::HashSet<_> = graph.used_packages.iter().collect();
    for manifest in project
        .manifest
        .iter()
        .chain(project.additional_manifests.iter())
    {
        for dependency in &manifest.dependencies {
            if !matches!(dependency.kind, "runtime" | "development") {
                continue;
            }
            if dependency_is_used(manifest.kind, &dependency.name, &used, graph)
                || (dependency.kind == "development"
                    && (development_dependency_is_used(project, &dependency.name)
                        || manifest
                            .binaries
                            .iter()
                            .any(|binary| binary == &dependency.name)))
            {
                continue;
            }
            findings.push(Finding {
                kind: if dependency.kind == "development" {
                    "unused_dev_dependency".to_string()
                } else {
                    "unused_dependency".to_string()
                },
                file: display_manifest(project, manifest),
                line: 1,
                name: dependency.name.clone(),
                confidence: "medium".to_string(),
                message: format!(
                    "{} dependency is not referenced by a scanned source file",
                    dependency.kind
                ),
                evidence: vec![
                    format!("declared as {}", dependency.kind),
                    "no matching import was found".to_string(),
                ],
            });
        }
    }

    for manifest in project
        .manifest
        .iter()
        .chain(project.additional_manifests.iter())
        .filter(|manifest| manifest.kind != ManifestKind::Npm)
    {
        let declared: Vec<_> = manifest
            .dependencies
            .iter()
            .filter(|dependency| matches!(dependency.kind, "runtime" | "development"))
            .map(|dependency| dependency.name.as_str())
            .collect();
        let mut seen = std::collections::HashSet::new();
        for package_use in &graph.package_uses {
            let file = &project.files[package_use.file];
            if !manifest_matches_language(manifest.kind, file.language)
                || package_use.resolved
                || is_system_import(file.language, &package_use.raw)
                || is_local_crate_import(manifest, file.language, &package_use.raw)
                || declared
                    .iter()
                    .any(|name| dependency_matches_import(manifest.kind, name, &package_use.raw))
                || !seen.insert((package_use.file, package_use.raw.clone()))
            {
                continue;
            }
            findings.push(Finding {
                kind: "unlisted_dependency".to_string(),
                file: file.display_path.clone(),
                line: package_use.line,
                name: package_use.name.clone(),
                confidence: "high".to_string(),
                message: "package is imported but not declared by the project manifest".to_string(),
                evidence: vec![
                    format!("imported as {}", package_use.raw),
                    "not present in the language manifest dependencies".to_string(),
                ],
            });
        }
    }

    if let Some(manifest) = &project.manifest
        && manifest.kind == ManifestKind::Npm
    {
        let references: std::collections::HashSet<_> = manifest
            .catalog_references
            .iter()
            .map(|reference| (&reference.namespace, &reference.name))
            .collect();
        let catalog_file = manifest
            .catalog_path
            .as_ref()
            .map(|path| display_path(project, path))
            .unwrap_or_else(|| display_manifest(project, manifest));
        for entry in &manifest.catalog_entries {
            if references.contains(&(&entry.namespace, &entry.name)) {
                continue;
            }
            findings.push(Finding {
                kind: "unused_catalog".to_string(),
                file: catalog_file.clone(),
                line: entry.line,
                name: catalog_name(&entry.namespace, &entry.name),
                confidence: "high".to_string(),
                message: "catalog entry is not referenced by a package manifest".to_string(),
                evidence: vec![
                    format!("declared in {} catalog", entry.namespace),
                    "no catalog protocol reference was found".to_string(),
                ],
            });
        }
        let entries: std::collections::HashSet<_> = manifest
            .catalog_entries
            .iter()
            .map(|entry| (&entry.namespace, &entry.name))
            .collect();
        for reference in &manifest.catalog_references {
            if entries.contains(&(&reference.namespace, &reference.name)) {
                continue;
            }
            findings.push(Finding {
                kind: "catalog_reference".to_string(),
                file: display_manifest(project, manifest),
                line: 1,
                name: catalog_name(&reference.namespace, &reference.name),
                confidence: "high".to_string(),
                message: "catalog protocol reference has no matching catalog entry".to_string(),
                evidence: vec![
                    format!("references the {} catalog", reference.namespace),
                    "no matching catalog entry was found".to_string(),
                ],
            });
        }
    }

    if let Some(manifest) = &project.manifest
        && manifest.kind == ManifestKind::Npm
    {
        let has_direct_dependency = |name: &str| {
            manifest.dependencies.iter().any(|dependency| {
                matches!(dependency.kind, "runtime" | "development") && dependency.name == name
            })
        };
        let mut seen = std::collections::HashSet::new();
        for package_use in &graph.package_uses {
            if manifest.optional_peers.contains(&package_use.name)
                && !has_direct_dependency(&package_use.name)
                && seen.insert(package_use.name.clone())
            {
                findings.push(Finding {
                    kind: "optional_peer_dependency".to_string(),
                    file: display_manifest(project, manifest),
                    line: 1,
                    name: package_use.name.clone(),
                    confidence: "medium".to_string(),
                    message: "optional peer dependency is referenced but not installed directly"
                        .to_string(),
                    evidence: vec![
                        "referenced by a scanned source file".to_string(),
                        "declared as an optional peer dependency".to_string(),
                    ],
                });
            }
        }
    }

    for unresolved in &graph.unresolved {
        let file = &project.files[unresolved.file];
        findings.push(Finding {
            kind: "unresolved".to_string(),
            file: file.display_path.clone(),
            line: unresolved.line,
            name: unresolved.raw.clone(),
            confidence: "high".to_string(),
            message: "reference could not be resolved to a scanned file".to_string(),
            evidence: vec![unresolved.reason.clone()],
        });
    }

    if let Some(manifest) = &project.manifest
        && manifest.kind == ManifestKind::Npm
    {
        let declared: std::collections::HashSet<_> = manifest
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect();
        let mut seen = std::collections::HashSet::new();
        for package_use in &graph.package_uses {
            if declared.contains(package_use.name.as_str())
                || is_node_builtin(&package_use.name)
                || !seen.insert((package_use.file, package_use.name.clone()))
            {
                continue;
            }
            let file = &project.files[package_use.file];
            findings.push(Finding {
                    kind: "unlisted_dependency".to_string(),
                    file: file.display_path.clone(),
                    line: package_use.line,
                    name: package_use.name.clone(),
                    confidence: "high".to_string(),
                    message: "package is imported but not declared in package.json".to_string(),
                    evidence: vec![
                        format!("imported as {}", package_use.raw),
                        "not present in dependencies, devDependencies, optionalDependencies, or peerDependencies".to_string(),
                    ],
                });
        }
        let declared_names: std::collections::HashSet<_> = manifest
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect();
        for binary in &manifest.binaries {
            if !declared_names.contains(binary.as_str()) {
                findings.push(Finding {
                    kind: "unlisted_binary".to_string(),
                    file: display_manifest(project, manifest),
                    line: 1,
                    name: binary.clone(),
                    confidence: "medium".to_string(),
                    message: "package script invokes a binary not declared by the package"
                        .to_string(),
                    evidence: vec![
                        "found in a package.json script".to_string(),
                        "no matching package declaration was found".to_string(),
                    ],
                });
            }
        }
    }

    findings.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut diagnostics: Vec<OutputDiagnostic> =
        project.diagnostics.iter().map(output_diagnostic).collect();
    diagnostics.extend(graph.diagnostics.iter().map(output_diagnostic));

    let unused_files = findings
        .iter()
        .filter(|finding| finding.kind == "unused_file")
        .count();
    let unused_dependencies = findings
        .iter()
        .filter(|finding| finding.kind == "unused_dependency")
        .count();

    Report {
        v: 1,
        ok: findings.is_empty(),
        summary: Summary {
            files: project.files.len(),
            roots: graph.roots.len(),
            reachable_files: graph.reachable.iter().filter(|value| **value).count(),
            unused_files,
            unused_dependencies,
            unresolved: graph.unresolved.len(),
            parse_errors: project.files.iter().filter(|file| file.parse_error).count(),
        },
        findings,
        diagnostics,
        dropped: Dropped::default(),
    }
}

pub fn render_json(mut report: Report, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return serde_json::to_string(&report).expect("report is serializable");
    }

    loop {
        let encoded = serde_json::to_string(&report).expect("report is serializable");
        if encoded.len().div_ceil(4) <= max_tokens || report.findings.is_empty() {
            if encoded.len().div_ceil(4) <= max_tokens {
                return encoded;
            }
            let minimal = json!({
                "v": report.v,
                "ok": report.ok,
                "dropped": {"not_shown": report.dropped.not_shown + report.findings.len()}
            })
            .to_string();
            return if minimal.len().div_ceil(4) <= max_tokens {
                minimal
            } else {
                "{}".to_string()
            };
        }
        report.findings.pop();
        report.dropped.not_shown += 1;
    }
}

pub fn render_text(report: &Report) -> String {
    let mut lines = Vec::new();
    for finding in &report.findings {
        lines.push(format!(
            "{} {}:{} {} [{}]",
            finding.kind, finding.file, finding.line, finding.name, finding.confidence
        ));
    }
    for diagnostic in &report.diagnostics {
        lines.push(format!(
            "diagnostic {}: {}",
            diagnostic.kind, diagnostic.message
        ));
    }
    if report.dropped.not_shown > 0 {
        lines.push(format!(
            "{} findings withheld by the token budget",
            report.dropped.not_shown
        ));
    }
    lines.join("\n") + if lines.is_empty() { "" } else { "\n" }
}

fn catalog_name(_namespace: &str, name: &str) -> String {
    name.to_string()
}

fn display_path(project: &Project, path: &std::path::Path) -> String {
    path.strip_prefix(&project.root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn display_manifest(project: &Project, manifest: &crate::model::PackageManifest) -> String {
    manifest
        .path
        .strip_prefix(&project.root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| manifest.path.to_string_lossy().to_string())
}

fn development_dependency_is_used(project: &Project, name: &str) -> bool {
    name == "typescript"
        && project
            .files
            .iter()
            .any(|file| file.language.is_typescript())
}

fn manifest_matches_language(kind: ManifestKind, language: crate::model::Language) -> bool {
    matches!(
        (kind, language),
        (ManifestKind::Cargo, crate::model::Language::Rust)
            | (ManifestKind::Go, crate::model::Language::Go)
            | (ManifestKind::Lua, crate::model::Language::Lua)
    )
}

fn is_local_crate_import(
    manifest: &crate::model::PackageManifest,
    language: crate::model::Language,
    raw: &str,
) -> bool {
    language == crate::model::Language::Rust
        && manifest.kind == ManifestKind::Cargo
        && manifest.package_name.as_deref() == raw.split("::").next()
}

fn is_system_import(language: crate::model::Language, raw: &str) -> bool {
    match language {
        crate::model::Language::Go => !raw.contains('.'),
        crate::model::Language::Rust => matches!(
            raw.split("::").next().unwrap_or(raw),
            "std" | "core" | "alloc" | "proc_macro" | "str"
        ),
        crate::model::Language::Lua => false,
        crate::model::Language::TypeScript | crate::model::Language::Tsx => false,
    }
}

fn dependency_matches_import(kind: ManifestKind, dependency: &str, raw: &str) -> bool {
    match kind {
        ManifestKind::Cargo => {
            let imported = raw.split("::").next().unwrap_or(raw).replace('-', "_");
            imported == dependency.replace('-', "_")
        }
        ManifestKind::Go => raw == dependency || raw.starts_with(&format!("{dependency}/")),
        ManifestKind::Lua => raw == dependency || raw.starts_with(&format!("{dependency}.")),
        ManifestKind::Npm => false,
    }
}

fn dependency_is_used(
    kind: ManifestKind,
    name: &str,
    used: &std::collections::HashSet<&String>,
    graph: &Graph,
) -> bool {
    match kind {
        ManifestKind::Npm | ManifestKind::Cargo => used.iter().any(|value| value.as_str() == name),
        ManifestKind::Go => graph
            .used_modules
            .iter()
            .any(|module| module == name || module.starts_with(&format!("{name}/"))),
        ManifestKind::Lua => graph.used_modules.iter().any(|module| module == name),
    }
}

fn member_finding(
    kind: &str,
    file: String,
    line: u32,
    namespace: &str,
    name: &str,
    message: &str,
) -> Finding {
    Finding {
        kind: kind.to_string(),
        file,
        line,
        name: format!("{namespace}.{name}"),
        confidence: "medium".to_string(),
        message: message.to_string(),
        evidence: vec![
            format!("member of {namespace}"),
            "no reference was found".to_string(),
        ],
    }
}

fn is_node_builtin(name: &str) -> bool {
    let name = name.strip_prefix("node:").unwrap_or(name);
    matches!(
        name,
        "assert"
            | "buffer"
            | "child_process"
            | "cluster"
            | "console"
            | "constants"
            | "crypto"
            | "dgram"
            | "diagnostics_channel"
            | "dns"
            | "domain"
            | "events"
            | "fs"
            | "http"
            | "http2"
            | "https"
            | "module"
            | "net"
            | "os"
            | "path"
            | "perf_hooks"
            | "process"
            | "punycode"
            | "querystring"
            | "readline"
            | "repl"
            | "stream"
            | "string_decoder"
            | "sys"
            | "timers"
            | "tls"
            | "trace_events"
            | "tty"
            | "url"
            | "util"
            | "v8"
            | "vm"
            | "wasi"
            | "worker_threads"
            | "zlib"
            | "test"
    )
}

fn output_diagnostic(diagnostic: &Diagnostic) -> OutputDiagnostic {
    OutputDiagnostic {
        kind: diagnostic.kind.clone(),
        message: diagnostic.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Report;

    #[test]
    fn tiny_json_budget_remains_valid() {
        let report = Report {
            v: 1,
            ok: false,
            summary: Summary {
                files: 1,
                roots: 1,
                reachable_files: 1,
                unused_files: 0,
                unused_dependencies: 0,
                unresolved: 0,
                parse_errors: 0,
            },
            findings: vec![],
            diagnostics: vec![],
            dropped: Dropped::default(),
        };
        let output = render_json(report, 1);
        serde_json::from_str::<serde_json::Value>(&output).unwrap();
    }
}
