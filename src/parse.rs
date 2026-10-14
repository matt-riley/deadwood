use crate::lang;
use crate::model::{
    EnumMember, ExportAlias, ExportSymbol, FileRecord, Import, ImportKind, Language,
    MemberReference, NamespaceMember, SymbolBinding, SymbolDefinition, SymbolImport, SymbolKind,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

pub fn parse(path: PathBuf, display_path: String, source: &str) -> FileRecord {
    parse_with(path, display_path, source, &crate::model::Config::default())
}
pub fn parse_with(
    path: PathBuf,
    display_path: String,
    original: &str,
    config: &crate::model::Config,
) -> FileRecord {
    let processed = if lang::from_path(Path::new(&display_path)) == Some(Language::Rust) {
        crate::conditions::rust_source(original, config)
    } else {
        original.to_string()
    };
    let source = processed.as_str();
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

    let mut class_members = Vec::new();
    for class in descendants(tree.root_node(), "class_declaration") {
        if !descendants(class, "decorator").is_empty() {
            continue;
        }
        if let Some(namespace) = class.child_by_field_name("name") {
            for kind in ["method_definition", "public_field_definition"] {
                for member in descendants(class, kind) {
                    if let Some(name) = member.child_by_field_name("name") {
                        let name = text(name, source.as_bytes());
                        if name == "constructor"
                            || text(member, source.as_bytes())
                                .split_whitespace()
                                .any(|word| word == "override")
                        {
                            continue;
                        }
                        if !matches!(
                            member.child_by_field_name("name").unwrap().kind(),
                            "property_identifier" | "private_property_identifier"
                        ) {
                            continue;
                        }
                        class_members.push(NamespaceMember {
                            namespace: text(namespace, source.as_bytes()),
                            name,
                            line: member.start_position().row as u32 + 1,
                        });
                    }
                }
            }
        }
    }
    let mut dynamic_references = Vec::new();
    resources(
        tree.root_node(),
        source.as_bytes(),
        language,
        &mut imports,
        &mut dynamic_references,
    );
    for symbol in &mut symbols {
        symbol.public |= match language {
            Language::Go => symbol.name.chars().next().is_some_and(char::is_uppercase),
            Language::Rust => source
                .lines()
                .nth(symbol.line.saturating_sub(1) as usize)
                .is_some_and(|line| line.trim_start().starts_with("pub ")),
            Language::Lua => source.contains(&format!("return {}", symbol.name)),
            _ => false,
        };
    }
    if language == Language::Rust {
        filter_rust_scoped_imports(&mut imports, source);
    }

    FileRecord {
        dynamic_references,
        class_members,
        compiled: false,
        path,
        source: original.to_string(),
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
        let specs = descendants(
            node,
            if node.kind() == "import_statement" {
                "import_specifier"
            } else {
                "export_specifier"
            },
        );
        let type_only = has_token(node, "type")
            || (!specs.is_empty() && specs.iter().all(|n| has_token(*n, "type")));
        imports.push(Import {
            public: false,
            type_only,
            alias: None,
            raw: raw.clone(),
            line: node.start_position().row as u32 + 1,
            kind,
        });
        let bindings = symbol_bindings(node, source);
        if !bindings.is_empty() {
            symbol_imports.push(SymbolImport {
                raw,
                bindings,
                reexport: node.kind() == "export_statement",
            });
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
                public: false,
                type_only: false,
                alias: None,
                kind: if is_relative(&raw) {
                    ImportKind::Module
                } else {
                    ImportKind::Package
                },
                raw: raw.clone(),
                line: node.start_position().row as u32 + 1,
            });
            if name == "require"
                && let Some(parent) = node.parent().filter(|n| n.kind() == "variable_declarator")
                && let Some(pattern) = parent.child_by_field_name("name")
            {
                let bindings = if pattern.kind() == "identifier" {
                    vec![SymbolBinding {
                        imported: "*".into(),
                        local: text(pattern, source),
                    }]
                } else {
                    pattern_names(pattern, source)
                };
                symbol_imports.push(SymbolImport {
                    raw,
                    bindings,
                    reexport: false,
                });
            }
        }
    }
    if node.kind() == "assignment_expression"
        && let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        )
    {
        let lhs = text(left, source);
        let line = node.start_position().row as u32 + 1;
        let add = |name: String, exports: &mut Vec<ExportSymbol>| {
            exports.push(ExportSymbol {
                name,
                line,
                kind: SymbolKind::Value,
            })
        };
        if lhs == "module.exports" {
            if right.kind() == "object" {
                let mut c = right.walk();
                for item in right.named_children(&mut c) {
                    match item.kind() {
                        "shorthand_property_identifier" => add(text(item, source), exports),
                        "pair" => {
                            if let Some(key) = item.child_by_field_name("key") {
                                add(text(key, source).trim_matches(['\'', '"']).into(), exports);
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                add("default".into(), exports);
            }
        } else if let Some(name) = lhs
            .strip_prefix("exports.")
            .or_else(|| lhs.strip_prefix("module.exports."))
            && is_identifier(name)
        {
            add(name.into(), exports);
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

fn descendants<'a>(node: Node<'a>, kind: &str) -> Vec<Node<'a>> {
    let mut result = Vec::new();
    if node.kind() == kind {
        result.push(node);
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        result.extend(descendants(child, kind));
    }
    result
}
fn symbol_bindings(node: Node<'_>, source: &[u8]) -> Vec<SymbolBinding> {
    let mut bindings = Vec::new();
    for kind in ["import_specifier", "export_specifier"] {
        for item in descendants(node, kind) {
            if let Some(name) = item.child_by_field_name("name") {
                let imported = text(name, source).trim_matches(['\'', '"']).to_string();
                let local = item
                    .child_by_field_name("alias")
                    .map(|n| text(n, source).trim_matches(['\'', '"']).to_string())
                    .unwrap_or_else(|| imported.clone());
                bindings.push(SymbolBinding { imported, local });
            }
        }
    }
    for kind in ["namespace_import", "namespace_export"] {
        for item in descendants(node, kind) {
            bindings.push(SymbolBinding {
                imported: "*".into(),
                local: item
                    .named_child(0)
                    .map(|n| text(n, source))
                    .unwrap_or_else(|| "*".into()),
            });
        }
    }
    if node.kind() == "import_statement" {
        for clause in descendants(node, "import_clause") {
            let mut c = clause.walk();
            for item in clause
                .named_children(&mut c)
                .filter(|n| n.kind() == "identifier")
            {
                bindings.push(SymbolBinding {
                    imported: "default".into(),
                    local: text(item, source),
                });
            }
        }
    } else if text(node, source)
        .trim_start()
        .strip_prefix("export")
        .is_some_and(|s| s.trim_start().starts_with('*'))
        && bindings.is_empty()
    {
        bindings.push(SymbolBinding {
            imported: "*".into(),
            local: "*".into(),
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
    let mut parser = Parser::new();
    parser
        .set_language(&lang::grammar(Language::TypeScript))
        .unwrap();
    let tree = parser.parse(body, None).unwrap();
    let mut c = tree.root_node().walk();
    tree.root_node()
        .named_children(&mut c)
        .filter(|n| n.kind() == "export_statement")
        .flat_map(|n| export_names(n, body.as_bytes()))
        .collect()
}

fn export_names(node: Node<'_>, source: &[u8]) -> Vec<String> {
    if text(node, source)
        .trim_start()
        .strip_prefix("export")
        .is_some_and(|s| s.trim_start().starts_with("default"))
    {
        return vec!["default".into()];
    }
    if let Some(declaration) = node.child_by_field_name("declaration") {
        if let Some(name) = declaration.child_by_field_name("name") {
            return vec![text(name, source)];
        }
        let mut names = Vec::new();
        for declarator in descendants(declaration, "variable_declarator") {
            if let Some(pattern) = declarator.child_by_field_name("name") {
                names.extend(pattern_names(pattern, source).into_iter().map(|b| b.local));
            }
        }
        return names;
    }
    symbol_bindings(node, source)
        .into_iter()
        .filter(|b| b.local != "*")
        .map(|b| b.local)
        .collect()
}
fn pattern_names(node: Node<'_>, source: &[u8]) -> Vec<SymbolBinding> {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => vec![SymbolBinding {
            imported: text(node, source),
            local: text(node, source),
        }],
        "pair_pattern" => node
            .child_by_field_name("key")
            .zip(node.child_by_field_name("value"))
            .map(|(key, value)| {
                vec![SymbolBinding {
                    imported: text(key, source),
                    local: text(value, source),
                }]
            })
            .unwrap_or_default(),
        _ => {
            let mut c = node.walk();
            node.named_children(&mut c)
                .flat_map(|n| pattern_names(n, source))
                .collect()
        }
    }
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
    if matches!(node.kind(), "identifier" | "field_identifier")
        && !shadowed(node, source, Language::Go)
    {
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
            let contract = node.kind() == "method_declaration"
                && name.chars().next().is_some_and(char::is_uppercase)
                || ["Test", "Benchmark", "Fuzz", "Example"]
                    .iter()
                    .any(|prefix| {
                        name.strip_prefix(prefix).is_some_and(|rest| {
                            rest.is_empty() || rest.chars().next().is_some_and(char::is_uppercase)
                        })
                    });
            add_symbol(
                symbols,
                name,
                node.start_position().row as u32 + 1,
                SymbolKind::Value,
            );
            if let Some(symbol) = symbols.last_mut() {
                symbol.contract = contract;
            }
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
            public: false,
            type_only: false,
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

fn filter_rust_scoped_imports(imports: &mut Vec<Import>, _source: &str) {
    let aliases: HashSet<_> = imports
        .iter()
        .filter(|i| i.kind == ImportKind::RustUse)
        .filter_map(|i| i.alias.clone())
        .collect();
    imports.retain(|i| {
        i.kind != ImportKind::RustUse
            || i.alias.is_some()
            || !aliases.contains(i.raw.split("::").next().unwrap_or(&i.raw))
    });
}
fn rust_use(node: Node<'_>, prefix: &str, source: &[u8]) -> Vec<(String, String)> {
    match node.kind() {
        "use_list" => {
            let mut c = node.walk();
            node.named_children(&mut c)
                .flat_map(|n| rust_use(n, prefix, source))
                .collect()
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .map(|n| text(n, source))
                .unwrap_or_default();
            let prefix = if prefix.is_empty() {
                path
            } else if path.is_empty() {
                prefix.into()
            } else {
                format!("{prefix}::{path}")
            };
            node.child_by_field_name("list")
                .map(|n| rust_use(n, &prefix, source))
                .unwrap_or_default()
        }
        "use_as_clause" => {
            let mut uses = node
                .child_by_field_name("path")
                .map(|n| rust_use(n, prefix, source))
                .unwrap_or_default();
            if let Some(alias) = node.child_by_field_name("alias") {
                for (_, local) in &mut uses {
                    *local = text(alias, source);
                }
            }
            uses
        }
        _ => {
            let path = text(node, source);
            let raw = if prefix.is_empty() {
                path.clone()
            } else if path == "self" {
                prefix.into()
            } else {
                format!("{prefix}::{path}")
            };
            let alias = raw.rsplit("::").next().unwrap_or(&raw).to_string();
            vec![(raw, alias)]
        }
    }
}
fn inside(node: Node<'_>, kind: &str) -> bool {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == kind {
            return true;
        }
        current = n.parent();
    }
    false
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
    ) && !shadowed(node, source, Language::Rust)
    {
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

    if matches!(
        node.kind(),
        "function_item" | "trait_item" | "struct_item" | "enum_item"
    ) {
        let mut contract = inside(node, "trait_item");
        let mut ancestor = node.parent();
        while let Some(parent) = ancestor {
            if parent.kind() == "impl_item" && parent.child_by_field_name("trait").is_some() {
                contract = true;
            }
            ancestor = parent.parent();
        }
        let mut previous = node.prev_named_sibling();
        while let Some(attribute) = previous.filter(|n| {
            matches!(
                n.kind(),
                "attribute_item" | "line_comment" | "block_comment"
            )
        }) {
            let raw = text(attribute, source);
            if raw == "#[test]" || raw.contains("::test]") || raw.starts_with("#[rstest") {
                contract = true;
            }
            previous = attribute.prev_named_sibling();
        }
        if let Some(name) = node.child_by_field_name("name") {
            let name = text(name, source);
            if let Some(symbol) = symbols
                .iter_mut()
                .find(|s| s.name == name && s.line == node.start_position().row as u32 + 1)
            {
                symbol.contract = contract;
                let mut c = node.walk();
                symbol.public = node
                    .named_children(&mut c)
                    .any(|n| n.kind() == "visibility_modifier" && text(n, source) == "pub");
            }
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

    if node.kind() == "scoped_identifier" && !inside(node, "use_declaration") {
        let raw = text(node, source);
        let first = raw.split("::").next().unwrap_or("");
        if !matches!(first, "crate" | "self" | "super" | "std" | "core" | "alloc")
            && first
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_lowercase())
        {
            imports.push(Import {
                public: false,
                type_only: false,
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
            public: {
                let mut c = node.walk();
                node.named_children(&mut c)
                    .any(|n| n.kind() == "visibility_modifier" && text(n, source) == "pub")
            },
            type_only: false,
            alias: None,
            raw: text(name, source),
            line: node.start_position().row as u32 + 1,
            kind: ImportKind::RustModule,
        });
    }

    if node.kind() == "use_declaration"
        && let Some(argument) = node.child_by_field_name("argument")
    {
        for (raw, alias) in rust_use(argument, "", source) {
            imports.push(Import {
                public: false,
                type_only: false,
                alias: Some(alias),
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
    if matches!(node.kind(), "identifier" | "dot_index_expression")
        && !(node.kind() == "identifier"
            && node
                .parent()
                .is_some_and(|p| p.kind() == "dot_index_expression"))
    {
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
        if name.starts_with("love.")
            && let Some(symbol) = symbols.last_mut()
        {
            symbol.contract = true;
        }
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
                public: false,
                type_only: false,
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
        symbols.push(SymbolDefinition {
            contract: false,
            name,
            line,
            kind,
            public: false,
        });
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

fn resources(
    node: Node<'_>,
    source: &[u8],
    language: Language,
    imports: &mut Vec<Import>,
    dynamic: &mut Vec<String>,
) {
    let callable = match language {
        Language::TypeScript | Language::Tsx => node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("constructor")),
        Language::Lua => node.child_by_field_name("name"),
        Language::Rust if node.kind() == "macro_invocation" => node.child_by_field_name("macro"),
        _ => None,
    };
    if let Some(callable) = callable {
        let name = text(callable, source);
        let module = match language {
            Language::TypeScript | Language::Tsx => matches!(
                name.as_str(),
                "require"
                    | "import"
                    | "import.meta.glob"
                    | "import.meta.globEager"
                    | "Worker"
                    | "SharedWorker"
                    | "URL"
            ),
            Language::Lua => matches!(name.as_str(), "require" | "dofile" | "loadfile"),
            Language::Rust => matches!(name.as_str(), "include" | "include_str" | "include_bytes"),
            _ => false,
        };
        if module {
            let args = node
                .child_by_field_name("arguments")
                .or_else(|| node.child_by_field_name("token_tree"))
                .or_else(|| node.named_child(1));
            let literal = args.and_then(|args| first_string_child(args, source));
            if let Some(raw) = literal {
                if !matches!(name.as_str(), "require" | "import" | "dofile" | "loadfile") {
                    // URL paths without a relative prefix are URLs, not local module references.
                    if name != "URL" || is_relative(&raw) {
                        imports.push(Import {
                            public: false,
                            type_only: false,
                            raw,
                            line: node.start_position().row as u32 + 1,
                            kind: ImportKind::Resource,
                            alias: None,
                        });
                    }
                }
            } else if !matches!(name.as_str(), "Worker" | "SharedWorker" | "URL") {
                dynamic.push(format!(
                    "line {}: non-literal {name} reference",
                    node.start_position().row + 1
                ));
            }
        }
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        resources(child, source, language, imports, dynamic);
    }
}
fn shadowed(node: Node<'_>, source: &[u8], language: Language) -> bool {
    let name = text(node, source);
    fn declared(
        node: Node<'_>,
        position: usize,
        source: &[u8],
        language: Language,
        name: &str,
    ) -> bool {
        if matches!(
            node.kind(),
            "block"
                | "function_declaration"
                | "function_item"
                | "function_literal"
                | "closure_expression"
        ) {
            return false;
        }
        let fields: &[&str] = match node.kind() {
            "short_var_declaration" => &["left"],
            "var_spec" | "const_spec" if language == Language::Go => &["name"],
            "parameter_declaration" | "variadic_parameter_declaration" => &["name"],
            "parameter" | "let_declaration" => &["pattern"],
            _ => &[],
        };
        let mut c = node.walk();
        for field in fields {
            for pattern in node.children_by_field_name(field, &mut c) {
                let identifiers: Vec<_> = descendants(pattern, "identifier");
                if identifiers.iter().any(|id| text(*id, source) == name)
                    && (node.end_byte() <= position
                        || pattern.byte_range().contains(&position)
                        || matches!(
                            node.kind(),
                            "parameter"
                                | "parameter_declaration"
                                | "variadic_parameter_declaration"
                        ))
                {
                    return true;
                }
            }
        }
        let mut c = node.walk();
        node.named_children(&mut c)
            .any(|child| declared(child, position, source, language, name))
    }
    let mut ancestor = node.parent();
    while let Some(scope) = ancestor {
        if matches!(
            scope.kind(),
            "block"
                | "function_declaration"
                | "function_item"
                | "function_literal"
                | "closure_expression"
        ) {
            let mut c = scope.walk();
            if scope
                .named_children(&mut c)
                .any(|child| declared(child, node.start_byte(), source, language, &name))
            {
                return true;
            }
        }
        ancestor = scope.parent();
    }
    false
}

fn has_token(node: Node<'_>, kind: &str) -> bool {
    let mut c = node.walk();
    node.children(&mut c).any(|n| n.kind() == kind)
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
