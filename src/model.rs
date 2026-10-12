use serde::Serialize;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    TypeScript,
    Tsx,
    Go,
    Rust,
    Lua,
}

impl Language {
    pub fn is_typescript(self) -> bool {
        matches!(self, Self::TypeScript | Self::Tsx)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportKind {
    Module,
    Package,
    RustModule,
    RustUse,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub raw: String,
    pub alias: Option<String>,
    pub line: u32,
    pub kind: ImportKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Value,
    Type,
}

#[derive(Clone, Debug)]
pub struct ExportSymbol {
    pub name: String,
    pub line: u32,
    pub kind: SymbolKind,
}

#[derive(Clone, Debug)]
pub struct ExportAlias {
    pub source: String,
    pub alias: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct SymbolDefinition {
    pub name: String,
    pub line: u32,
    pub kind: SymbolKind,
}

#[derive(Clone, Debug)]
pub struct SymbolBinding {
    pub imported: String,
    pub local: String,
}

#[derive(Clone, Debug)]
pub struct SymbolImport {
    pub raw: String,
    pub bindings: Vec<SymbolBinding>,
}

#[derive(Clone, Debug)]
pub struct MemberReference {
    pub namespace: String,
    pub member: String,
}

#[derive(Clone, Debug)]
pub struct EnumMember {
    pub namespace: String,
    pub name: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct NamespaceMember {
    pub namespace: String,
    pub name: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct FileRecord {
    pub path: PathBuf,
    pub display_path: String,
    pub language: Language,
    pub imports: Vec<Import>,
    pub exports: Vec<ExportSymbol>,
    pub export_aliases: Vec<ExportAlias>,
    pub symbols: Vec<SymbolDefinition>,
    pub symbol_references: Vec<String>,
    pub symbol_imports: Vec<SymbolImport>,
    pub member_references: Vec<MemberReference>,
    pub enum_members: Vec<EnumMember>,
    pub namespace_members: Vec<NamespaceMember>,
    pub package: Option<String>,
    pub has_main: bool,
    pub parse_error: bool,
    pub is_test: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ManifestKind {
    #[default]
    Npm,
    Cargo,
    Go,
    Lua,
}

#[derive(Clone, Debug)]
pub struct Dependency {
    pub name: String,
    pub kind: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct CatalogEntry {
    pub namespace: String,
    pub name: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct CatalogReference {
    pub namespace: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct PackageManifest {
    pub kind: ManifestKind,
    pub path: PathBuf,
    pub package_name: Option<String>,
    pub entries: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub optional_peers: Vec<String>,
    pub binaries: Vec<String>,
    pub catalog_path: Option<PathBuf>,
    pub catalog_entries: Vec<CatalogEntry>,
    pub catalog_references: Vec<CatalogReference>,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub entries: Vec<String>,
    pub exclude: Vec<String>,
    pub include_tests: bool,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub files: Vec<FileRecord>,
    pub manifest: Option<PackageManifest>,
    pub additional_manifests: Vec<PackageManifest>,
    pub config: Config,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct PackageUse {
    pub file: usize,
    pub line: u32,
    pub name: String,
    pub raw: String,
    pub resolved: bool,
}

#[derive(Clone, Debug)]
pub struct Graph {
    pub edges: Vec<Vec<usize>>,
    pub unresolved: Vec<Unresolved>,
    pub used_packages: Vec<String>,
    pub package_uses: Vec<PackageUse>,
    pub used_modules: Vec<String>,
    pub used_symbols: Vec<(usize, String)>,
    pub used_enum_members: Vec<(usize, String, String)>,
    pub used_namespace_members: Vec<(usize, String, String)>,
    pub roots: Vec<Root>,
    pub reachable: Vec<bool>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct Root {
    pub file: usize,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Unresolved {
    pub file: usize,
    pub line: u32,
    pub raw: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub v: u8,
    pub ok: bool,
    pub summary: Summary,
    pub findings: Vec<Finding>,
    pub diagnostics: Vec<OutputDiagnostic>,
    pub dropped: Dropped,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub files: usize,
    pub roots: usize,
    pub reachable_files: usize,
    pub unused_files: usize,
    pub unused_dependencies: usize,
    pub unresolved: usize,
    pub parse_errors: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub name: String,
    pub confidence: String,
    pub message: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutputDiagnostic {
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Dropped {
    pub not_shown: usize,
}
