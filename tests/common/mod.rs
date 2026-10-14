#![allow(dead_code)]
use deadwood_cli::{discover, model::Report, report, resolve};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "deadwood-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let fixture = Self(root);
        for (name, content) in files {
            fixture.write(name, content);
        }
        fixture
    }
    pub fn write(&self, name: &str, content: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    pub fn report(&self) -> Report {
        let project = discover::discover(&self.0, None, true).unwrap();
        assert!(!project.files.is_empty(), "empty fixture");
        report::build(&project, &resolve::build(&project))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub fn has(report: &Report, kind: &str, name: &str) -> bool {
    report
        .findings
        .iter()
        .any(|f| f.kind == kind && f.name == name)
}
