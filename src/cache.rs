use crate::{model::FileRecord, parse};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::Path,
};
const VERSION: u32 = 3;
#[derive(Serialize, Deserialize)]
struct Cached {
    version: u32,
    record: FileRecord,
}
pub fn parse(
    path: &Path,
    display: String,
    source: &str,
    cache: Option<&Path>,
    config: &crate::model::Config,
) -> FileRecord {
    let key = hash(&(
        path,
        env!("CARGO_PKG_VERSION"),
        VERSION,
        &config.features,
        config.all_features,
        config.include_tests,
        config.production,
        &config.target,
    ));
    let cached = cache.map(|dir| dir.join(format!("{key:016x}.json")));
    if let Some(p) = &cached
        && let Ok(text) = fs::read_to_string(p)
        && let Ok(saved) = serde_json::from_str::<Cached>(&text)
        && saved.version == VERSION
        && saved.record.source == source
        && saved.record.display_path == display
        && saved.record.path == path
    {
        return saved.record;
    }
    let record = parse::parse_with(path.to_path_buf(), display, source, config);
    if let Some(p) = cached
        && fs::create_dir_all(p.parent().unwrap()).is_ok()
        && let Ok(text) = serde_json::to_string(&Cached {
            version: VERSION,
            record: record.clone(),
        })
    {
        let temporary = p.with_extension(format!("{}.tmp", std::process::id()));
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(&temporary, &p);
        }
        let _ = fs::remove_file(&temporary);
    }
    record
}
fn hash(value: &impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
pub fn snapshot(root: &Path) -> u64 {
    snapshot_with(root, None)
}
pub fn snapshot_with(root: &Path, cache: Option<&Path>) -> u64 {
    let excluded = cache.map(|p| p.to_path_buf());
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(false)
        .filter_entry(move |e| {
            !excluded.as_ref().is_some_and(|p| e.path().starts_with(p))
                && !matches!(
                    e.file_name().to_str(),
                    Some(
                        ".git"
                            | "target"
                            | "node_modules"
                            | ".deadwood-cache"
                            | ".deadwood-backup"
                            | "coverage"
                            | ".pi"
                            | "dist"
                            | "build"
                            | "vendor"
                    )
                )
        });
    let mut items = Vec::new();
    for e in walker
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
    {
        if let Ok(bytes) = fs::read(e.path()) {
            items.push((e.path().to_path_buf(), hash(&bytes)));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    hash(&items)
}
