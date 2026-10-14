use crate::model::Language;
use std::path::Path;
use tree_sitter::Language as TsLanguage;

pub fn from_path(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "mts" | "cts" | "js" | "mjs" | "cjs" => Some(Language::TypeScript),
        "tsx" | "jsx" => Some(Language::Tsx),
        "go" => Some(Language::Go),
        "rs" => Some(Language::Rust),
        "lua" => Some(Language::Lua),
        _ => None,
    }
}

pub fn grammar(language: Language) -> TsLanguage {
    match language {
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Language::Go => tree_sitter_go::LANGUAGE.into(),
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::Lua => tree_sitter_lua::LANGUAGE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tree_sitter::Parser;

    #[test]
    fn maps_supported_extensions() {
        assert_eq!(from_path(Path::new("src/a.ts")), Some(Language::TypeScript));
        assert_eq!(from_path(Path::new("src/a.tsx")), Some(Language::Tsx));
        assert_eq!(from_path(Path::new("main.go")), Some(Language::Go));
        assert_eq!(from_path(Path::new("lib.rs")), Some(Language::Rust));
        assert_eq!(from_path(Path::new("init.lua")), Some(Language::Lua));
        assert_eq!(from_path(Path::new("readme.md")), None);
    }

    #[test]
    fn every_grammar_loads() {
        for language in [
            Language::TypeScript,
            Language::Tsx,
            Language::Go,
            Language::Rust,
            Language::Lua,
        ] {
            let mut parser = Parser::new();
            parser.set_language(&grammar(language)).unwrap();
            assert!(parser.parse("", None).is_some(), "{language:?}");
        }
    }
}
