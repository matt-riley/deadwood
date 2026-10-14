//! Conservative, backed-up source edits. Planning never writes.
use crate::{
    lang,
    model::{FixAction, Project, Report},
};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tree_sitter::{Node, Parser};
#[derive(Debug)]
pub struct Change {
    pub file: String,
    original: Vec<u8>,
    replacement: Option<Vec<u8>>,
}
#[derive(Debug)]
pub struct Plan {
    pub actions: Vec<FixAction>,
    pub changes: Vec<Change>,
    root: PathBuf,
    stamp: u64,
    cache: Option<PathBuf>,
}
pub fn plan(project: &Project, report: &Report, remove_files: bool) -> Result<Plan, String> {
    plan_with(project, report, remove_files, false)
}
pub fn plan_with(
    project: &Project,
    report: &Report,
    remove_files: bool,
    native: bool,
) -> Result<Plan, String> {
    if !report.complete || report.summary.unresolved > 0 {
        return Err("refusing fixes: analysis is incomplete or has unresolved references".into());
    }
    if crate::cache::snapshot_with(&project.root, project.config.cache.as_deref()) != project.stamp
    {
        return Err("refusing fixes: project changed since analysis".into());
    }
    let mut by_file = BTreeMap::<String, Vec<_>>::new();
    for finding in &report.findings {
        if finding.severity == "error" {
            by_file
                .entry(finding.file.clone())
                .or_default()
                .push(finding);
        }
    }
    let mut plan = Plan {
        root: project.root.clone(),
        stamp: project.stamp,
        cache: project.config.cache.clone(),
        actions: Vec::new(),
        changes: Vec::new(),
    };
    for (file, issues) in by_file {
        let path = checked_path(&project.root, &file)?;
        let original = fs::read(&path).map_err(|e| e.to_string())?;
        if remove_files
            && issues
                .iter()
                .any(|f| f.kind == "unused_file" && f.confidence == "high")
        {
            if project.config.semantic_go
                && lang::from_path(&path) == Some(crate::model::Language::Go)
                && !native
            {
                return Err("Go semantic file removal requires --verify-command".into());
            }
            plan.actions.push(FixAction {
                file: file.clone(),
                action: "remove_file".into(),
                names: vec![file.clone()],
            });
            plan.changes.push(Change {
                file,
                original,
                replacement: None,
            });
            continue;
        }
        let replacement = replacement_for(project, &path, &file, &original, &issues, native)?;
        if let Some(replacement) = replacement.filter(|r| r != &original) {
            plan.actions.push(FixAction {
                file: file.clone(),
                action: "edit".into(),
                names: issues
                    .iter()
                    .filter(|f| {
                        matches!(
                            f.kind.as_str(),
                            "unused_dependency"
                                | "unused_dev_dependency"
                                | "unused_catalog"
                                | "unused_export"
                                | "unused_type"
                        )
                    })
                    .map(|f| f.name.clone())
                    .collect(),
            });
            plan.changes.push(Change {
                file,
                original,
                replacement: Some(replacement),
            });
        }
    }
    Ok(plan)
}
fn replacement_for(
    project: &Project,
    path: &Path,
    file: &str,
    original: &[u8],
    issues: &[&crate::model::Finding],
    native: bool,
) -> Result<Option<Vec<u8>>, String> {
    let names: HashSet<_> = issues
        .iter()
        .filter(|f| {
            matches!(
                f.kind.as_str(),
                "unused_dependency" | "unused_dev_dependency" | "unused_catalog"
            )
        })
        .map(|f| f.name.as_str())
        .collect();
    Ok(if path.file_name().is_some_and(|n| n == "package.json") {
        fix_package_json(original, issues)?
    } else if path.file_name().is_some_and(|n| n == "pnpm-workspace.yaml") && !names.is_empty() {
        Some(
            fix_catalog(
                std::str::from_utf8(original).map_err(|e| e.to_string())?,
                issues,
            )?
            .into_bytes(),
        )
    } else if native
        && !names.is_empty()
        && (path
            .file_name()
            .is_some_and(|n| n == "Cargo.toml" || n == "go.mod")
            || path.extension().is_some_and(|n| n == "rockspec"))
    {
        Some(
            fix_native_manifest(
                path,
                std::str::from_utf8(original).map_err(|e| e.to_string())?,
                &names,
            )?
            .into_bytes(),
        )
    } else if lang::from_path(path).is_some_and(|l| l.is_typescript())
        && !project.files.iter().any(|f| f.path == path && f.compiled)
    {
        let names: HashSet<_> = issues
            .iter()
            .filter(|f| matches!(f.kind.as_str(), "unused_export" | "unused_type"))
            .map(|f| f.name.as_str())
            .collect();
        if names.is_empty() {
            None
        } else {
            Some(
                fix_exports(
                    std::str::from_utf8(original).map_err(|e| e.to_string())?,
                    file,
                    &names,
                )?
                .into_bytes(),
            )
        }
    } else {
        None
    })
}
fn checked_path(root: &Path, file: &str) -> Result<PathBuf, String> {
    let relative = Path::new(file);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("unsafe fix path: {file}"));
    }
    let path = root.join(relative);
    let canonical = fs::canonicalize(&path).map_err(|e| e.to_string())?;
    if canonical != path || !canonical.starts_with(root) {
        return Err(format!("refusing symlink or escaped fix target: {file}"));
    }
    Ok(path)
}
fn apply_edits(source: &str, edits: &[(std::ops::Range<usize>, String)]) -> Result<String, String> {
    let mut result = source.to_string();
    let mut edits = edits.to_vec();
    edits.sort_by_key(|(range, _)| range.start);
    if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
        return Err("overlapping source fixes".into());
    }
    for (range, replacement) in edits.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    Ok(result)
}
fn fix_exports(source: &str, file: &str, unused: &HashSet<&str>) -> Result<String, String> {
    let record = crate::parse::parse(PathBuf::from(file), file.into(), source);
    let mut parser = Parser::new();
    parser
        .set_language(&lang::grammar(record.language))
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    let mut edits = Vec::new();
    let mut c = tree.root_node().walk();
    for node in tree
        .root_node()
        .named_children(&mut c)
        .filter(|n| n.kind() == "export_statement")
    {
        if node.child_by_field_name("source").is_some() {
            continue;
        }
        let raw = &source[node.byte_range()];
        let line = node.start_position().row as u32 + 1;
        let exports: Vec<_> = record.exports.iter().filter(|e| e.line == line).collect();
        if exports.is_empty() {
            continue;
        }
        if let Some(clause) = named_child(node, "export_clause") {
            let mut cursor = clause.walk();
            let members: Vec<_> = clause
                .named_children(&mut cursor)
                .filter(|n| n.kind() == "export_specifier")
                .collect();
            let kept: Vec<_> = members
                .iter()
                .filter(|n| {
                    let name = n
                        .child_by_field_name("alias")
                        .or_else(|| n.child_by_field_name("name"))
                        .map(|n| source[n.byte_range()].trim_matches(['\'', '"']))
                        .unwrap_or("");
                    !unused.contains(name)
                })
                .map(|n| &source[n.byte_range()])
                .collect();
            if kept.len() == members.len() {
                continue;
            }
            let replacement = if kept.is_empty() {
                String::new()
            } else {
                format!("{{ {} }}", kept.join(", "))
            };
            edits.push((
                if kept.is_empty() {
                    node.byte_range()
                } else {
                    clause.byte_range()
                },
                replacement,
            ));
        } else if exports.iter().all(|e| unused.contains(e.name.as_str())) {
            if let Some(declaration) = node.child_by_field_name("declaration") {
                // Anonymous default declarations cannot safely become local declarations.
                if raw.trim_start().starts_with("export default")
                    && declaration.child_by_field_name("name").is_none()
                {
                    continue;
                }
                edits.push((node.start_byte()..declaration.start_byte(), String::new()));
            } else if let Some(value) = node.child_by_field_name("value") {
                // Retain evaluation of default expressions; skip anonymous functions/classes.
                if matches!(value.kind(), "function_expression" | "class") {
                    continue;
                }
                edits.push((node.start_byte()..value.start_byte(), String::new()));
            }
        }
    }
    let result = apply_edits(source, &edits)?;
    if crate::parse::parse(PathBuf::from(file), file.into(), &result).parse_error {
        return Err(format!("fix would introduce syntax errors: {file}"));
    }
    Ok(result)
}
fn named_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut c = node.walk();
    node.named_children(&mut c).find(|n| n.kind() == kind)
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("deadwood-{}-tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            fs::set_permissions(&tmp, metadata.permissions()).map_err(|e| e.to_string())?;
        }
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    let _ = fs::remove_file(&tmp);
    result
}
pub fn apply(plan: &Plan, verify: Option<&str>) -> Result<Option<PathBuf>, String> {
    if crate::cache::snapshot_with(&plan.root, plan.cache.as_deref()) != plan.stamp {
        return Err("refusing fixes: stale project snapshot".into());
    }
    for change in &plan.changes {
        let path = checked_path(&plan.root, &change.file)?;
        if fs::read(path).map_err(|e| e.to_string())? != change.original {
            return Err(format!("refusing stale fix: {}", change.file));
        }
    }
    if plan.changes.is_empty() {
        return Ok(None);
    }
    let backup_root = plan.root.join(".deadwood-backup");
    if fs::symlink_metadata(&backup_root).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("backup directory must not be a symlink".into());
    }
    fs::create_dir_all(&backup_root).map_err(|e| e.to_string())?;
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let backup = backup_root.join(format!("{id}-{}", std::process::id()));
    fs::create_dir(&backup).map_err(|e| e.to_string())?;
    for change in &plan.changes {
        let saved = backup.join(&change.file);
        fs::create_dir_all(saved.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(plan.root.join(&change.file), saved).map_err(|e| e.to_string())?;
    }
    fs::write(
        backup.join("changes.json"),
        serde_json::to_vec_pretty(&plan.actions).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut applied = Vec::new();
    let result = (|| {
        for change in &plan.changes {
            let path = checked_path(&plan.root, &change.file)?;
            if fs::read(&path).map_err(|e| e.to_string())? != change.original {
                return Err(format!("file changed during fix: {}", change.file));
            }
            if let Some(bytes) = &change.replacement {
                atomic_write(&path, bytes)?;
            } else {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
            applied.push(change);
        }
        if let Some(command) = verify {
            crate::output::command(command, "", &plan.root)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        for change in applied.into_iter().rev() {
            let path = plan.root.join(&change.file);
            let current = fs::read(&path).ok();
            if current != change.replacement {
                return Err(format!(
                    "{error}; rollback refused because {} changed externally; backup: {}",
                    change.file,
                    backup.display()
                ));
            }
            atomic_write(&path, &change.original)
                .map_err(|e| format!("{error}; rollback: {e}; backup: {}", backup.display()))?;
        }
        return Err(format!(
            "fix rolled back: {error}; backup: {}",
            backup.display()
        ));
    }
    Ok(Some(backup))
}

fn fix_native_manifest(
    path: &Path,
    source: &str,
    unused: &HashSet<&str>,
) -> Result<String, String> {
    if path.file_name().is_some_and(|n| n == "Cargo.toml") {
        let value: toml::Value = toml::from_str(source).map_err(|e| e.to_string())?;
        let protected: HashSet<_> = value
            .get("features")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flat_map(|m| m.values())
            .filter_map(toml::Value::as_array)
            .flatten()
            .filter_map(toml::Value::as_str)
            .map(|s| {
                s.trim_start_matches("dep:")
                    .split('/')
                    .next()
                    .unwrap_or(s)
                    .trim_end_matches('?')
                    .replace('-', "_")
            })
            .collect();
        let mut document: toml_edit::DocumentMut = source
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| e.to_string())?;
        fn remove(
            table: &mut toml_edit::Table,
            unused: &HashSet<&str>,
            protected: &HashSet<String>,
        ) {
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(deps) = table
                    .get_mut(section)
                    .and_then(toml_edit::Item::as_table_mut)
                {
                    let names: Vec<_> = deps
                        .iter()
                        .map(|(n, _)| n.to_string())
                        .filter(|name| {
                            unused.contains(name.replace('-', "_").as_str())
                                && !protected.contains(&name.replace('-', "_"))
                        })
                        .collect();
                    for name in names {
                        deps.remove(&name);
                    }
                }
            }
        }
        remove(document.as_table_mut(), unused, &protected);
        if let Some(targets) = document
            .get_mut("target")
            .and_then(toml_edit::Item::as_table_mut)
        {
            for (_, target) in targets.iter_mut() {
                if let Some(table) = target.as_table_mut() {
                    remove(table, unused, &protected);
                }
            }
        }
        return Ok(document.to_string());
    }
    if path.file_name().is_some_and(|n| n == "go.mod") {
        let mut block = false;
        let mut output = String::new();
        for line in source.split_inclusive('\n') {
            let text = line.trim();
            if text.starts_with("require (") {
                block = true;
                output.push_str(line);
                continue;
            }
            if block && text == ")" {
                block = false;
                output.push_str(line);
                continue;
            }
            let dep = if block {
                Some(text)
            } else {
                text.strip_prefix("require ")
            };
            if dep
                .and_then(|s| s.split_whitespace().next())
                .is_some_and(|name| unused.contains(name))
            {
                continue;
            }
            output.push_str(line);
        }
        return Ok(output);
    }
    let mut parser = Parser::new();
    parser
        .set_language(&lang::grammar(crate::model::Language::Lua))
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    let mut edits = Vec::new();
    fn tables<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
        if node.kind() == "assignment_statement" {
            out.push(node);
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            tables(child, out);
        }
    }
    let mut assignments = Vec::new();
    tables(tree.root_node(), &mut assignments);
    for assignment in assignments {
        if !assignment
            .named_child(0)
            .is_some_and(|n| source[n.byte_range()].trim() == "dependencies")
        {
            continue;
        }
        let table = assignment
            .named_child(1)
            .and_then(|n| n.named_child(0))
            .filter(|n| n.kind() == "table_constructor");
        if let Some(table) = table {
            let mut c = table.walk();
            let fields: Vec<_> = table
                .named_children(&mut c)
                .filter(|n| n.kind() == "field")
                .collect();
            if fields.iter().any(|f| {
                f.child_by_field_name("name").is_some()
                    || f.child_by_field_name("value")
                        .is_none_or(|v| v.kind() != "string")
            }) {
                continue;
            }
            let kept: Vec<_> = fields
                .iter()
                .filter(|field| {
                    let raw = field
                        .child_by_field_name("value")
                        .map(|n| source[n.byte_range()].trim_matches(['\'', '"']))
                        .unwrap_or("");
                    !raw.split_whitespace()
                        .next()
                        .is_some_and(|name| unused.contains(name))
                })
                .map(|n| &source[n.byte_range()])
                .collect();
            if kept.len() != fields.len() {
                edits.push((table.byte_range(), format!("{{ {} }}", kept.join(", "))));
            }
        }
    }
    let result = apply_edits(source, &edits)?;
    if parser.parse(&result, None).unwrap().root_node().has_error() {
        return Err("rockspec fix introduced syntax errors".into());
    }
    Ok(result)
}

fn fix_package_json(
    original: &[u8],
    issues: &[&crate::model::Finding],
) -> Result<Option<Vec<u8>>, String> {
    let mut v: serde_json::Value = serde_json::from_slice(original).map_err(|e| e.to_string())?;
    let dependencies: HashSet<_> = issues
        .iter()
        .filter(|f| {
            matches!(
                f.kind.as_str(),
                "unused_dependency" | "unused_dev_dependency"
            )
        })
        .map(|f| f.name.as_str())
        .collect();
    let catalogs: HashSet<_> = issues
        .iter()
        .filter(|f| f.kind == "unused_catalog")
        .filter_map(|f| {
            f.evidence
                .first()
                .and_then(|e| e.strip_prefix("catalog: "))
                .map(|ns| (ns, f.name.as_str()))
        })
        .collect();
    for key in ["dependencies", "devDependencies"] {
        if let Some(m) = v.get_mut(key).and_then(serde_json::Value::as_object_mut) {
            m.retain(|name, _| !dependencies.contains(name.as_str()));
        }
    }
    for key in ["catalog", "catalogs"] {
        if let Some(m) = v.get_mut(key).and_then(serde_json::Value::as_object_mut) {
            if key == "catalog" {
                m.retain(|name, _| !catalogs.contains(&("default", name.as_str())));
            } else {
                for (namespace, catalog) in m.iter_mut() {
                    if let Some(catalog) = catalog.as_object_mut() {
                        catalog.retain(|name, _| {
                            !catalogs.contains(&(namespace.as_str(), name.as_str()))
                        });
                    }
                }
            }
        }
    }
    Ok(if dependencies.is_empty() && catalogs.is_empty() {
        None
    } else {
        Some(
            format!(
                "{}\n",
                serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
            )
            .into_bytes(),
        )
    })
}

fn fix_catalog(source: &str, issues: &[&crate::model::Finding]) -> Result<String, String> {
    let lines: Vec<_> = source.split_inclusive('\n').collect();
    let mut offset = 0;
    let mut edits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        for issue in issues {
            if issue.kind == "unused_catalog"
                && issue.line as usize == i + 1
                && line
                    .trim()
                    .split_once(':')
                    .is_some_and(|(key, _)| key.trim_matches(['\'', '"']) == issue.name)
            {
                edits.push((offset..offset + line.len(), String::new()));
            }
        }
        offset += line.len();
    }
    apply_edits(source, &edits)
}
