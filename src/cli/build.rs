use clap::Args;
use color_eyre::{
    eyre::{eyre, Context, Result},
    owo_colors::OwoColorize,
};

use std::{env, path::PathBuf};
use tracing::info;

use crate::core::{AutoBuildConfig, AutoBuildContext, BuildDryRun, PackageDepIdent};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Output format
    #[arg(value_enum, short = 'f', long, default_value_t = OutputFormat::Plain, requires = "dry_run")]
    format: OutputFormat,
    /// Do a dry run of the build, does not actually build anything
    #[arg(short = 'd', long)]
    dry_run: bool,
    /// List of packages to build
    packages: Vec<PackageDepIdent>,
}

pub(crate) fn execute(args: Params) -> Result<()> {
    let config_path = args.config_path.unwrap_or(
        env::current_dir()
            .context("Failed to determine current working directory")?
            .join("hab-auto-build.json"),
    );
    let config = AutoBuildConfig::new(&config_path)?;

    let run_context = AutoBuildContext::new(&config, &config_path)
        .with_context(|| eyre!("Failed to initialize run"))?;

    if args.dry_run {
        let dry_run = run_context.build_dry_run(args.packages);
        match args.format {
            OutputFormat::Plain => output_plain(dry_run)?,
            OutputFormat::Json => output_json(dry_run)?,
        }
    } else {
        run_context.build(args.packages)?;
    }
    Ok(())
}

fn output_plain(dry_run: BuildDryRun) -> Result<()> {
    info!(target: "user-ui", "{}", "Plan Build Order:");
    for (index, (dependency, studio_dep, remote_deps, causes)) in dry_run.order.iter().enumerate() {
        if !remote_deps.is_empty() {
            info!(target: "user-ui",
                "{:>4} - {} {} {} {}",
                (index + 1).to_string(),
                studio_dep.map_or(None, |d| Some(d.to_dep_ident())).map(|d| format!("[{}]", d)).unwrap_or_else(|| String::from("[native]")),
                dependency.to_dep_ident(),
                causes
                    .iter()
                    .map(|cause| {
                        cause.to_emoji()
                    })
                    .collect::<Vec<_>>().join(""),
                format!("[{} remote deps]", remote_deps.len()).yellow(),

            );
        } else {
            info!(target: "user-ui",
                "{:>4} - {} {} {}",
                (index + 1).to_string(),
                studio_dep.map_or(None, |d| Some(d.to_dep_ident())).map(|d| format!("[{}]", d)).unwrap_or_else(|| String::from("[native]")),
                dependency.to_dep_ident(),
                causes
                .iter()
                .map(|cause| {
                    cause.to_emoji()
                })
                .collect::<Vec<_>>().join(""),

            );
        }
    }
    Ok(())
}

fn output_json(_dry_run: BuildDryRun) -> Result<()> {
    todo!()
}
