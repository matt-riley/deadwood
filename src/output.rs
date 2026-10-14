use crate::{
    model::{Graph, Project, Report, Trace},
    report, resolve,
};
use serde_json::json;
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub fn traces(
    project: &Project,
    graph: &Graph,
    file: Option<&str>,
    export: Option<&str>,
    dependency: Option<&str>,
) -> Vec<Trace> {
    let mut result = Vec::new();
    if dependency.is_none() {
        for (index, record) in project.files.iter().enumerate() {
            if file.is_some_and(|p| !crate::discover::matches_pattern(&record.display_path, p)) {
                continue;
            }
            if export.is_none() {
                let mut references: Vec<_> = graph
                    .edges
                    .iter()
                    .enumerate()
                    .filter(|(_, edges)| edges.contains(&index))
                    .map(|(i, _)| project.files[i].display_path.clone())
                    .collect();
                references.extend(
                    graph
                        .roots
                        .iter()
                        .filter(|r| r.file == index)
                        .map(|r| format!("entry: {}", r.reason)),
                );
                result.push(Trace {
                    kind: "file".into(),
                    file: record.display_path.clone(),
                    name: record.display_path.clone(),
                    references,
                });
            }
            for semantic in &graph.semantic {
                if !semantic.complete {
                    continue;
                }
                for declaration in semantic
                    .declarations
                    .iter()
                    .filter(|d| d.reportable && d.span.file == record.display_path)
                {
                    if export.is_some_and(|name| name != declaration.name) {
                        continue;
                    }
                    let mut references: Vec<_> = semantic
                        .references
                        .iter()
                        .filter(|r| r.to == declaration.id)
                        .map(|r| {
                            format!(
                                "{}:{} ({})",
                                r.span.file,
                                r.span.line,
                                semantic
                                    .declarations
                                    .iter()
                                    .find(|d| d.id == r.from)
                                    .map_or("unknown", |d| d.name.as_str())
                            )
                        })
                        .collect();
                    references.extend(
                        semantic
                            .roots
                            .iter()
                            .filter(|r| r.symbol == declaration.id)
                            .map(|r| format!("entry: {}", r.reason)),
                    );
                    references.sort();
                    references.dedup();
                    result.push(Trace {
                        kind: "symbol".into(),
                        file: record.display_path.clone(),
                        name: declaration.name.clone(),
                        references,
                    });
                }
            }
            if project.config.semantic_go && record.language == crate::model::Language::Go {
                continue;
            }
            for symbol in &record.exports {
                if export.is_some_and(|name| symbol.name != name) {
                    continue;
                }
                let mut references = Vec::new();
                for (source, consumer) in project.files.iter().enumerate() {
                    for import in &consumer.symbol_imports {
                        if import
                            .bindings
                            .iter()
                            .any(|b| b.imported == symbol.name || b.imported == "*")
                            && resolve::resolve_import(
                                project,
                                source,
                                &import.raw,
                                crate::model::ImportKind::Module,
                            )
                            .contains(&index)
                        {
                            references.push(consumer.display_path.clone());
                        }
                    }
                }
                result.push(Trace {
                    kind: "export".into(),
                    file: record.display_path.clone(),
                    name: symbol.name.clone(),
                    references,
                });
            }
        }
    }
    if file.is_none() && export.is_none() {
        for manifest in project.manifests() {
            for dep in &manifest.dependencies {
                if dependency.is_some_and(|name| name != dep.name) {
                    continue;
                }
                result.push(Trace {
                    kind: "dependency".into(),
                    file: manifest
                        .path
                        .strip_prefix(&project.root)
                        .unwrap_or(&manifest.path)
                        .to_string_lossy()
                        .into(),
                    name: dep.name.clone(),
                    references: graph
                        .package_uses
                        .iter()
                        .filter(|u| {
                            report::manifest_owns(project, manifest, u.file)
                                && report::dependency_matches(
                                    project, manifest, u.file, &dep.name, &u.raw,
                                )
                        })
                        .map(|u| format!("{}:{}", project.files[u.file].display_path, u.line))
                        .collect(),
                });
            }
        }
    }
    result
}
pub fn sarif(report: &Report) -> String {
    let rules: Vec<_> = report::ISSUE_TYPES
        .iter()
        .map(|id| json!({"id":id,"name":id,"shortDescription":{"text":id.replace('_'," ")}}))
        .collect();
    let results:Vec<_>=report.findings.iter().map(|f|json!({
        "ruleId":f.kind,"level":if f.severity=="warn"{"warning"}else{"error"},
        "message":{"text":format!("{}: {}",f.name,f.message)},
        "locations":[{"physicalLocation":{"artifactLocation":{"uri":uri(&f.file),"uriBaseId":"%SRCROOT%"},"region":{"startLine":f.line.max(1)}}}],
        "properties":{"confidence":f.confidence,"evidence":f.evidence}
    })).collect();
    json!({"$schema":"https://json.schemastore.org/sarif-2.1.0.json","version":"2.1.0","runs":[{
        "tool":{"driver":{"name":"deadwood","version":env!("CARGO_PKG_VERSION"),"rules":rules}},
        "results":results,"invocations":[{"executionSuccessful":report.complete,
            "toolExecutionNotifications":report.diagnostics.iter().map(|d|json!({"level":"warning","message":{"text":d.message}})).collect::<Vec<_>>()}]
    }]}).to_string()
}
fn uri(path: &str) -> String {
    path.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn github(report: &Report) -> String {
    let escape = |s: &str| {
        s.replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A")
    };
    report
        .findings
        .iter()
        .map(|f| {
            format!(
                "::{} file={},line={},title={}::{}\n",
                if f.severity == "warn" {
                    "warning"
                } else {
                    "error"
                },
                escape(&f.file).replace(',', "%2C").replace(':', "%3A"),
                f.line.max(1),
                escape(&f.kind),
                escape(&format!("{}: {}", f.name, f.message))
            )
        })
        .collect()
}
pub fn markdown(report: &Report) -> String {
    let escape = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let mut output = "| Rule | File | Name | Severity |\n|---|---|---|---|\n".to_string();
    for f in &report.findings {
        output.push_str(&format!(
            "| {} | {}:{} | {} | {} |\n",
            f.kind,
            escape(&f.file),
            f.line,
            escape(&f.name),
            f.severity
        ));
    }
    output
}
pub fn command(command: &str, input: &str, root: &std::path::Path) -> Result<String, String> {
    let words = shlex::split(command).ok_or("invalid command quoting")?;
    let (program, args) = words.split_first().ok_or("empty command")?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("command stdin unavailable")?;
    // Write on a separate thread so a command producing output before consuming stdin cannot deadlock.
    let bytes = input.as_bytes().to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&bytes));
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let written = writer.join().map_err(|_| "command stdin writer failed")?;
    if !output.status.success() {
        return Err(format!(
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if let Err(e) = written
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(e.to_string());
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}
pub fn preprocess(
    report: Report,
    commands: &[String],
    root: &std::path::Path,
) -> Result<Report, String> {
    let mut report = report;
    for cmd in commands {
        let result = command(
            cmd,
            &serde_json::to_string(&report).map_err(|e| e.to_string())?,
            root,
        )?;
        report = serde_json::from_str(&result)
            .map_err(|e| format!("invalid preprocessor report: {e}"))?;
        if report.v != 1
            || report.findings.iter().any(|f| {
                !report::ISSUE_TYPES.contains(&f.kind.as_str())
                    || !matches!(f.severity.as_str(), "error" | "warn")
            })
        {
            return Err("invalid preprocessor report schema".into());
        }
    }
    Ok(report)
}
