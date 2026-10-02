use clap::{Parser, Subcommand};
use pctl::{
    Result,
    config::{DEFAULT_PROFILE, Project, validate_environment, validate_profile},
    environment,
    execution::{self, RunOptions, TaskStatus},
    generation, planning,
    process::{Cancellation, SignalGuard},
    reporting::{self, Format},
    tools,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(version, about = "Plan and run repository operations")]
struct Cli {
    #[arg(long, global = true, default_value = "pctl.toml")]
    manifest: PathBuf,
    #[arg(long, global = true, default_value = DEFAULT_PROFILE)]
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
        #[arg(long, value_enum, default_value_t = Format::Terminal)]
        format: Format,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
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

/// Write to stdout, treating a closed pipe (`pctl list | head`) as a reader that
/// is done rather than an error. `println!` would panic instead. SIGPIPE stays
/// ignored on purpose: killing pctl outright would skip child-process cleanup.
fn write_stdout(bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(bytes).and_then(|()| stdout.flush()) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(source) => Err(pctl::error::Error::Io {
            context: "Cannot write to stdout".into(),
            source,
        }),
        Ok(()) => Ok(()),
    }
}

macro_rules! out {
    ($($argument:tt)*) => {
        write_stdout(format!("{}\n", format_args!($($argument)*)).as_bytes())?
    };
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
            return Err(format!("Repeated parameter: {key}").into());
        }
    }
    Ok(map)
}

fn print_plan(plan: &pctl::model::Plan, json: bool) -> Result<()> {
    if json {
        out!("{}", serde_json::to_string_pretty(plan)?);
    } else {
        for task in &plan.tasks {
            out!(
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
    let project = Project::load(&cli.manifest)?;
    let cancellation = Cancellation::default();
    let _signals = SignalGuard::install(cancellation.clone())?;
    match cli.command {
        Commands::List => {
            for (id, task) in &project.manifest.tasks {
                out!("{id}\t{}", task.description);
            }
        }
        Commands::Graph => {
            let edges: BTreeMap<_, _> = project
                .manifest
                .tasks
                .iter()
                .map(|(id, task)| (id, &task.depends_on))
                .collect();
            out!("{}", serde_json::to_string_pretty(&edges)?);
        }
        Commands::Doctor => {
            let mut problems = Vec::new();
            for (id, task) in &project.manifest.tasks {
                match pctl::paths::inside(
                    &project.root,
                    std::path::Path::new(&task.working_directory),
                ) {
                    Ok(directory) if directory.is_dir() => {}
                    Ok(_) => problems.push(format!("{id}: missing working directory")),
                    Err(error) => problems.push(format!("{id}: {error}")),
                }
            }
            let probes = tools::probe_all(
                &project.manifest.tools,
                project.manifest.tools.keys().cloned(),
                &project.root,
                &environment::load(None)?,
                &cancellation,
            )?;
            problems.extend(
                probes
                    .into_iter()
                    .filter_map(|(_, outcome)| outcome.err().map(|e| e.to_string())),
            );
            if !problems.is_empty() {
                return Err(pctl::error::Error::Validation(problems));
            }
            out!("Manifest, working directories, and declared tools are valid.");
        }
        Commands::Config { command } => match command {
            ConfigCommand::Check { values } => {
                if values {
                    validate_profile(&project.manifest, &cli.profile)?;
                    let mut source = project
                        .manifest
                        .profiles
                        .get(&cli.profile)
                        .map(|p| p.environment.clone())
                        .unwrap_or_default();
                    source.extend(environment::load(cli.env_file.as_deref())?);
                    validate_environment(&project.manifest, &cli.profile, &source)?;
                }
                out!(
                    "Configuration contracts are valid{}.",
                    if values {
                        " for the selected environment"
                    } else {
                        " (values not checked)"
                    }
                );
            }
            ConfigCommand::Generate { output, check } => {
                validate_profile(&project.manifest, &cli.profile)?;
                let output = pctl::paths::inside(&project.root, &output)?;
                generation::generate(&project.manifest, &cli.profile, &output, check)?;
                out!(
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
                out!(
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
            validate_profile(&project.manifest, &cli.profile)?;
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
            validate_profile(&project.manifest, &cli.profile)?;
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
            if format == Format::Ndjson {
                // Stream each task as it finishes so CI can follow progress; the
                // observer cannot fail the run, and a closed pipe is already quiet.
                options.on_task_finished = Some(Arc::new(|task| {
                    if let Ok(line) = reporting::task_event(task) {
                        let _ = write_stdout(format!("{line}\n").as_bytes());
                    }
                }));
            }
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
            if format == Format::Ndjson {
                // Finished tasks were already streamed; only never-started ones remain.
                for task in &report.tasks {
                    if task.status == TaskStatus::Skipped {
                        out!("{}", reporting::task_event(task)?);
                    }
                }
                out!("{}", reporting::run_event(&report)?);
            } else {
                out!("{}", reporting::render(&report, format)?);
            }
            return Ok(report.exit_code);
        }
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
