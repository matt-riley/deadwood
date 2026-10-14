use deadwood_cli::{model::Language, semantic::*};
fn graph() -> SemanticGraph {
    let symbol = |id: &str, name: &str, start| Declaration {
        id: SymbolId(id.into()),
        name: name.into(),
        kind: deadwood_cli::model::SymbolKind::Value,
        span: SourceSpan {
            file: "main.go".into(),
            start,
            end: start + 10,
            line: 1,
        },
        owner: None,
        exported: false,
        reportable: true,
    };
    SemanticGraph {
        v: 1,
        language: Language::Go,
        complete: true,
        profile: Default::default(),
        declarations: vec![
            symbol("main", "main", 0),
            symbol("a", "same", 20),
            symbol("b", "same", 40),
            symbol("dead1", "dead1", 60),
            symbol("dead2", "dead2", 80),
        ],
        references: vec![],
        roots: vec![SemanticRoot {
            symbol: SymbolId("main".into()),
            reason: "entry".into(),
        }],
        packages: vec![],
        diagnostics: vec![],
    }
}
fn edge(from: &str, to: &str) -> Reference {
    Reference {
        from: SymbolId(from.into()),
        to: SymbolId(to.into()),
        span: SourceSpan {
            file: "main.go".into(),
            start: 0,
            end: 1,
            line: 1,
        },
    }
}
#[test]
fn identities_and_reachability_do_not_keep_an_unused_cycle_alive() {
    let mut g = graph();
    g.references = vec![
        edge("main", "a"),
        edge("a", "a"),
        edge("dead1", "dead2"),
        edge("dead2", "dead1"),
    ];
    let live = g.reachable().unwrap();
    assert_eq!(live.len(), 2);
    assert!(live.contains(&SymbolId("a".into())));
    assert!(!live.contains(&SymbolId("b".into())));
    assert!(!live.contains(&SymbolId("dead1".into())));
    let encoded = serde_json::to_string(&g).unwrap();
    assert_eq!(
        serde_json::from_str::<SemanticGraph>(&encoded)
            .unwrap()
            .reachable()
            .unwrap(),
        live
    );
}
#[test]
fn malformed_ir_is_not_a_clean_empty_analysis() {
    let mut g = graph();
    g.declarations.push(g.declarations[0].clone());
    assert!(g.reachable().is_err());
    g = graph();
    g.references.push(edge("main", "missing"));
    assert!(g.reachable().is_err());
    g = graph();
    g.roots[0].symbol = SymbolId("missing".into());
    assert!(g.reachable().is_err());
    g = graph();
    g.declarations[0].span.end = 0;
    g.declarations[0].span.start = 1;
    assert!(g.reachable().is_err());
    g = graph();
    g.declarations[0].owner = Some(SymbolId("missing".into()));
    assert!(g.reachable().is_err());
    g = graph();
    g.v = 2;
    assert!(g.reachable().is_err());
    g = graph();
    g.declarations.clear();
    g.references.clear();
    g.roots.clear();
    assert!(g.reachable().is_err());
}
#[test]
fn incomplete_graph_cannot_be_used_for_findings() {
    let mut g = graph();
    g.complete = false;
    assert!(g.reachable().is_err());
}
