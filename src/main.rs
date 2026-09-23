use clap::{Parser, Subcommand};
use pctl::{
    Result,
    config::{Project, validate_environment},
    execution, planning,
};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Plan and run repository operations")]
struct Cli {
    #[arg(long, global = true, default_value = "pctl.toml")]
    manifest: PathBuf,
    #[arg(long, global = true, default_value = "local")]
    profile: String,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List declared tasks and their descriptions.
    List,
    /// Check the manifest and working directories without running tasks.
    Doctor,
    /// Inspect an execution plan. Parameters are not a secret transport.
    Plan {
        task: String,
        #[arg(long = "param", value_parser = parameter)]
        parameters: Vec<(String, String)>,
        #[arg(long)]
        json: bool,
    },
    /// Run a task and its dependencies sequentially, stopping on failure.
    Run {
        task: String,
        #[arg(long = "param", value_parser = parameter)]
        parameters: Vec<(String, String)>,
        #[arg(long)]
        allow_destructive: bool,
    },
    /// Validate environment contracts.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Check declarations; optionally validate current process values too.
    Check {
        #[arg(long)]
        values: bool,
    },
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

fn run(cli: Cli) -> Result<i32> {
    let project = Project::load(&cli.manifest)?;
    match cli.command {
        Commands::List => {
            for (id, task) in &project.manifest.tasks {
                println!("{id}\t{}", task.description);
            }
        }
        Commands::Doctor => {
            for (id, task) in &project.manifest.tasks {
                let directory = project
                    .root
                    .join(&task.working_directory)
                    .canonicalize()
                    .map_err(|_| format!("{id}: missing working directory"))?;
                if !directory.starts_with(&project.root) || !directory.is_dir() {
                    return Err(format!("{id}: working directory must be inside project"));
                }
            }
            println!("Manifest and task directories are valid. External tools were not probed.");
        }
        Commands::Config {
            command: ConfigCommand::Check { values },
        } => {
            if values {
                let environment = std::env::vars_os()
                    .filter_map(|(key, value)| {
                        Some((key.into_string().ok()?, value.into_string().ok()?))
                    })
                    .collect();
                validate_environment(&project.manifest, &cli.profile, &environment)?;
            }
            println!(
                "Configuration contracts are valid{}.",
                if values {
                    " for the current environment"
                } else {
                    " (values not checked)"
                }
            );
        }
        Commands::Plan {
            task,
            parameters,
            json,
        } => {
            let plan = planning::build(
                &project.manifest,
                &task,
                &cli.profile,
                &parameter_map(parameters)?,
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?
                );
            } else {
                println!("Profile: {}", plan.profile);
                for task in plan.tasks {
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
        }
        Commands::Run {
            task,
            parameters,
            allow_destructive,
        } => {
            let plan = planning::build(
                &project.manifest,
                &task,
                &cli.profile,
                &parameter_map(parameters)?,
            )?;
            let report = execution::execute(
                &plan,
                &project.root,
                allow_destructive,
                &mut execution::LocalProcessExecutor,
            )?;
            if let Some(task) = report.failed {
                eprintln!("{task} failed with exit code {}", report.exit_code);
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
