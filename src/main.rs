use clap::{CommandFactory, Parser, Subcommand};
use pctl::{
    Result,
    config::{Project, validate_environment},
    environment,
    execution::{self, RunOptions},
    generation, planning,
    process::{Cancellation, SignalGuard},
    reporting, tools,
};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Plan and run repository operations")]
struct Cli {
    #[arg(long, global = true, default_value = "pctl.toml")]
    manifest: PathBuf,
    #[arg(long, global = true, default_value = "local")]
    profile: String,
    /// Explicit dotenv file; real process environment takes precedence.
    #[arg(long, global = true)]
    env_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    List,
    /// Check manifest, directories, and declared tool version probes.
    Doctor,
    Plan {
        task: String,
        #[arg(long = "param", value_parser = parameter)]
        parameters: Vec<(String, String)>,
        #[arg(long)]
        json: bool,
    },
    /// Print dependency edges as JSON.
    Graph,
    Run {
        task: String,
        #[arg(long = "param", value_parser = parameter)]
        parameters: Vec<(String, String)>,
        #[arg(long)]
        allow_destructive: bool,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..))]
        jobs: u16,
        /// Bypass task fingerprint cache.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "terminal", value_parser = ["terminal", "json", "ndjson", "junit"])]
        format: String,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Generate CLI shell completions without loading a project.
    Completions {
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Check {
        #[arg(long)]
        values: bool,
    },
    Generate {
        #[arg(long, default_value = ".pctl/generated")]
        output: PathBuf,
        /// Compare generated artifacts without writing.
        #[arg(long)]
        check: bool,
    },
    /// Show a variable's declaration without reading its value.
    Explain { name: String },
}

fn parameter(input: &str) -> Result<(String, String)> {
    let (key, value) = input.split_once('=').ok_or("Use --param name=value")?;
    if key.is_empty() {
        return Err("Parameter name cannot be empty".into());
    }
    Ok((key.into(), value.into()))
}

fn parameter_map(parameters: Vec<(String, String)>) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for (key, value) in parameters {
        if map.insert(key.clone(), value).is_some() {
            return Err(format!("Repeated parameter: {key}"));
        }
    }
    Ok(map)
}

fn print_plan(plan: &pctl::model::Plan, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(plan).map_err(|e| e.to_string())?
        );
    } else {
        for task in &plan.tasks {
            println!(
                "{}{}",
                task.id,
                if task.destructive {
                    " [destructive]"
                } else {
                    ""
                }
            );
        }
    }
    Ok(())
}

fn run(cli: Cli) -> Result<i32> {
    if let Commands::Completions { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "pctl", &mut std::io::stdout());
        return Ok(0);
    }
    let project = Project::load(&cli.manifest)?;
    let cancellation = Cancellation::default();
    let _signals = SignalGuard::install(cancellation.clone())?;
    match cli.command {
        Commands::List => {
            for (id, task) in &project.manifest.tasks {
                println!("{id}\t{}", task.description);
            }
        }
        Commands::Graph => {
            let edges: BTreeMap<_, _> = project
                .manifest
                .tasks
                .iter()
                .map(|(id, task)| (id, &task.depends_on))
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&edges).map_err(|e| e.to_string())?
            );
        }
        Commands::Doctor => {
            for (id, task) in &project.manifest.tasks {
                let directory = pctl::paths::inside(
                    &project.root,
                    std::path::Path::new(&task.working_directory),
                )?;
                if !directory.is_dir() {
                    return Err(format!("{id}: missing working directory"));
                }
            }
            tools::probe(
                &project.manifest.tools,
                project.manifest.tools.keys().cloned(),
                &project.root,
                &environment::load(None)?,
                &cancellation,
            )?;
            println!("Manifest, working directories, and declared tools are valid.");
        }
        Commands::Config { command } => match command {
            ConfigCommand::Check { values } => {
                if values {
                    let mut source = project
                        .manifest
                        .profiles
                        .get(&cli.profile)
                        .map(|p| p.environment.clone())
                        .unwrap_or_default();
                    source.extend(environment::load(cli.env_file.as_deref())?);
                    validate_environment(&project.manifest, &cli.profile, &source)?;
                }
                println!(
                    "Configuration contracts are valid{}.",
                    if values {
                        " for the selected environment"
                    } else {
                        " (values not checked)"
                    }
                );
            }
            ConfigCommand::Generate { output, check } => {
                let output = pctl::paths::inside(&project.root, &output)?;
                generation::generate(&project.manifest, &cli.profile, &output, check)?;
                println!(
                    "Generated contracts {}.",
                    if check { "match" } else { "written" }
                );
            }
            ConfigCommand::Explain { name } => {
                let contract = project
                    .manifest
                    .variables
                    .get(&name)
                    .ok_or("Unknown environment variable")?;
                println!(
                    "{name}\ntype: {:?}\nvisibility: {:?}\nconsumers: {}\nrequired in: {}\nbrowser exposed: {}",
                    contract.kind,
                    contract.visibility,
                    contract.consumers.join(", "),
                    contract.required_in.join(", "),
                    contract.browser_exposed
                );
            }
        },
        Commands::Plan {
            task,
            parameters,
            json,
        } => {
            print_plan(
                &planning::build(
                    &project.manifest,
                    &task,
                    &cli.profile,
                    &parameter_map(parameters)?,
                )?,
                json,
            )?;
        }
        Commands::Run {
            task,
            parameters,
            allow_destructive,
            jobs,
            force,
            dry_run,
            format,
        } => {
            let plan = planning::build(
                &project.manifest,
                &task,
                &cli.profile,
                &parameter_map(parameters)?,
            )?;
            if dry_run {
                print_plan(&plan, true)?;
                return Ok(0);
            }
            execution::preflight(&plan, &project.root, allow_destructive)?;
            let source = environment::load(cli.env_file.as_deref())?;
            let mut options = RunOptions {
                jobs: jobs.into(),
                allow_destructive,
                force,
                cancellation: cancellation.clone(),
                secrets: environment::secrets(&project.manifest, &source),
                ..Default::default()
            };
            for task in &plan.tasks {
                options.environments.insert(
                    task.id.clone(),
                    environment::resolve(&project.manifest, task, &cli.profile, &source)?,
                );
            }
            options.tool_versions = tools::probe(
                &plan.tools,
                plan.tasks.iter().flat_map(|task| task.requires.clone()),
                &project.root,
                &source,
                &cancellation,
            )?;
            let report = execution::execute_with(
                &plan,
                &project.root,
                &options,
                &execution::LocalProcessExecutor,
            )?;
            println!("{}", reporting::render(&report, &format)?);
            return Ok(report.exit_code);
        }
        Commands::Completions { .. } => unreachable!(),
    }
    Ok(0)
}

fn main() {
    match run(Cli::parse()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("pctl: {error}");
            std::process::exit(2);
        }
    }
}
