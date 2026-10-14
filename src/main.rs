use clap::{Parser, ValueEnum};
use deadwood_cli::{
    discover,
    model::{Project, Report},
    output, report, resolve,
};
use std::{path::PathBuf, process::ExitCode, time::Instant};

#[derive(Debug, Parser)]
#[command(
    name = "deadwood",
    version,
    about = "Find unused files, dependencies and symbols in TypeScript, Go, Rust and Lua"
)]
struct Cli {
    #[arg(default_value = ".")]
    path: PathBuf,
    #[arg(long,value_enum,default_value_t=Format::Json)]
    format: Format,
    #[arg(long, default_value_t = 4000)]
    max_tokens: usize,
    #[arg(long, conflicts_with = "exclude_tests")]
    include_tests: bool,
    #[arg(long)]
    exclude_tests: bool,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long,value_enum,default_value_t=FailOn::Error)]
    fail_on: FailOn,
    #[arg(long)]
    production: bool,
    #[arg(long)]
    strict: bool,
    #[arg(long)]
    include_entry_exports: bool,
    #[arg(long, value_delimiter = ',')]
    include: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    exclude: Vec<String>,
    #[arg(long, short = 'W')]
    workspace: Vec<String>,
    #[arg(long)]
    no_gitignore: bool,
    #[arg(long)]
    trace: bool,
    #[arg(long)]
    trace_file: Option<String>,
    #[arg(long)]
    trace_export: Option<String>,
    #[arg(long)]
    trace_dependency: Option<String>,
    #[arg(long)]
    debug: bool,
    #[arg(long)]
    performance: bool,
    #[arg(long)]
    duration: bool,
    #[arg(long)]
    memory: bool,
    #[arg(long, conflicts_with = "watch")]
    fix: bool,
    #[arg(long, requires = "fix")]
    dry_run: bool,
    #[arg(long, requires = "fix")]
    allow_remove_files: bool,
    #[arg(long, requires = "fix")]
    verify_command: Option<String>,
    #[arg(long)]
    cache: bool,
    #[arg(long)]
    cache_location: Option<PathBuf>,
    #[arg(long)]
    watch: bool,
    #[arg(long, hide = true)]
    watch_count: Option<usize>,
    #[arg(long, default_value_t = 200)]
    watch_interval: u64,
    #[arg(long)]
    preprocessor: Vec<String>,
    #[arg(long)]
    reporter: Option<String>,
    #[arg(long)]
    allow_compilers: bool,
    #[arg(long, value_delimiter = ',')]
    features: Vec<String>,
    #[arg(long)]
    all_features: bool,
    #[arg(long)]
    no_default_features: bool,
    #[arg(long, value_delimiter = ',')]
    go_tags: Vec<String>,
    /// Use the Go compiler (executes trusted Go tooling; offline by default)
    #[arg(long)]
    semantic_go: bool,
    /// Permit Go dependency downloads, including the bundled adapter dependencies
    #[arg(long, requires = "semantic_go")]
    go_downloads: bool,
    /// Maximum seconds for Go adapter compilation and analysis
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=3600))]
    go_timeout: u64,
    #[arg(long)]
    target: Option<String>,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Json,
    Text,
    Compact,
    Markdown,
    Sarif,
    GithubActions,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum FailOn {
    None,
    High,
    Error,
    Warn,
}
fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("deadwood: {e}");
            ExitCode::from(2)
        }
    }
}
fn run(cli: &Cli) -> Result<u8, String> {
    let mut count = 0;
    let input = std::fs::canonicalize(&cli.path).map_err(|e| e.to_string())?;
    let watch_root = discover::project_root(if input.is_file() {
        input.parent().unwrap()
    } else {
        &input
    });
    loop {
        let stamp = cli.watch.then(|| {
            deadwood_cli::cache::snapshot_with(&watch_root, cli.cache_location.as_deref())
        });
        let start = Instant::now();
        let mut project = discover_project(cli)?;
        let discovered = Instant::now();
        let mut graph = resolve::build(&project);
        let resolved = Instant::now();
        let mut findings = report::build(&project, &graph);
        if cli.fix {
            filter_workspaces(&project, &mut findings, &cli.workspace)?;
            let plan = deadwood_cli::fix::plan_with(
                &project,
                &findings,
                cli.allow_remove_files,
                cli.verify_command.is_some(),
            )?;
            if cli.dry_run {
                findings.fixes = plan.actions;
            } else {
                let backup = deadwood_cli::fix::apply(&plan, cli.verify_command.as_deref())?;
                project = discover_project(cli)?;
                graph = resolve::build(&project);
                findings = report::build(&project, &graph);
                findings.fixes = plan.actions;
                if let Some(backup) = backup {
                    findings
                        .diagnostics
                        .push(deadwood_cli::model::OutputDiagnostic {
                            kind: "fix_backup".into(),
                            message: backup.display().to_string(),
                        });
                }
            }
        }
        if cli.trace
            || cli.trace_file.is_some()
            || cli.trace_export.is_some()
            || cli.trace_dependency.is_some()
        {
            findings.traces = output::traces(
                &project,
                &graph,
                cli.trace_file.as_deref(),
                cli.trace_export.as_deref(),
                cli.trace_dependency.as_deref(),
            );
        }
        filter_workspaces(&project, &mut findings, &cli.workspace)?;
        let code = if !findings.complete {
            2
        } else if findings.findings.iter().any(|f| match cli.fail_on {
            FailOn::None => false,
            FailOn::High => f.confidence == "high" && f.severity == "error",
            FailOn::Error => f.severity == "error",
            FailOn::Warn => true,
        }) {
            1
        } else {
            0
        };
        diagnostics(
            cli, &project, &graph, &findings, start, discovered, resolved,
        );
        let rendered = present(cli, &project, findings)?;
        println!("{rendered}");
        use std::io::Write;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        count += 1;
        if !cli.watch || cli.watch_count.is_some_and(|limit| count >= limit) {
            return Ok(code);
        }
        loop {
            std::thread::sleep(std::time::Duration::from_millis(cli.watch_interval.max(10)));
            let next =
                deadwood_cli::cache::snapshot_with(&project.root, cli.cache_location.as_deref());
            if stamp != Some(next) {
                break;
            }
        }
    }
}
fn filter_workspaces(
    project: &Project,
    report: &mut Report,
    selectors: &[String],
) -> Result<(), String> {
    if selectors.is_empty() {
        return Ok(());
    }
    let selected: std::collections::HashSet<_> = project
        .manifests()
        .filter(|m| {
            let dir = m
                .path
                .parent()
                .unwrap_or(&project.root)
                .strip_prefix(&project.root)
                .unwrap_or(&project.root)
                .to_string_lossy();
            let path = if dir.is_empty() { "." } else { dir.as_ref() };
            let matches = |s: &str| {
                discover::matches_pattern(path, s)
                    || m.package_name
                        .as_ref()
                        .is_some_and(|n| discover::matches_pattern(n, s))
            };
            (selectors.iter().all(|s| s.starts_with('!'))
                || selectors
                    .iter()
                    .filter(|s| !s.starts_with('!'))
                    .any(|s| matches(s)))
                && !selectors
                    .iter()
                    .filter_map(|s| s.strip_prefix('!'))
                    .any(matches)
        })
        .map(|m| &m.path)
        .collect();
    if selected.is_empty() {
        return Err("workspace selector matched no packages".into());
    }
    let matches = |file: &str| {
        project
            .manifests()
            .filter(|m| {
                project
                    .root
                    .join(file)
                    .starts_with(m.path.parent().unwrap_or(&project.root))
            })
            .max_by_key(|m| m.path.components().count())
            .is_some_and(|m| selected.contains(&m.path))
    };
    report.findings.retain(|f| matches(&f.file));
    report.traces.retain(|t| matches(&t.file));
    report.summary.unused_files = report
        .findings
        .iter()
        .filter(|f| f.kind == "unused_file")
        .count();
    report.summary.unused_dependencies = report
        .findings
        .iter()
        .filter(|f| {
            matches!(
                f.kind.as_str(),
                "unused_dependency" | "unused_dev_dependency"
            )
        })
        .count();
    report.ok = report.complete && !report.findings.iter().any(|f| f.severity == "error");
    Ok(())
}
fn discover_project(cli: &Cli) -> Result<Project, String> {
    discover::discover_with(&cli.path, cli.config.as_deref(), cli.include_tests, |c| {
        c.include_tests |= cli.include_tests;
        if cli.exclude_tests {
            c.include_tests = false;
        }
        c.production |= cli.production;
        c.strict |= cli.strict;
        c.include_entry_exports |= cli.include_entry_exports || cli.all;
        c.no_gitignore |= cli.no_gitignore;
        c.allow_compilers = cli.allow_compilers;
        c.semantic_go = cli.semantic_go;
        c.go_downloads = cli.go_downloads;
        c.go_timeout = Some(cli.go_timeout);
        if !cli.features.is_empty() {
            c.features = cli.features.clone();
        }
        c.all_features |= cli.all_features;
        c.no_default_features |= cli.no_default_features;
        if !cli.go_tags.is_empty() {
            c.go_tags = cli.go_tags.clone();
        }
        if cli.target.is_some() {
            c.target = cli.target.clone();
        }
        if cli.cache || cli.cache_location.is_some() {
            c.cache = Some(
                cli.cache_location
                    .clone()
                    .unwrap_or_else(|| cli.path.join(".deadwood-cache")),
            );
        }
        if !cli.include.is_empty() {
            c.include = cli.include.clone();
        }
        if cli.all {
            c.include = report::ISSUE_TYPES.iter().map(|s| s.to_string()).collect();
        }
        for kind in &cli.exclude {
            c.rules.insert(kind.clone(), deadwood_cli::model::Rule::Off);
        }
    })
}

fn diagnostics(
    cli: &Cli,
    project: &Project,
    graph: &deadwood_cli::model::Graph,
    findings: &Report,
    start: Instant,
    discovered: Instant,
    resolved: Instant,
) {
    if cli.debug {
        eprintln!(
            "packages={} files={} roots={} reachable={}",
            project.manifests().count(),
            project.files.len(),
            graph.roots.len(),
            findings.summary.reachable_files
        );
        for root in &graph.roots {
            eprintln!(
                "entry {}: {}",
                project.files[root.file].display_path, root.reason
            );
        }
    }
    if cli.performance {
        eprintln!(
            "discover={:?} resolve={:?} report={:?}",
            discovered - start,
            resolved - discovered,
            resolved.elapsed()
        );
    }
    if cli.duration {
        eprintln!("duration={:?}", start.elapsed());
    }
    if cli.memory
        && let Ok(result) = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
    {
        eprintln!("rss_kib={}", String::from_utf8_lossy(&result.stdout).trim());
    }
}

fn present(cli: &Cli, project: &Project, findings: Report) -> Result<String, String> {
    let presented = output::preprocess(findings, &cli.preprocessor, &project.root)?;
    Ok(if let Some(command) = &cli.reporter {
        output::command(
            command,
            &serde_json::to_string(&presented).map_err(|e| e.to_string())?,
            &project.root,
        )?
    } else {
        match cli.format {
            Format::Json => report::render_json(presented, cli.max_tokens),
            Format::Text | Format::Compact => report::render_text(&presented),
            Format::Markdown => output::markdown(&presented),
            Format::Sarif => output::sarif(&presented),
            Format::GithubActions => output::github(&presented),
        }
    })
}
