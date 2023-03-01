use clap::Args;
use color_eyre::{
    eyre::{eyre, Context, Result},
    owo_colors::OwoColorize,
};

use std::{env, path::PathBuf};
use tracing::{error, info, warn};

use crate::{core::{
    AutoBuildConfig, AutoBuildContext, BuildDryRun, CheckStatus, PackageDepGlob, PackageDepIdent,
    PackageTarget,
}, check::ViolationLevel};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Output format
    #[arg(value_enum, short = 'f', long, default_value_t = OutputFormat::Plain, requires = "dry_run")]
    format: OutputFormat,
    /// List of packages to check
    packages: Vec<PackageDepGlob>,
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

    let packages = run_context.glob_deps(&args.packages, PackageTarget::default())?;
    for package in packages {
        match run_context.check(&package) {
            Ok(check_statuses) => {
                for check_status in check_statuses {
                    match check_status {
                        CheckStatus::CheckSucceeded(package, _, artifact_violations) => {
                            let error_count = artifact_violations.iter().filter(|v| v.level == ViolationLevel::Error).count();
                            let warning_count = artifact_violations.iter().filter(|v| v.level == ViolationLevel::Warn).count();
                            info!(target: "user-log", "Checked package {}: {} errors, {} warnings", package, error_count, warning_count);
                        }
                        CheckStatus::ArtifactNotFound(package) => {
                            warn!(target: "user-log", "No artifact found for package {}", package)
                        }
                    }
                }
            }
            Err(err) => {
                error!(target: "user-log", "Failed to check package {}: {}", package, err)
            }
        };
    }
    Ok(())
}

fn output_plain(dry_run: BuildDryRun) -> Result<()> {
    todo!()
}

fn output_json(_dry_run: BuildDryRun) -> Result<()> {
    todo!()
}
