use crate::lang;
use crate::model::{
    EnumMember, ExportAlias, ExportSymbol, FileRecord, Import, ImportKind, Language,
    MemberReference, NamespaceMember, SymbolBinding, SymbolDefinition, SymbolImport, SymbolKind,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

pub fn parse(path: PathBuf, display_path: String, source: &str) -> FileRecord {
    let language = lang::from_path(Path::new(&display_path)).expect("parse called for source path");
    let mut parser = Parser::new();
    parser
        .set_language(&lang::grammar(language))
        .expect("supported grammar must load");
    let tree = parser.parse(source, None).expect("parser returns a tree");

    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut export_aliases = Vec::new();
    let mut symbols = Vec::new();
    let mut symbol_references = Vec::new();
    let mut symbol_imports = Vec::new();
    let mut member_references = Vec::new();
    let mut enum_members = Vec::new();
    let mut namespace_members = Vec::new();
    let mut package = None;
    let mut has_main = false;
    walk(
        tree.root_node(),
        source.as_bytes(),
        language,
        &mut imports,
        &mut exports,
        &mut export_aliases,
        &mut symbols,
        &mut symbol_references,
        &mut symbol_imports,
        &mut member_references,
        &mut enum_members,
        &mut namespace_members,
        &mut package,
        &mut has_main,
    );

    if language == Language::Rust {
        filter_rust_scoped_imports(&mut imports, source);
    }

    FileRecord {
        path,
        display_path: display_path.clone(),
        language,
        imports,
        exports,
        export_aliases,
        symbols,
        symbol_references,
        symbol_imports,
        member_references,
        enum_members,
        namespace_members,
        package,
        has_main,
        parse_error: tree.root_node().has_error(),
        is_test: is_test_path(&display_path),
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    node: Node<'_>,
    source: &[u8],
    language: Language,
    imports: &mut Vec<Import>,
    exports: &mut Vec<ExportSymbol>,
    export_aliases: &mut Vec<ExportAlias>,
    symbols: &mut Vec<SymbolDefinition>,
    symbol_references: &mut Vec<String>,
    symbol_imports: &mut Vec<SymbolImport>,
    member_references: &mut Vec<MemberReference>,
    enum_members: &mut Vec<EnumMember>,
    namespace_members: &mut Vec<NamespaceMember>,
    package: &mut Option<String>,
    has_main: &mut bool,
) {
    match language {
        Language::TypeScript | Language::Tsx => visit_typescript(
            node,
            source,
            imports,
            exports,
            export_aliases,
            symbol_references,
            symbol_imports,
            member_references,
            enum_members,
            namespace_members,
        ),
        Language::Go => visit_go(
            node,
            source,
            imports,
            symbols,
            symbol_references,
            member_references,
            package,
            has_main,
        ),
        Language::Rust => visit_rust(
            node,
            source,
            imports,
            symbols,
            symbol_references,
            member_references,
            has_main,
        ),
        Language::Lua => visit_lua(
            node,
            source,
            imports,
            symbols,
            symbol_references,
            member_references,
        ),
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(
            child,
            source,
            language,
            imports,
            exports,
            export_aliases,
            symbols,
            symbol_references,
            symbol_imports,
            member_references,
            enum_members,
            namespace_members,
            package,
            has_main,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_typescript(
    node: Node<'_>,
    source: &[u8],
    imports: &mut Vec<Import>,
    exports: &mut Vec<ExportSymbol>,
    export_aliases: &mut Vec<ExportAlias>,
    symbol_references: &mut Vec<String>,
    symbol_imports: &mut Vec<SymbolImport>,
    member_references: &mut Vec<MemberReference>,
    enum_members: &mut Vec<EnumMember>,
    namespace_members: &mut Vec<NamespaceMember>,
) {
    if matches!(node.kind(), "identifier" | "type_identifier") {
        symbol_references.push(text(node, source));
    }

    if node.kind() == "member_expression"
        && let (Some(object), Some(property)) = (
            node.child_by_field_name("object"),
            node.child_by_field_name("property"),
        )
    {
        member_references.push(MemberReference {
            namespace: text(object, source),
            member: text(property, source),
        });
    }

    if node.kind() == "export_statement"
        && node
            .parent()
            .is_some_and(|parent| parent.kind() == "program")
    {
        let source_text = text(node, source);
        let kind = if is_type_export(&source_text) {
            SymbolKind::Type
        } else {
            SymbolKind::Value
        };
        for name in export_names(node, source) {
            exports.push(ExportSymbol {
                name,
                line: node.start_position().row as u32 + 1,
                kind,
            });
        }
        collect_declared_members(
            &source_text,
            node.start_position().row as u32 + 1,
            enum_members,
            namespace_members,
        );
        export_aliases.extend(export_aliases_in(
            &source_text,
            node.start_position().row as u32 + 1,
        ));
    }

    if matches!(node.kind(), "import_statement" | "export_statement")
        && let Some(source_node) = node.child_by_field_name("source")
        && let Some(raw) = string_literal(source_node, source)
    {
        let kind = if is_relative(&raw) {
            ImportKind::Module
        } else {
            ImportKind::Package
        };
        imports.push(Import {
            alias: None,
            raw: raw.clone(),
            line: node.start_position().row as u32 + 1,
            kind,
        });
        let bindings = symbol_bindings(node, source);
        if !bindings.is_empty() {
            symbol_imports.push(SymbolImport { raw, bindings });
        }
    }

    if node.kind() == "call_expression"
        && let Some(function) = node.child_by_field_name("function")
    {
        let name = text(function, source);
        if matches!(name.as_str(), "require" | "import")
            && let Some(arguments) = node.child_by_field_name("arguments")
            && let Some(raw) = first_string_child(arguments, source)
        {
            imports.push(Import {
                alias: None,
                kind: if is_relative(&raw) {
                    ImportKind::Module
                } else {
                    ImportKind::Package
                },
                raw,
                line: node.start_position().row as u32 + 1,
            });
        }
    }
}

fn export_aliases_in(source: &str, line: u32) -> Vec<ExportAlias> {
    let body = source.trim().strip_prefix("export").unwrap_or("").trim();
    if let Some(body) = body.strip_prefix("default ") {
        let source = body.trim_end_matches(';').trim();
        if is_identifier(source) {
            return vec![ExportAlias {
                source: source.to_string(),
                alias: "default".to_string(),
                line,
            }];
        }
    }
    if body.starts_with('{') {
        let Some(close) = body.find('}') else {
            return Vec::new();
        };
        return body[1..close]
            .split(',')
            .filter_map(|item| {
                let mut parts = item.trim().split(" as ").map(str::trim);
                let source = parts.next()?;
                let alias = parts.next()?;
                (is_identifier(source) && is_identifier(alias)).then_some(ExportAlias {
                    source: source.to_string(),
                    alias: alias.to_string(),
                    line,
                })
            })
            .collect();
    }
    for keyword in ["const", "let", "var"] {
        if let Some(rest) = body.strip_prefix(keyword) {
            let Some((left, right)) = rest.split_once('=') else {
                return Vec::new();
            };
            let Some(alias) = left.trim().split(':').next() else {
                return Vec::new();
            };
            let alias = alias.trim();
            let source = right.trim().trim_end_matches(';').trim();
            if is_identifier(alias) && is_identifier(source) {
                return vec![ExportAlias {
                    source: source.to_string(),
                    alias: alias.to_string(),
                    line,
                }];
            }
        }
    }
    Vec::new()
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric() || ch == '$')
}

fn symbol_bindings(node: Node<'_>, source: &[u8]) -> Vec<SymbolBinding> {
    let raw = text(node, source);
    let keyword = if node.kind() == "import_statement" {
        "import"
    } else {
        "export"
    };
    let body = raw
        .strip_prefix(keyword)
        .unwrap_or(&raw)
        .split(" from ")
        .next()
        .unwrap_or("")
        .trim();
    if let Some(body) = body.strip_prefix("type ") {
        return symbol_bindings_from_body(node.kind(), body.trim());
    }
    symbol_bindings_from_body(node.kind(), body)
}

fn symbol_bindings_from_body(node_kind: &str, body: &str) -> Vec<SymbolBinding> {
    if body.starts_with('*') {
        let local = body.split(" as ").nth(1).map(str::trim).unwrap_or("*");
        return vec![SymbolBinding {
            imported: "*".to_string(),
            local: local.to_string(),
        }];
    }
    let mut bindings = Vec::new();
    if let (Some(open), Some(close)) = (body.find('{'), body.find('}')) {
        if node_kind == "import_statement" {
            let default = body[..open].trim().trim_end_matches(',').trim();
            if !default.is_empty() {
                bindings.push(SymbolBinding {
                    imported: "default".to_string(),
                    local: default.to_string(),
                });
            }
        }
        bindings.extend(body[open + 1..close].split(',').filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let item = item.strip_prefix("type ").unwrap_or(item).trim();
            let mut parts = item.split(" as ").map(str::trim);
            let imported = parts.next()?.to_string();
            let local = parts.next().unwrap_or(&imported).to_string();
            Some(SymbolBinding { imported, local })
        }));
        return bindings;
    }
    if node_kind == "import_statement"
        && !body.is_empty()
        && !body.starts_with('"')
        && !body.starts_with('\'')
    {
        let local = body.split(',').next().unwrap_or(body).trim();
        bindings.push(SymbolBinding {
            imported: "default".to_string(),
            local: local.to_string(),
        });
    }
    bindings
}

fn is_type_export(source: &str) -> bool {
    let body = source.trim().strip_prefix("export").unwrap_or("").trim();
    let body = body.strip_prefix("default").unwrap_or(body).trim();
    ["type ", "interface ", "enum "]
        .iter()
        .any(|prefix| body.starts_with(prefix))
}

fn collect_declared_members(
    source: &str,
    line: u32,
    enum_members: &mut Vec<EnumMember>,
    namespace_members: &mut Vec<NamespaceMember>,
) {
    for keyword in ["enum", "namespace"] {
        let Some((name, body)) = declaration_body(source, keyword) else {
            continue;
        };
        if keyword == "enum" {
            let namespace = name;
            for item in body.split(',') {
                let member_name = item
                    .split(['=', '\n', ';'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches("/*")
                    .trim();
                if !member_name.is_empty()
                    && member_name
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                {
                    enum_members.push(EnumMember {
                        namespace: namespace.clone(),
                        name: member_name.to_string(),
                        line,
                    });
                }
            }
        } else {
            for member in top_level_exports(&body) {
                namespace_members.push(NamespaceMember {
                    namespace: name.clone(),
                    name: member,
                    line,
                });
            }
        }
    }
}

fn declaration_body(source: &str, keyword: &str) -> Option<(String, String)> {
    let start = source.find(keyword)?;
    let after = &source[start + keyword.len()..];
    let name = after
        .trim_start()
        .split([' ', '{', '<'])
        .next()?
        .to_string();
    let open = source[start..].find('{')? + start;
    let mut depth = 0;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((name, source[open + 1..open + offset].to_string()));
                }
            }
            _ => {}
        }
    }
    None
}

fn top_level_exports(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut positions = Vec::new();
    let mut depth = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'{' {
            depth += 1;
        } else if bytes[index] == b'}' {
            depth -= 1;
        } else if depth == 0
            && bytes[index..].starts_with(b"export")
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
        {
            positions.push(index + "export".len());
            index += "export".len();
            continue;
        }
        index += 1;
    }
    positions
        .iter()
        .enumerate()
        .filter_map(|(index, start)| {
            let end = positions.get(index + 1).copied().unwrap_or(body.len());
            export_names_without_default(body[*start..end].trim())
                .into_iter()
                .next()
        })
        .collect()
}

fn export_names(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let source_text = text(node, source);
    let after_export = source_text
        .trim()
        .strip_prefix("export")
        .unwrap_or("")
        .trim();
    if after_export.starts_with("default") {
        return vec!["default".to_string()];
    }
    if let Some(raw) = after_export.strip_prefix("type ")
        && raw.trim_start().starts_with('{')
    {
        return export_names_without_default(raw.trim());
    }
    export_names_without_default(after_export)
}

fn export_names_without_default(raw: &str) -> Vec<String> {
    if raw.starts_with('*') {
        return Vec::new();
    }
    if raw.starts_with('{')
        && let (Some(open), Some(close)) = (raw.find('{'), raw.find('}'))
    {
        return raw[open + 1..close]
            .split(',')
            .filter_map(|item| {
                item.trim()
                    .strip_prefix("type ")
                    .unwrap_or(item.trim())
                    .split(" as ")
                    .next()
            })
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
    }
    for keyword in ["const", "let", "var"] {
        if let Some(rest) = raw.strip_prefix(keyword) {
            let name = rest
                .trim()
                .split([',', '=', ';'])
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                return vec![name.to_string()];
            }
        }
    }
    for keyword in [
        "function",
        "class",
        "interface",
        "type",
        "enum",
        "namespace",
    ] {
        if let Some(rest) = raw.strip_prefix(keyword) {
            let name = rest
                .split([' ', '{', '<', '='])
                .find(|part| !part.is_empty());
            if let Some(name) = name {
                return vec![name.to_string()];
            }
        }
    }
    Vec::new()
}

#[allow(clippy::too_many_arguments)]
fn visit_go(
    node: Node<'_>,
    source: &[u8],
    imports: &mut Vec<Import>,
    symbols: &mut Vec<SymbolDefinition>,
    symbol_references: &mut Vec<String>,
    member_references: &mut Vec<MemberReference>,
    package: &mut Option<String>,
    has_main: &mut bool,
) {
    if matches!(node.kind(), "identifier" | "field_identifier") {
        symbol_references.push(text(node, source));
    }
    if node.kind() == "selector_expression"
        && let (Some(operand), Some(field)) = (
            node.child_by_field_name("operand"),
            node.child_by_field_name("field"),
        )
    {
        member_references.push(MemberReference {
            namespace: text(operand, source),
            member: text(field, source),
        });
    }
    if node.kind() == "package_clause"
        && let Some(name) = node.named_child(0)
    {
        *package = Some(text(name, source));
    }

    if node.kind() == "function_declaration" || node.kind() == "method_declaration" {
        if let Some(name) = node.child_by_field_name("name") {
            let name = text(name, source);
            if name == "main" {
                *has_main = true;
            }
            add_symbol(
                symbols,
                name,
                node.start_position().row as u32 + 1,
                SymbolKind::Value,
            );
        }
    } else if matches!(node.kind(), "type_spec" | "const_spec" | "var_spec")
        && let Some(name) = node.child_by_field_name("name")
    {
        add_symbol(
            symbols,
            text(name, source),
            node.start_position().row as u32 + 1,
            if node.kind() == "type_spec" {
                SymbolKind::Type
            } else {
                SymbolKind::Value
            },
        );
    }

    if node.kind() == "import_spec"
        && let Some(path) = node.child_by_field_name("path")
        && let Some(raw) = string_literal(path, source)
    {
        imports.push(Import {
            alias: node
                .child_by_field_name("name")
                .map(|name| text(name, source))
                .filter(|name| name != "_" && name != "."),
            raw,
            line: node.start_position().row as u32 + 1,
            kind: ImportKind::Package,
        });
    }
}

fn filter_rust_scoped_imports(imports: &mut Vec<Import>, source: &str) {
    let aliases: HashSet<String> = imports
        .iter()
        .filter(|import| {
            import.kind == ImportKind::RustUse
                && source
                    .lines()
                    .nth(import.line.saturating_sub(1) as usize)
                    .is_some_and(|line| line.trim_start().starts_with("use "))
        })
        .flat_map(|import| {
            let path = import.raw.split(" as ").next().unwrap_or(&import.raw);
            let path = path.split("::{").next().unwrap_or(path);
            let mut aliases = vec![
                path.split("::").next().unwrap_or(path).to_string(),
                path.rsplit("::").next().unwrap_or(path).to_string(),
            ];
            if let Some(members) = import.raw.split("::{").nth(1) {
                aliases.extend(
                    members
                        .split('}')
                        .next()
                        .unwrap_or(members)
                        .split(',')
                        .filter_map(|member| {
                            let member = member.split(" as ").last()?.trim();
                            (!member.is_empty()).then(|| member.to_string())
                        }),
                );
            }
            aliases
        })
        .collect();
    let declarations: HashSet<(u32, String)> = imports
        .iter()
        .filter(|import| {
            import.kind == ImportKind::RustUse
                && source
                    .lines()
                    .nth(import.line.saturating_sub(1) as usize)
                    .is_some_and(|line| line.trim_start().starts_with("use "))
        })
        .map(|import| (import.line, import.raw.clone()))
        .collect();
    imports.retain(|import| {
        if import.kind != ImportKind::RustUse {
            return true;
        }
        let first = import.raw.split("::").next().unwrap_or(&import.raw);
        let line_is_use = source
            .lines()
            .nth(import.line.saturating_sub(1) as usize)
            .is_some_and(|line| line.trim_start().starts_with("use "));
        if line_is_use {
            declarations.contains(&(import.line, import.raw.clone()))
        } else {
            !aliases.contains(first)
        }
    });
}

fn visit_rust(
    node: Node<'_>,
    source: &[u8],
    imports: &mut Vec<Import>,
    symbols: &mut Vec<SymbolDefinition>,
    symbol_references: &mut Vec<String>,
    member_references: &mut Vec<MemberReference>,
    has_main: &mut bool,
) {
    if matches!(
        node.kind(),
        "identifier" | "type_identifier" | "field_identifier"
    ) {
        symbol_references.push(text(node, source));
    }
    if matches!(
        node.kind(),
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "type_item"
            | "const_item"
            | "static_item"
            | "mod_item"
    ) && let Some(name) = node.child_by_field_name("name")
    {
        let name = text(name, source);
        if node.kind() == "function_item" && name == "main" {
            *has_main = true;
        }
        if name != "tests" && !within_test_module(node, source) {
            add_symbol(
                symbols,
                name,
                node.start_position().row as u32 + 1,
                if matches!(
                    node.kind(),
                    "struct_item" | "enum_item" | "trait_item" | "type_item"
                ) {
                    SymbolKind::Type
                } else {
                    SymbolKind::Value
                },
            );
        }
    }

    if node.kind() == "scoped_identifier" {
        let raw = text(node, source);
        let parts: Vec<_> = raw.split("::").collect();
        if parts.len() >= 2 {
            member_references.push(MemberReference {
                namespace: parts[..parts.len() - 1].join("::"),
                member: parts[parts.len() - 1].to_string(),
            });
        }
    }

    if node.kind() == "scoped_identifier" {
        let raw = text(node, source);
        let first = raw.split("::").next().unwrap_or("");
        if !matches!(first, "crate" | "self" | "super" | "std" | "core" | "alloc")
            && first
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_lowercase())
        {
            imports.push(Import {
                alias: None,
                raw,
                line: node.start_position().row as u32 + 1,
                kind: ImportKind::RustUse,
            });
        }
    }

    if node.kind() == "mod_item"
        && node.child_by_field_name("body").is_none()
        && let Some(name) = node.child_by_field_name("name")
    {
        imports.push(Import {
            alias: None,
            raw: text(name, source),
            line: node.start_position().row as u32 + 1,
            kind: ImportKind::RustModule,
        });
    }

    if node.kind() == "use_declaration" {
        let raw = text(node, source)
            .trim()
            .strip_prefix("use")
            .unwrap_or("")
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();
        if !raw.is_empty() {
            imports.push(Import {
                alias: None,
                raw,
                line: node.start_position().row as u32 + 1,
                kind: ImportKind::RustUse,
            });
        }
    }
}

fn within_test_module(node: Node<'_>, source: &[u8]) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "mod_item"
            && parent
                .child_by_field_name("name")
                .is_some_and(|name| text(name, source) == "tests")
        {
            return true;
        }
        current = parent.parent();
    }
    false
}

fn visit_lua(
    node: Node<'_>,
    source: &[u8],
    imports: &mut Vec<Import>,
    symbols: &mut Vec<SymbolDefinition>,
    symbol_references: &mut Vec<String>,
    member_references: &mut Vec<MemberReference>,
) {
    if matches!(node.kind(), "identifier" | "dot_index_expression") {
        let name = text(node, source);
        symbol_references.push(name.rsplit(['.', ':']).next().unwrap_or(&name).to_string());
    }
    if node.kind() == "dot_index_expression"
        && let (Some(table), Some(field)) = (
            node.child_by_field_name("table"),
            node.child_by_field_name("field"),
        )
    {
        member_references.push(MemberReference {
            namespace: text(table, source),
            member: text(field, source),
        });
    }

    if node.kind() == "function_declaration"
        && let Some(name) = node.child_by_field_name("name")
    {
        let name = text(name, source);
        add_symbol(
            symbols,
            name.rsplit(['.', ':']).next().unwrap_or(&name).to_string(),
            node.start_position().row as u32 + 1,
            SymbolKind::Value,
        );
    }
    if node.kind() == "variable_declaration"
        && let Some(list) = node.named_child(0)
        && list.kind() == "variable_list"
    {
        let mut cursor = list.walk();
        for child in list.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                add_symbol(
                    symbols,
                    text(child, source),
                    child.start_position().row as u32 + 1,
                    SymbolKind::Value,
                );
            }
        }
    }
    if node.kind() != "function_call" {
        return;
    }

    let call = text(node, source);
    for name in ["require", "dofile", "loadfile"] {
        if let Some(raw) = literal_call_argument(&call, name) {
            imports.push(Import {
                alias: None,
                raw,
                line: node.start_position().row as u32 + 1,
                kind: ImportKind::Module,
            });
            break;
        }
    }
}

fn add_symbol(symbols: &mut Vec<SymbolDefinition>, name: String, line: u32, kind: SymbolKind) {
    if !symbols
        .iter()
        .any(|symbol| symbol.name == name && symbol.line == line)
    {
        symbols.push(SymbolDefinition { name, line, kind });
    }
}

fn first_string_child(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| string_literal(child, source))
}

fn string_literal(node: Node<'_>, source: &[u8]) -> Option<String> {
    let value = text(node, source);
    let bytes = value.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let quote = bytes[0];
    if !matches!(quote, b'\'' | b'"') || bytes[bytes.len() - 1] != quote {
        return None;
    }
    Some(
        value[1..value.len() - 1]
            .replace("\\\\", "\\")
            .replace("\\\"", "\""),
    )
}

fn literal_call_argument(call: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}(");
    let start = call.trim_start().strip_prefix(&prefix)?;
    let quote = start.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let end = start[1..].find(quote as char)? + 1;
    Some(start[1..end].to_string())
}

fn text(node: Node<'_>, source: &[u8]) -> String {
    node.utf8_text(source).unwrap_or("").to_string()
}

fn is_relative(raw: &str) -> bool {
    raw == "." || raw == ".." || raw.starts_with("./") || raw.starts_with("../")
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower
        .split('/')
        .any(|part| matches!(part, "test" | "tests" | "__tests__" | "spec" | "testdata"))
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with("_test.go")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with("_test.rs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(name: &str, source: &str) -> FileRecord {
        parse(PathBuf::from(name), name.to_string(), source)
    }

    #[test]
    fn extracts_typescript_modules_and_packages() {
        let file = parsed(
            "src/main.ts",
            r#"import {x} from './x';
import lodash from 'lodash';
export * from './y';
const z = require('./z');"#,
        );
        let imports: Vec<_> = file.imports.iter().map(|item| item.raw.as_str()).collect();
        assert_eq!(imports, vec!["./x", "lodash", "./y", "./z"]);
        assert_eq!(file.symbol_imports[0].bindings[0].imported, "x");
    }

    #[test]
    fn extracts_typescript_exports_and_symbol_imports() {
        let file = parsed(
            "src/used.ts",
            "export const used = 1;\nexport const unusedExport = 2;\n",
        );
        assert_eq!(
            file.exports
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["used", "unusedExport"]
        );
        assert_eq!(file.symbol_references, vec!["used", "unusedExport"]);
    }

    #[test]
    fn extracts_go_packages_and_main() {
        let file = parsed(
            "cmd/app/main.go",
            r#"package main
import ("fmt"; "example.com/me/lib")
func main() {}"#,
        );
        assert_eq!(file.package.as_deref(), Some("main"));
        assert!(file.has_main);
        assert_eq!(file.imports.len(), 2);
    }

    #[test]
    fn extracts_rust_modules_and_uses() {
        let file = parsed("src/lib.rs", "mod model;\nuse crate::parse::Parser;");
        assert_eq!(file.imports[0].raw, "model");
        assert_eq!(file.imports[0].kind, ImportKind::RustModule);
        assert_eq!(file.imports[1].raw, "crate::parse::Parser");
    }

    #[test]
    fn extracts_literal_lua_requires() {
        let file = parsed("lua/init.lua", "local x = require('foo.bar')");
        assert_eq!(file.imports[0].raw, "foo.bar");
    }
}
