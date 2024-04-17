use std::{collections::HashMap, path::PathBuf};

use clap::{arg, Args};
use color_eyre::eyre::{eyre, Context, Result};
use owo_colors::OwoColorize;
use tracing::info;

use crate::{
    cli::output::OutputFormat,
    core::{
        AutoBuildConfig, AutoBuildContext, ChangeDetectionMode, PackageDiff, PackageName,
        PackageOrigin, PackageTarget,
    },
};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration for source repos
    #[arg(short = 's', long)]
    source_config_path: PathBuf,
    /// Path to hab auto build configuration for target repos
    #[arg(short = 't', long)]
    target_config_path: PathBuf,
    /// Output format
    #[arg(value_enum, short = 'f', long, default_value_t = OutputFormat::Plain)]
    format: OutputFormat,
}

pub(crate) fn execute(args: Params) -> Result<()> {
    let source_config_path = args.source_config_path;
    let target_config_path = args.target_config_path;
    let source_config = AutoBuildConfig::new(&source_config_path)?;
    let target_config = AutoBuildConfig::new(&target_config_path)?;

    let source_run_context = AutoBuildContext::new(
        &source_config,
        &source_config_path,
        ChangeDetectionMode::Disk,
    )
    .with_context(|| eyre!("Failed to initialize run"))?;
    let target_run_context = AutoBuildContext::new(
        &target_config,
        &target_config_path,
        ChangeDetectionMode::Disk,
    )
    .with_context(|| eyre!("Failed to initialize run"))?;

    let diffs = target_run_context.compare(&source_run_context);

    match args.format {
        OutputFormat::Plain => output_plain(diffs)?,
        OutputFormat::Json => todo!(),
    }
    Ok(())
}

fn output_plain(
    package_diffs: HashMap<(PackageTarget, PackageOrigin, PackageName), PackageDiff>,
) -> Result<()> {
    let mut print_header = true;
    for ((_, origin, name), package_diff) in package_diffs.iter() {
        if package_diff.source != package_diff.target {
            if print_header {
                info!(target: "user-ui", "{}", "Updated Plans".white().bold());
                print_header = false;
            }
            info!(target: "user-ui", "{}", format!("{}: {} -> {}", format!("{}/{}", origin, name).yellow(), serde_json::to_string(&package_diff.source).unwrap().white(), serde_json::to_string(&package_diff.target).unwrap().blue()));
        }
    }
    let mut print_header = true;
    for ((_, origin, name), package_diff) in package_diffs.iter() {
        if package_diff.source == package_diff.target {
            if print_header {
                info!(target: "user-ui", "{}", "Unchanged Plans ".white().bold());
                print_header = false;
            }
            info!(target: "user-ui", "{}", format!("{}: {}", format!("{}/{}", origin, name).green(), serde_json::to_string(&package_diff.source).unwrap()).white());
        }
    }
    Ok(())
}
