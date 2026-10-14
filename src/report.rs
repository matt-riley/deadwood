use crate::model::{
    Diagnostic, Dropped, Finding, Graph, ManifestKind, OutputDiagnostic, Project, Report, Rule,
    Summary, SymbolKind,
};
use crate::{discover::matches_pattern, resolve};
use serde_json::json;
use std::collections::HashSet;

pub const ISSUE_TYPES: &[&str] = &[
    "unused_file",
    "unused_dependency",
    "unused_dev_dependency",
    "unused_export",
    "unused_type",
    "unused_symbol",
    "unused_enum_member",
    "unused_class_member",
    "unused_namespace_member",
    "unused_namespace_export",
    "unused_namespace_type",
    "unlisted_dependency",
    "unlisted_binary",
    "unresolved",
    "optional_peer_dependency",
    "duplicate_export",
    "unused_catalog",
    "catalog_reference",
    "cycle",
];
pub fn build(project: &Project, graph: &Graph) -> Report {
    let mut findings = Vec::new();
    file_findings(project, graph, &mut findings);
    duplicate_findings(project, graph, &mut findings);
    symbol_findings(project, graph, &mut findings);
    semantic_findings(project, graph, &mut findings);
    dependency_findings(project, graph, &mut findings);
    unlisted_findings(project, graph, &mut findings);
    catalog_findings(project, graph, &mut findings);
    cycle_findings(project, graph, &mut findings);
    let mut diagnostics: Vec<_> = project
        .diagnostics
        .iter()
        .chain(&graph.diagnostics)
        .filter(|d| {
            !(project.config.semantic_go
                && matches!(d.kind.as_str(), "parse_error" | "dynamic_reference")
                && project.files.iter().any(|f| {
                    f.language == crate::model::Language::Go
                        && (d.message == format!("{} contains syntax errors", f.display_path)
                            || d.message.starts_with(&format!("{}:", f.display_path)))
                }))
        })
        .map(output_diagnostic)
        .collect();
    diagnostics.extend(
        graph
            .semantic
            .iter()
            .filter(|s| s.complete)
            .filter_map(|s| s.reachable().err())
            .map(|message| OutputDiagnostic {
                kind: "semantic_error".into(),
                message,
            }),
    );
    apply_rules(project, &mut findings);
    configuration_hints(project, &findings, &mut diagnostics);
    findings.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.file.cmp(&b.file))
            .then(a.name.cmp(&b.name))
    });
    findings.dedup_by(|a, b| a.kind == b.kind && a.file == b.file && a.name == b.name);
    let complete = graph.semantic.iter().all(|g| g.complete)
        && !diagnostics.iter().any(|d| {
            matches!(
                d.kind.as_str(),
                "no_entrypoints"
                    | "no_source_files"
                    | "missing_entry"
                    | "parse_error"
                    | "unreadable_file"
                    | "unsupported_analysis"
                    | "dynamic_reference"
                    | "semantic_error"
            )
        });
    Report {
        analysis: graph.semantic.iter().map(|g| g.summary()).collect(),
        fixes: Vec::new(),
        traces: Vec::new(),
        v: 1,
        complete,
        ok: complete && !findings.iter().any(|f| f.severity == "error"),
        summary: Summary {
            files: project.files.len(),
            roots: graph.roots.len(),
            reachable_files: graph.reachable.iter().filter(|b| **b).count(),
            unused_files: findings.iter().filter(|f| f.kind == "unused_file").count(),
            unused_dependencies: findings
                .iter()
                .filter(|f| {
                    matches!(
                        f.kind.as_str(),
                        "unused_dependency" | "unused_dev_dependency"
                    )
                })
                .count(),
            unresolved: graph.unresolved.len(),
            parse_errors: project
                .files
                .iter()
                .filter(|f| {
                    f.parse_error
                        && !(project.config.semantic_go && f.language == crate::model::Language::Go)
                })
                .count(),
        },
        findings,
        diagnostics,
        dropped: Dropped::default(),
    }
}
pub fn finding(
    kind: &str,
    file: &str,
    line: u32,
    name: &str,
    confidence: &str,
    message: &str,
    evidence: Vec<String>,
) -> Finding {
    Finding {
        kind: kind.into(),
        severity: "error".into(),
        file: file.into(),
        line,
        name: name.into(),
        confidence: confidence.into(),
        message: message.into(),
        evidence,
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
    finding(
        kind,
        &file,
        line,
        &format!("{namespace}.{name}"),
        "medium",
        message,
        vec![format!("member of {namespace}")],
    )
}
fn public_api(project: &Project, index: usize) -> bool {
    if project.files[index].language == crate::model::Language::Go {
        return true;
    }
    let Some(manifest) = project.owner(index) else {
        return false;
    };
    let Some(library) = &manifest.library else {
        return false;
    };
    let path = manifest
        .path
        .parent()
        .unwrap_or(&project.root)
        .join(library);
    let Some(root) = project.files.iter().position(|f| f.path == path) else {
        return false;
    };
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(file) = stack.pop() {
        if !seen.insert(file) {
            continue;
        }
        if file == index {
            return true;
        }
        for import in &project.files[file].imports {
            if import.kind == crate::model::ImportKind::RustModule && import.public {
                stack.extend(resolve::resolve_import(
                    project,
                    file,
                    &import.raw,
                    import.kind,
                ));
            }
        }
    }
    false
}
pub(crate) fn dependency_matches(
    project: &Project,
    manifest: &crate::model::PackageManifest,
    source: usize,
    name: &str,
    raw: &str,
) -> bool {
    match manifest.kind {
        ManifestKind::Npm => resolve::package_name(raw) == name,
        ManifestKind::Cargo => {
            raw.split("::").next().unwrap_or(raw).replace('-', "_") == name.replace('-', "_")
        }
        ManifestKind::Go => raw == name || raw.starts_with(&format!("{name}/")),
        ManifestKind::Lua => resolve::lua_provider(project, source, raw).as_deref() == Some(name),
    }
}
pub fn binary_provider(manifest: &crate::model::PackageManifest, binary: &str) -> Option<String> {
    let known = [
        ("tsc", "typescript"),
        ("tsserver", "typescript"),
        ("boxen", "boxen-cli"),
        ("babel", "@babel/cli"),
        ("ng", "@angular/cli"),
        ("vue-cli-service", "@vue/cli-service"),
    ];
    for dep in &manifest.dependencies {
        if dep.name == binary
            || known
                .iter()
                .any(|(bin, package)| binary == *bin && dep.name == *package)
        {
            return Some(dep.name.clone());
        }
        if let Some(root) = manifest.path.parent()
            && let Ok(value) = crate::discover::read_jsonc(
                &root
                    .join("node_modules")
                    .join(&dep.name)
                    .join("package.json"),
            )
            && value.get("bin").is_some_and(|bin| {
                bin.as_object().is_some_and(|m| m.contains_key(binary))
                    || bin.is_string() && dep.name.rsplit('/').next() == Some(binary)
            })
        {
            return Some(dep.name.clone());
        }
    }
    None
}
fn is_system_import(language: crate::model::Language, raw: &str) -> bool {
    match language {
        crate::model::Language::Go => !raw.contains('.'),
        crate::model::Language::Rust => matches!(
            raw.split("::").next().unwrap_or(raw),
            "std"
                | "core"
                | "alloc"
                | "proc_macro"
                | "str"
                | "char"
                | "bool"
                | "usize"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "f32"
                | "f64"
        ),
        crate::model::Language::Lua => false,
        crate::model::Language::TypeScript | crate::model::Language::Tsx => {
            is_node_builtin(&resolve::package_name(raw))
        }
    }
}
fn display_path(project: &Project, path: &std::path::Path) -> String {
    path.strip_prefix(&project.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
fn apply_rules(project: &Project, findings: &mut Vec<Finding>) {
    findings.retain_mut(|f| {
        if !project.config.include.is_empty()
            && !project.config.include.iter().any(|k| k == &f.kind)
        {
            return false;
        }
        let rule = project
            .config
            .rules
            .get(&f.kind)
            .copied()
            .unwrap_or(if f.kind == "cycle" {
                Rule::Warn
            } else {
                Rule::Error
            });
        if rule == Rule::Off {
            return false;
        }
        f.severity = if rule == Rule::Warn { "warn" } else { "error" }.into();
        let file = project.files.iter().position(|r| r.display_path == f.file);
        let workspace = file.and_then(|i| project.workspace(i));
        let ignored = match f.kind.as_str() {
            "unused_dependency"
            | "unused_dev_dependency"
            | "unlisted_dependency"
            | "optional_peer_dependency" => project
                .config
                .ignore_dependencies
                .iter()
                .chain(workspace.into_iter().flat_map(|w| &w.ignore_dependencies))
                .any(|p| matches_pattern(&f.name, p)),
            "unlisted_binary" => project
                .config
                .ignore_binaries
                .iter()
                .any(|p| matches_pattern(&f.name, p)),
            "unresolved" => project
                .config
                .ignore_unresolved
                .iter()
                .chain(workspace.into_iter().flat_map(|w| &w.ignore_unresolved))
                .any(|p| matches_pattern(&f.name, p)),
            "unused_enum_member" | "unused_namespace_member" | "unused_class_member" => project
                .config
                .ignore_members
                .iter()
                .any(|p| matches_pattern(&f.name, p)),
            _ => false,
        };
        if ignored
            || project
                .config
                .ignore_issues
                .get(&f.file)
                .is_some_and(|types| types.is_empty() || types.contains(&f.kind))
            || project.config.ignore_issues.iter().any(|(p, types)| {
                matches_pattern(&f.file, p) && (types.is_empty() || types.contains(&f.kind))
            })
        {
            return false;
        }
        if let Some(index) = file {
            let source = &project.files[index].source;
            let lines: Vec<_> = source.lines().collect();
            let line = f.line.saturating_sub(1) as usize;
            let previous = lines.get(line.wrapping_sub(1)).copied().unwrap_or("");
            let current = lines.get(line).copied().unwrap_or("");
            if [previous, current].iter().any(|l| {
                l.contains("deadwood-ignore")
                    && (l.contains(&f.kind) || !l.contains("deadwood-ignore:"))
            }) {
                return false;
            }
            for tag in &project.config.tags {
                let tag_name = tag.trim_start_matches(['-', '+', '@']);
                let tagged = lines[..line.min(lines.len())]
                    .iter()
                    .rev()
                    .take_while(|l| {
                        l.trim().is_empty()
                            || l.trim().starts_with('*')
                            || l.trim().starts_with('/')
                    })
                    .any(|l| l.contains(&format!("@{tag_name}")));
                if tag.starts_with('-') && tagged || !tag.starts_with('-') && !tagged {
                    return false;
                }
            }
        }
        true
    });
}
fn configuration_hints(
    project: &Project,
    _findings: &[Finding],
    diagnostics: &mut Vec<OutputDiagnostic>,
) {
    for pattern in &project.config.exclude {
        if !pattern.contains('*') && !project.root.join(pattern).exists() {
            diagnostics.push(OutputDiagnostic {
                kind: "configuration_hint".into(),
                message: format!("exclude matches no path: {pattern}"),
            });
        }
    }
}
fn output_diagnostic(d: &Diagnostic) -> OutputDiagnostic {
    OutputDiagnostic {
        kind: d.kind.clone(),
        message: d.message.clone(),
    }
}
pub fn render_json(mut report: Report, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return serde_json::to_string(&report).expect("report is serializable");
    }

    loop {
        let encoded = serde_json::to_string(&report).expect("report is serializable");
        if encoded.len().div_ceil(4) <= max_tokens
            || report.findings.is_empty()
                && report.traces.is_empty()
                && report.diagnostics.is_empty()
        {
            if encoded.len().div_ceil(4) <= max_tokens {
                return encoded;
            }
            let minimal = json!({
                "v": report.v,
                "ok": report.ok,
                "complete": report.complete,
                "dropped": {"not_shown": report.dropped.not_shown + report.findings.len()}
            })
            .to_string();
            return if minimal.len().div_ceil(4) <= max_tokens {
                minimal
            } else {
                "{}".to_string()
            };
        }
        if report.findings.pop().is_some() {
            report.dropped.not_shown += 1;
        } else if report.traces.pop().is_none() {
            report.diagnostics.pop();
        }
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
    for fix in &report.fixes {
        lines.push(format!(
            "fix {}: {} ({})",
            fix.action,
            fix.file,
            fix.names.join(", ")
        ));
    }
    for trace in &report.traces {
        lines.push(format!(
            "trace {} {} {}: {}",
            trace.kind,
            trace.file,
            trace.name,
            trace.references.join(", ")
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

fn file_findings(project: &Project, graph: &Graph, findings: &mut Vec<Finding>) {
    for (index, file) in project.files.iter().enumerate() {
        let inactive_go = project.config.semantic_go
            && file.language == crate::model::Language::Go
            && !graph
                .semantic
                .iter()
                .filter(|g| g.complete)
                .flat_map(|g| &g.packages)
                .any(|p| p.files.contains(&file.display_path));
        if !inactive_go
            && !graph.reachable[index]
            && (!file.is_test || project.config.include_tests)
        {
            findings.push(finding(
                "unused_file",
                &file.display_path,
                1,
                &file.display_path,
                "high",
                "file is not reachable from an entry point",
                vec![
                    "no reachable imports point to this file".into(),
                    "file is not an entry point".into(),
                ],
            ));
        }
    }
}

fn duplicate_findings(project: &Project, _graph: &Graph, findings: &mut Vec<Finding>) {
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
                    severity: "error".to_string(),
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
}

fn symbol_findings(project: &Project, graph: &Graph, findings: &mut Vec<Finding>) {
    let roots: HashSet<_> = graph.roots.iter().map(|r| r.file).collect();
    for (index, file) in project
        .files
        .iter()
        .enumerate()
        .filter(|(i, _)| graph.reachable[*i])
    {
        if graph.semantic.iter().any(|s| s.language == file.language) {
            continue;
        }
        if file.language.is_typescript() {
            typescript_findings(project, graph, index, roots.contains(&index), findings);
        } else {
            native_findings(project, graph, index, findings);
        }
    }
}
fn typescript_findings(
    project: &Project,
    graph: &Graph,
    index: usize,
    entry: bool,
    findings: &mut Vec<Finding>,
) {
    let file = &project.files[index];
    let used = |index: usize, name: &str| {
        graph
            .used_symbols
            .iter()
            .any(|(i, n)| *i == index && n == name)
    };
    if entry && !resolve::include_entry_exports(project, index) {
        return;
    }
    for export in &file.exports {
        if !used(index, &export.name) {
            findings.push(finding(
                if export.kind == SymbolKind::Type {
                    "unused_type"
                } else {
                    "unused_export"
                },
                &file.display_path,
                export.line,
                &export.name,
                "medium",
                "export has no reachable consumer",
                vec!["exported from a reachable file".into()],
            ));
        }
    }
    for member in &file.class_members {
        if used(index, &member.namespace)
            && !project
                .files
                .iter()
                .enumerate()
                .filter(|(i, _)| graph.reachable[*i])
                .any(|(_, consumer)| {
                    consumer
                        .member_references
                        .iter()
                        .any(|r| r.member == member.name)
                })
        {
            findings.push(member_finding(
                "unused_class_member",
                file.display_path.clone(),
                member.line,
                &member.namespace,
                &member.name,
                "class member has no reachable reference",
            ));
        }
    }
    for member in &file.enum_members {
        if used(index, &member.namespace)
            && !graph
                .used_enum_members
                .iter()
                .any(|(i, ns, n)| *i == index && ns == &member.namespace && n == &member.name)
        {
            findings.push(member_finding(
                "unused_enum_member",
                file.display_path.clone(),
                member.line,
                &member.namespace,
                &member.name,
                "enum member is not referenced",
            ));
        }
    }
    for member in &file.namespace_members {
        if used(index, &member.namespace)
            && !graph
                .used_namespace_members
                .iter()
                .any(|(i, ns, n)| *i == index && ns == &member.namespace && n == &member.name)
        {
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
}
fn native_findings(project: &Project, graph: &Graph, index: usize, findings: &mut Vec<Finding>) {
    let file = &project.files[index];
    let used = |index: usize, name: &str| {
        graph
            .used_symbols
            .iter()
            .any(|(i, n)| *i == index && n == name)
    };
    if file.is_test && !project.config.include_tests {
        return;
    }
    let library = match file.language {
        crate::model::Language::Rust => project.owner(index).is_some_and(|m| m.library.is_some()),
        crate::model::Language::Go => graph.roots.iter().any(|r| {
            r.reason == "Go library API"
                && project.owner(r.file).map(|m| &m.path) == project.owner(index).map(|m| &m.path)
        }),
        _ => false,
    };
    for symbol in &file.symbols {
        if symbol.contract
            || used(index, &symbol.name)
            || matches!(symbol.name.as_str(), "main" | "init" | "tests")
            || symbol.public
                && library
                && public_api(project, index)
                && !resolve::include_entry_exports(project, index)
        {
            continue;
        }
        findings.push(finding(
            "unused_symbol",
            &file.display_path,
            symbol.line,
            &symbol.name,
            "medium",
            "symbol has no reachable reference",
            vec![format!("declared as a {:?} symbol", symbol.kind)],
        ));
    }
}

fn semantic_findings(project: &Project, graph: &Graph, findings: &mut Vec<Finding>) {
    for semantic in &graph.semantic {
        let Ok(live) = semantic.reachable() else {
            continue;
        };
        for declaration in &semantic.declarations {
            if !declaration.reportable || live.contains(&declaration.id) {
                continue;
            }
            let Some((index, file)) = project
                .files
                .iter()
                .enumerate()
                .find(|(_, f)| f.display_path == declaration.span.file)
            else {
                continue;
            };
            if !graph.reachable[index] {
                continue;
            }
            findings.push(finding(
                "unused_symbol",
                &file.display_path,
                declaration.span.line,
                &declaration.name,
                "medium",
                "declaration is not reachable in the selected compiler profile",
                vec![
                    format!("compiler declaration: {}", declaration.id.0),
                    format!(
                        "byte span: {}..{}",
                        declaration.span.start, declaration.span.end
                    ),
                ],
            ));
        }
    }
}
fn dependency_findings(project: &Project, graph: &Graph, findings: &mut Vec<Finding>) {
    for manifest in project.manifests() {
        if manifest.kind == ManifestKind::Go
            && project.config.semantic_go
            && !graph
                .semantic
                .iter()
                .any(|g| g.language == crate::model::Language::Go && g.complete)
        {
            continue;
        }

        let owner_matches = |source| manifest_owns(project, manifest, source);
        for dependency in &manifest.dependencies {
            if !matches!(dependency.kind, "runtime" | "development" | "build")
                || project.config.production && dependency.kind == "development"
                || manifest.kind == ManifestKind::Lua && dependency.name == "lua"
            {
                continue;
            }
            let source_use = graph.package_uses.iter().any(|usage| {
                owner_matches(usage.file)
                    && dependency_matches(
                        project,
                        manifest,
                        usage.file,
                        &dependency.name,
                        &usage.raw,
                    )
            });
            let script_use = manifest
                .binaries
                .iter()
                .any(|bin| binary_provider(manifest, bin).as_deref() == Some(&dependency.name));
            let tool_use = manifest.kind == ManifestKind::Npm
                && project.files.iter().enumerate().any(|(i, f)| {
                    let path = f
                        .path
                        .strip_prefix(manifest.path.parent().unwrap_or(&project.root))
                        .unwrap_or(&f.path)
                        .to_string_lossy();
                    owner_matches(i)
                        && graph.reachable[i]
                        && (dependency.name == "typescript" && f.language.is_typescript()
                            || f.is_test
                                && matches!(
                                    dependency.name.as_str(),
                                    "vitest" | "jest" | "@playwright/test" | "cypress"
                                )
                            || crate::discover::plugin_entry(&dependency.name, &path))
                });
            if source_use || script_use || tool_use {
                continue;
            }
            findings.push(finding(
                if dependency.kind == "development" {
                    "unused_dev_dependency"
                } else {
                    "unused_dependency"
                },
                &display_path(project, &manifest.path),
                1,
                &dependency.name,
                "medium",
                "dependency has no reachable reference",
                vec![format!("declared as {}", dependency.kind)],
            ));
        }
        if manifest.kind == ManifestKind::Npm {
            for binary in &manifest.binaries {
                if binary_provider(manifest, binary).is_none() {
                    findings.push(finding(
                        "unlisted_binary",
                        &display_path(project, &manifest.path),
                        1,
                        binary,
                        "medium",
                        "script invokes an undeclared binary",
                        vec!["found in package scripts".into()],
                    ));
                }
            }
            for peer in &manifest.optional_peers {
                if graph
                    .package_uses
                    .iter()
                    .any(|p| owner_matches(p.file) && p.name == *peer)
                    && !manifest
                        .dependencies
                        .iter()
                        .any(|d| d.name == *peer && matches!(d.kind, "runtime" | "development"))
                {
                    findings.push(finding(
                        "optional_peer_dependency",
                        &display_path(project, &manifest.path),
                        1,
                        peer,
                        "medium",
                        "referenced peer is declared optional",
                        vec!["referenced by reachable code".into()],
                    ));
                }
            }
        }
    }
}

fn unlisted_findings(project: &Project, graph: &Graph, findings: &mut Vec<Finding>) {
    let mut unlisted = HashSet::new();
    for usage in &graph.package_uses {
        let file = &project.files[usage.file];
        if usage.resolved || is_system_import(file.language, &usage.raw) {
            continue;
        }
        let Some(owner) = project.owner(usage.file) else {
            continue;
        };
        if owner.kind == ManifestKind::Cargo
            && owner.package_name.as_deref() == usage.raw.split("::").next()
        {
            continue;
        }
        let declared = project
            .manifests()
            .filter(|m| {
                m.matches(file.language)
                    && (m.path == owner.path
                        || !project.config.strict
                            && owner
                                .path
                                .starts_with(m.path.parent().unwrap_or(&project.root)))
            })
            .any(|manifest| {
                manifest.dependencies.iter().any(|d| {
                    (!project.config.production || d.kind != "development")
                        && dependency_matches(project, manifest, usage.file, &d.name, &usage.raw)
                })
            });
        if !declared && unlisted.insert((usage.file, usage.name.clone())) {
            findings.push(finding(
                "unlisted_dependency",
                &file.display_path,
                usage.line,
                &usage.name,
                "high",
                "import is not declared by its package",
                vec![format!("imported as {}", usage.raw)],
            ));
        }
    }
    for unresolved in &graph.unresolved {
        findings.push(finding(
            "unresolved",
            &project.files[unresolved.file].display_path,
            unresolved.line,
            &unresolved.raw,
            "high",
            "reference could not be resolved",
            vec![unresolved.reason.clone()],
        ));
    }
}

fn catalog_findings(project: &Project, _graph: &Graph, findings: &mut Vec<Finding>) {
    // Catalog references are shared by every workspace, not just the root manifest.
    for manifest in project
        .manifests()
        .filter(|m| m.kind == ManifestKind::Npm && !m.catalog_entries.is_empty())
    {
        let references: HashSet<_> = project
            .manifests()
            .flat_map(|m| &m.catalog_references)
            .map(|r| (&r.namespace, &r.name))
            .collect();
        for entry in &manifest.catalog_entries {
            if !references.contains(&(&entry.namespace, &entry.name)) {
                findings.push(finding(
                    "unused_catalog",
                    &display_path(
                        project,
                        manifest.catalog_path.as_ref().unwrap_or(&manifest.path),
                    ),
                    entry.line,
                    &entry.name,
                    "high",
                    "catalog entry has no package reference",
                    vec![format!("catalog: {}", entry.namespace)],
                ));
            }
        }
    }
    for manifest in project.manifests() {
        for reference in &manifest.catalog_references {
            if !project
                .manifests()
                .flat_map(|m| &m.catalog_entries)
                .any(|e| e.name == reference.name && e.namespace == reference.namespace)
            {
                findings.push(finding(
                    "catalog_reference",
                    &display_path(project, &manifest.path),
                    1,
                    &reference.name,
                    "high",
                    "catalog reference has no entry",
                    vec![format!("catalog: {}", reference.namespace)],
                ));
            }
        }
    }
}

fn cycle_findings(project: &Project, graph: &Graph, findings: &mut Vec<Finding>) {
    if project.config.include.iter().any(|k| k == "cycle")
        || project.config.rules.contains_key("cycle")
    {
        for cycle in &graph.cycles {
            let paths: Vec<_> = cycle
                .iter()
                .map(|i| project.files[*i].display_path.clone())
                .collect();
            findings.push(finding(
                "cycle",
                &paths[0],
                1,
                &paths.join(" -> "),
                "medium",
                "runtime imports form a cycle",
                paths.clone(),
            ));
        }
    }
}

pub(crate) fn manifest_owns(
    project: &Project,
    manifest: &crate::model::PackageManifest,
    source: usize,
) -> bool {
    project
        .owner(source)
        .is_some_and(|owner| owner.path == manifest.path)
        || !project.config.strict
            && project.owner(source).is_some_and(|owner| {
                owner
                    .path
                    .starts_with(manifest.path.parent().unwrap_or(&project.root))
            })
            && manifest.matches(project.files[source].language)
}

#[cfg(test)]
mod tests {
    #[test]
    fn tiny_json_budget_remains_valid() {
        let report = super::Report {
            analysis: Vec::new(),
            fixes: Vec::new(),
            traces: Vec::new(),
            v: 1,
            complete: true,
            ok: true,
            summary: super::Summary {
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
            dropped: Default::default(),
        };
        let output = super::render_json(report, 1);
        serde_json::from_str::<serde_json::Value>(&output).unwrap();
        assert!(output.len().div_ceil(4) <= 1);
    }
}
