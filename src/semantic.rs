//! Compiler-neutral declaration graph. Backends supply identities; reachability lives here.
use crate::model::{Diagnostic, Language, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolId(pub String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceSpan {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub line: u32,
}
impl SourceSpan {
    fn validate(&self) -> Result<(), String> {
        if self.file.is_empty() || self.start > self.end || self.line == 0 {
            return Err("invalid semantic source span".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Declaration {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub span: SourceSpan,
    pub owner: Option<SymbolId>,
    pub exported: bool,
    pub reportable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    pub from: SymbolId,
    pub to: SymbolId,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticRoot {
    pub symbol: SymbolId,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticPackage {
    pub id: SymbolId,
    pub files: Vec<String>,
    pub imports: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticGraph {
    pub v: u8,
    pub language: Language,
    pub complete: bool,
    pub profile: BTreeMap<String, String>,
    pub declarations: Vec<Declaration>,
    pub references: Vec<Reference>,
    pub roots: Vec<SemanticRoot>,
    pub packages: Vec<SemanticPackage>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisSummary {
    pub language: Language,
    pub complete: bool,
    pub profile: BTreeMap<String, String>,
    pub declarations: usize,
    pub references: usize,
    pub roots: usize,
    pub selected_files: usize,
}
impl SemanticGraph {
    pub fn summary(&self) -> AnalysisSummary {
        AnalysisSummary {
            language: self.language,
            complete: self.complete,
            profile: self.profile.clone(),
            declarations: self.declarations.len(),
            references: self.references.len(),
            roots: self.roots.len(),
            selected_files: self
                .packages
                .iter()
                .flat_map(|p| &p.files)
                .collect::<HashSet<_>>()
                .len(),
        }
    }
    /// Reject invalid/incomplete IR before using any of it to suggest cleanup.
    pub fn reachable(&self) -> Result<HashSet<SymbolId>, String> {
        if self.v != 1 || !self.complete || self.declarations.is_empty() || self.roots.is_empty() {
            return Err("unsupported, incomplete or empty semantic graph".into());
        }
        let mut ids = HashSet::new();
        for declaration in &self.declarations {
            declaration.span.validate()?;
            if declaration.id.0.is_empty() || !ids.insert(&declaration.id) {
                return Err("empty or duplicate semantic declaration identity".into());
            }
        }
        for declaration in &self.declarations {
            if declaration
                .owner
                .as_ref()
                .is_some_and(|owner| !ids.contains(owner))
            {
                return Err("unknown semantic declaration owner".into());
            }
        }
        let mut edges: HashMap<&SymbolId, Vec<&SymbolId>> = HashMap::new();
        for reference in &self.references {
            reference.span.validate()?;
            if !ids.contains(&reference.from) || !ids.contains(&reference.to) {
                return Err("unknown semantic reference endpoint".into());
            }
            edges
                .entry(&reference.from)
                .or_default()
                .push(&reference.to);
        }
        for root in &self.roots {
            if !ids.contains(&root.symbol) {
                return Err("unknown semantic root".into());
            }
        }
        for package in &self.packages {
            if !ids.contains(&package.id) || package.files.is_empty() {
                return Err("invalid semantic package".into());
            }
        }
        let mut live = HashSet::new();
        let mut stack: Vec<_> = self.roots.iter().map(|root| &root.symbol).collect();
        while let Some(id) = stack.pop() {
            if live.insert(id.clone()) {
                stack.extend(edges.get(id).into_iter().flatten().copied());
            }
        }
        Ok(live)
    }
}
