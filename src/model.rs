use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportKind {
    Module,
    Package,
    RustModule,
    Resource,
    RustUse,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Import {
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub type_only: bool,
    pub raw: String,
    pub alias: Option<String>,
    pub line: u32,
    pub kind: ImportKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Value,
    Type,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportSymbol {
    pub name: String,
    pub line: u32,
    pub kind: SymbolKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportAlias {
    pub source: String,
    pub alias: String,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolDefinition {
    #[serde(default)]
    pub contract: bool,
    pub name: String,
    pub public: bool,
    pub line: u32,
    pub kind: SymbolKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolBinding {
    pub imported: String,
    pub local: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolImport {
    pub raw: String,
    pub reexport: bool,
    pub bindings: Vec<SymbolBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberReference {
    pub namespace: String,
    pub member: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnumMember {
    pub namespace: String,
    pub name: String,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamespaceMember {
    pub namespace: String,
    pub name: String,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecord {
    #[serde(default)]
    pub class_members: Vec<NamespaceMember>,
    #[serde(default)]
    pub dynamic_references: Vec<String>,
    #[serde(default)]
    pub compiled: bool,
    pub path: PathBuf,
    pub source: String,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogReference {
    pub namespace: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct PackageManifest {
    pub library: Option<String>,
    pub source: String,
    pub private: bool,
    pub aliases: BTreeMap<String, Vec<String>>,
    pub source_dir: String,
    pub output_dir: Option<String>,
    pub provided_modules: BTreeMap<String, String>,
    pub development_entries: Vec<String>,
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    #[default]
    Error,
    Warn,
    Off,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub entries: Vec<String>,
    pub project: Vec<String>,
    pub exclude: Vec<String>,
    pub paths: BTreeMap<String, Vec<String>>,
    pub ignore_dependencies: Vec<String>,
    pub ignore_unresolved: Vec<String>,
    pub include_entry_exports: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub entries: Vec<String>,
    pub project: Vec<String>,
    pub exclude: Vec<String>,
    pub include_tests: bool,
    pub production: bool,
    pub strict: bool,
    pub include_entry_exports: bool,
    pub ignore_exports_used_in_file: bool,
    pub include: Vec<String>,
    pub ignore_dependencies: Vec<String>,
    pub ignore_binaries: Vec<String>,
    pub ignore_unresolved: Vec<String>,
    pub ignore_members: Vec<String>,
    pub ignore_issues: BTreeMap<String, Vec<String>>,
    pub rules: BTreeMap<String, Rule>,
    pub tags: Vec<String>,
    pub paths: BTreeMap<String, Vec<String>>,
    pub workspaces: BTreeMap<String, WorkspaceConfig>,
    pub ignore_workspaces: Vec<String>,
    pub lua_paths: Vec<String>,
    pub lua_modules: BTreeMap<String, String>,
    pub features: Vec<String>,
    pub all_features: bool,
    pub no_default_features: bool,
    pub go_tags: Vec<String>,
    pub target: Option<String>,
    pub plugins: Vec<String>,
    pub no_gitignore: bool,
    pub compilers: BTreeMap<String, String>,
    #[serde(skip)]
    pub allow_compilers: bool,
    #[serde(skip)]
    pub semantic_go: bool,
    #[serde(skip)]
    pub go_downloads: bool,
    #[serde(skip)]
    pub go_timeout: Option<u64>,
    #[serde(skip)]
    pub cache: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub stamp: u64,
    pub root: PathBuf,
    pub files: Vec<FileRecord>,
    pub manifest: Option<PackageManifest>,
    pub additional_manifests: Vec<PackageManifest>,
    pub config: Config,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageUse {
    pub file: usize,
    pub line: u32,
    pub name: String,
    pub raw: String,
    pub resolved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Graph {
    pub semantic: Vec<crate::semantic::SemanticGraph>,
    pub edges: Vec<Vec<usize>>,
    pub cycles: Vec<Vec<usize>>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Root {
    pub file: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unresolved {
    pub file: usize,
    pub line: u32,
    pub raw: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub analysis: Vec<crate::semantic::AnalysisSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<FixAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<Trace>,
    pub v: u8,
    pub complete: bool,
    pub ok: bool,
    pub summary: Summary,
    pub findings: Vec<Finding>,
    pub diagnostics: Vec<OutputDiagnostic>,
    pub dropped: Dropped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub files: usize,
    pub roots: usize,
    pub reachable_files: usize,
    pub unused_files: usize,
    pub unused_dependencies: usize,
    pub unresolved: usize,
    pub parse_errors: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub kind: String,
    pub severity: String,
    pub file: String,
    pub line: u32,
    pub name: String,
    pub confidence: String,
    pub message: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputDiagnostic {
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Dropped {
    pub not_shown: usize,
}

impl Project {
    pub fn manifests(&self) -> impl Iterator<Item = &PackageManifest> {
        self.manifest.iter().chain(self.additional_manifests.iter())
    }

    pub fn owner(&self, file: usize) -> Option<&PackageManifest> {
        let record = &self.files[file];
        self.manifests()
            .filter(|m| {
                m.matches(record.language)
                    && record.path.starts_with(&self.root)
                    && m.path
                        .parent()
                        .is_some_and(|dir| record.path.starts_with(dir))
            })
            .max_by_key(|m| m.path.components().count())
    }

    pub fn workspace(&self, file: usize) -> Option<&WorkspaceConfig> {
        let dir = self
            .owner(file)?
            .path
            .parent()?
            .strip_prefix(&self.root)
            .ok()?;
        let name = if dir.as_os_str().is_empty() {
            ".".to_string()
        } else {
            dir.to_string_lossy().replace('\\', "/")
        };
        self.config
            .workspaces
            .iter()
            .filter(|(pattern, _)| crate::discover::matches_pattern(&name, pattern))
            .max_by_key(|(pattern, _)| pattern.len())
            .map(|(_, config)| config)
    }
}
impl PackageManifest {
    pub fn matches(&self, language: Language) -> bool {
        matches!(
            (self.kind, language),
            (ManifestKind::Npm, Language::TypeScript | Language::Tsx)
                | (ManifestKind::Go, Language::Go)
                | (ManifestKind::Cargo, Language::Rust)
                | (ManifestKind::Lua, Language::Lua)
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trace {
    pub kind: String,
    pub file: String,
    pub name: String,
    pub references: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixAction {
    pub file: String,
    pub action: String,
    pub names: Vec<String>,
}
