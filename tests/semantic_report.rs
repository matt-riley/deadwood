mod common;
use common::Fixture;
use deadwood_cli::{discover, report, resolve, semantic::SemanticGraph};
#[test]
fn incomplete_or_invalid_backend_graph_cannot_be_a_clean_report() {
    let f = Fixture::new(&[
        ("Cargo.toml", "[package]\nname='probe'\nversion='0.1.0'\n"),
        ("src/main.rs", "fn main(){}"),
    ]);
    let project = discover::discover(f.path(), None, true).unwrap();
    for (version, complete) in [(1, false), (2, true)] {
        let mut graph = resolve::build(&project);
        graph.semantic.push(SemanticGraph {
            v: version,
            language: deadwood_cli::model::Language::Go,
            complete,
            profile: Default::default(),
            declarations: vec![],
            references: vec![],
            roots: vec![],
            packages: vec![],
            diagnostics: vec![],
        });
        let r = report::build(&project, &graph);
        assert!(!r.complete);
        assert!(!r.ok);
    }
}
