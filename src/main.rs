use clap::{Parser, ValueEnum};
use deadwood_cli::{discover, report, resolve};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "deadwood",
    version,
    about = "Find unused files and dependencies across TypeScript, Go, Rust and Lua"
)]
struct Cli {
    #[arg(default_value = ".")]
    path: PathBuf,

    #[arg(long, value_enum, default_value_t = Format::Json)]
    format: Format,

    #[arg(long, default_value_t = 4000)]
    max_tokens: usize,

    #[arg(long)]
    include_tests: bool,

    #[arg(long)]
    all: bool,

    #[arg(long)]
    config: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = FailOn::None)]
    fail_on: FailOn,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Json,
    Text,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FailOn {
    None,
    High,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("deadwood: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    let project = discover::discover(&cli.path, cli.config.as_deref(), cli.include_tests)?;
    let graph = resolve::build(&project);
    let output = report::build(&project, &graph);

    match cli.format {
        Format::Json => println!("{}", report::render_json(output.clone(), cli.max_tokens)),
        Format::Text => print!("{}", report::render_text(&output)),
    }

    let failed = match cli.fail_on {
        FailOn::None => false,
        FailOn::High => output
            .findings
            .iter()
            .any(|finding| finding.confidence == "high"),
    };
    let _ = cli.all;
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
