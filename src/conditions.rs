//! Static build conditions. Unknown Rust conditions are kept, never guessed away.
use crate::model::{Config, FileRecord, Language};
use std::{collections::HashSet, path::Path};
use tree_sitter::{Node, Parser};

#[derive(Debug)]
enum Condition {
    Atom(String),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}
fn parts<'a>(s: &'a str, separator: &str) -> Vec<&'a str> {
    let mut depth = 0i32;
    let mut quote = false;
    let mut escaped = false;
    let mut start = 0;
    let mut result = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let b = s.as_bytes()[i];
        if escaped {
            escaped = false;
        } else if b == b'\\' && quote {
            escaped = true;
        } else if b == b'"' {
            quote = !quote;
        } else if !quote {
            if depth == 0 && s[i..].starts_with(separator) {
                result.push(&s[start..i]);
                i += separator.len();
                start = i;
                continue;
            }
            if b == b'(' {
                depth += 1;
            } else if b == b')' {
                depth -= 1;
            }
        }
        i += s[i..].chars().next().unwrap().len_utf8();
    }
    result.push(&s[start..]);
    result
}
fn rust_expr(s: &str) -> Condition {
    let s = s.trim();
    for (prefix, kind) in [("all(", 0), ("any(", 1), ("not(", 2)] {
        if let Some(body) = s.strip_prefix(prefix).and_then(|s| s.strip_suffix(')')) {
            let items: Vec<_> = parts(body, ",")
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .map(rust_expr)
                .collect();
            return match kind {
                0 => Condition::All(items),
                1 => Condition::Any(items),
                _ => Condition::Not(Box::new(
                    items
                        .into_iter()
                        .next()
                        .unwrap_or(Condition::Atom("?".into())),
                )),
            };
        }
    }
    Condition::Atom(s.into())
}
fn go_expr(s: &str) -> Condition {
    let s = s.trim();
    let or = parts(s, "||");
    if or.len() > 1 {
        return Condition::Any(or.into_iter().map(go_expr).collect());
    }
    let and = parts(s, "&&");
    if and.len() > 1 {
        return Condition::All(and.into_iter().map(go_expr).collect());
    }
    if let Some(s) = s.strip_prefix('!') {
        return Condition::Not(Box::new(go_expr(s)));
    }
    if let Some(s) = s.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        return go_expr(s);
    }
    Condition::Atom(s.into())
}
fn evaluate(expr: &Condition, atom: &impl Fn(&str) -> Option<bool>) -> Option<bool> {
    match expr {
        Condition::Atom(s) => atom(s),
        Condition::Not(e) => evaluate(e, atom).map(|v| !v),
        Condition::All(items) => {
            let values: Vec<_> = items.iter().map(|e| evaluate(e, atom)).collect();
            if values.contains(&Some(false)) {
                Some(false)
            } else if values.contains(&None) {
                None
            } else {
                Some(true)
            }
        }
        Condition::Any(items) => {
            let values: Vec<_> = items.iter().map(|e| evaluate(e, atom)).collect();
            if values.contains(&Some(true)) {
                Some(true)
            } else if values.contains(&None) {
                None
            } else {
                Some(false)
            }
        }
    }
}
fn platform(config: &Config) -> (String, String) {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let target = config.target.as_deref().unwrap_or("");
    if let Some((os, arch)) = target.split_once('/') {
        return (
            if os == "darwin" { "macos" } else { os }.into(),
            match arch {
                "amd64" => "x86_64",
                "arm64" => "aarch64",
                "386" => "x86",
                s => s,
            }
            .into(),
        );
    }
    let target_os = if target.is_empty() {
        os
    } else if target.contains("apple") || target.contains("darwin") {
        "macos"
    } else {
        [
            "windows",
            "android",
            "linux",
            "freebsd",
            "netbsd",
            "openbsd",
            "dragonfly",
            "illumos",
            "solaris",
            "emscripten",
            "wasi",
            "none",
        ]
        .into_iter()
        .find(|os| target.contains(os))
        .unwrap_or("unknown")
    };
    let target_arch = if target.is_empty() {
        arch
    } else {
        target.split('-').next().unwrap_or(arch)
    };
    let target_arch = if target_arch.starts_with("armv") {
        "arm"
    } else if target_arch.starts_with("riscv64") {
        "riscv64"
    } else {
        target_arch
    };
    (target_os.into(), target_arch.into())
}

fn rust_atom(s: &str, config: &Config) -> Option<bool> {
    let (os, arch) = platform(config);
    match s.trim() {
        "unix" => Some(matches!(
            os.as_str(),
            "linux"
                | "macos"
                | "android"
                | "freebsd"
                | "netbsd"
                | "openbsd"
                | "dragonfly"
                | "solaris"
                | "illumos"
        )),
        "windows" => Some(os == "windows"),
        "test" => Some(config.include_tests && !config.production),
        "debug_assertions" => None,
        other => {
            let (key, value) = other.split_once('=')?;
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "feature" => {
                    Some(config.all_features || config.features.iter().any(|f| f == value))
                }
                "target_os" => Some(os == value),
                "target_arch" => Some(arch == value),
                "target_family" => Some(if value == "unix" {
                    os != "windows"
                } else {
                    os == value
                }),
                "target_pointer_width" => {
                    Some(value == if arch.contains("64") { "64" } else { "32" })
                }
                _ => None,
            }
        }
    }
}
const GO_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
];
const GO_ARCH: &[&str] = &[
    "386", "amd64", "amd64p32", "arm", "arm64", "loong64", "mips", "mips64", "mips64le", "mipsle",
    "ppc64", "ppc64le", "riscv64", "s390x", "sparc64", "wasm",
];
pub fn go_enabled(file: &FileRecord, config: &Config) -> bool {
    if file.language != Language::Go {
        return true;
    }
    let (os, arch) = platform(config);
    let os = match os.as_str() {
        "macos" => "darwin",
        s => s,
    };
    let arch = match arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        s => s,
    };
    let tags: HashSet<_> = config
        .go_tags
        .iter()
        .map(String::as_str)
        .chain([os, arch, "gc"])
        .collect();
    let atom = |s: &str| Some(tags.contains(s) || s == "unix" && os != "windows" && os != "plan9");
    let stem = Path::new(&file.display_path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let stem = stem.strip_suffix("_test").unwrap_or(&stem);
    for suffix in stem.split('_').rev().take(2) {
        if (GO_OS.contains(&suffix) || GO_ARCH.contains(&suffix)) && !tags.contains(suffix) {
            return false;
        }
    }
    for line in file
        .source
        .lines()
        .take_while(|l| !l.trim_start().starts_with("package "))
    {
        if let Some(expr) = line.trim().strip_prefix("//go:build ") {
            return evaluate(&go_expr(expr), &atom).unwrap_or(true);
        }
    }
    file.source
        .lines()
        .take_while(|l| !l.trim_start().starts_with("package "))
        .filter_map(|l| l.trim().strip_prefix("// +build "))
        .all(|line| {
            line.split_whitespace().any(|term| {
                term.split(',').all(|tag| {
                    let expr = if let Some(tag) = tag.strip_prefix('!') {
                        Condition::Not(Box::new(Condition::Atom(tag.into())))
                    } else {
                        Condition::Atom(tag.into())
                    };
                    evaluate(&expr, &atom).unwrap_or(true)
                })
            })
        })
}
pub fn rust_source(source: &str, config: &Config) -> String {
    let mut parser = Parser::new();
    parser
        .set_language(&crate::lang::grammar(Language::Rust))
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    fn disabled(
        node: Node<'_>,
        source: &str,
        config: &Config,
        ranges: &mut Vec<std::ops::Range<usize>>,
    ) {
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        let mut exclude = false;
        for child in children {
            if matches!(child.kind(), "attribute_item" | "inner_attribute_item") {
                let raw = child
                    .named_child(0)
                    .map(|n| &source[n.byte_range()])
                    .unwrap_or("");
                if let Some(expr) = raw.strip_prefix("cfg(").and_then(|s| s.strip_suffix(')')) {
                    let enabled = evaluate(&rust_expr(expr), &|s| rust_atom(s, config));
                    if enabled == Some(false) {
                        if child.kind() == "inner_attribute_item" {
                            ranges.push(node.byte_range());
                            return;
                        }
                        exclude = true;
                    }
                }
                continue;
            }
            if exclude {
                ranges.push(child.byte_range());
                exclude = false;
            } else {
                disabled(child, source, config, ranges);
            }
        }
    }
    let mut ranges = Vec::new();
    disabled(tree.root_node(), source, config, &mut ranges);
    let mut bytes = source.as_bytes().to_vec();
    for range in ranges {
        for b in &mut bytes[range] {
            if !matches!(*b, b'\r' | b'\n') {
                *b = b' ';
            }
        }
    }
    String::from_utf8(bytes).unwrap()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expressions_preserve_boolean_precedence() {
        let atom = |s: &str| Some(s == "a" || s == "c");
        assert_eq!(evaluate(&go_expr("a && (!b || c)"), &atom), Some(true));
        assert_eq!(evaluate(&go_expr("!a || b && c"), &atom), Some(false));
        assert_eq!(
            evaluate(&rust_expr("all(a,not(b),any(b,c))"), &atom),
            Some(true)
        );
        assert_eq!(
            evaluate(&rust_expr("all(a,unknown)"), &|s| if s == "unknown" {
                None
            } else {
                atom(s)
            }),
            None
        );
    }
    #[test]
    fn static_targets_features_and_unknown_conditions_are_explicit() {
        let mut config = Config {
            target: Some("x86_64-unknown-linux-gnu".into()),
            features: vec!["enabled".into()],
            ..Default::default()
        };
        for (expr, value) in [
            ("unix", Some(true)),
            ("windows", Some(false)),
            ("test", Some(false)),
            ("debug_assertions", None),
            ("feature = \"enabled\"", Some(true)),
            ("feature = \"disabled\"", Some(false)),
            ("target_os = \"linux\"", Some(true)),
            ("target_arch = \"x86_64\"", Some(true)),
            ("target_family = \"unix\"", Some(true)),
            ("target_family = \"windows\"", Some(false)),
            ("target_pointer_width = \"64\"", Some(true)),
            ("target_pointer_width = \"32\"", Some(false)),
            ("custom = \"yes\"", None),
        ] {
            assert_eq!(rust_atom(expr, &config), value, "{expr}");
        }
        config.include_tests = true;
        assert_eq!(rust_atom("test", &config), Some(true));
        config.production = true;
        assert_eq!(rust_atom("test", &config), Some(false));
        for (target, os, arch) in [
            ("x86_64-pc-windows-msvc", "windows", "x86_64"),
            ("aarch64-apple-darwin", "macos", "aarch64"),
            ("aarch64-linux-android", "android", "aarch64"),
            ("armv7-unknown-linux-gnueabihf", "linux", "arm"),
            ("riscv64gc-unknown-linux-gnu", "linux", "riscv64"),
            ("linux/amd64", "linux", "x86_64"),
            ("darwin/arm64", "macos", "aarch64"),
        ] {
            config.target = Some(target.into());
            assert_eq!(platform(&config), (os.into(), arch.into()));
        }
    }
}
