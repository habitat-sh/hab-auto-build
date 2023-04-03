use clap::Args;
use color_eyre::eyre::{eyre, Context, Result};
use owo_colors::OwoColorize;
use std::{env, fmt::Write, path::PathBuf};
use tracing::{error, info};

use crate::{
    check::{LeveledArtifactCheckViolation, LeveledSourceCheckViolation, ViolationLevel},
    core::{
        AutoBuildConfig, AutoBuildContext, BuildPlan, PackageDepGlob,
        PackageTarget, PlanCheckStatus,
    },
};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Output format
    #[arg(value_enum, short = 'f', long, default_value_t = OutputFormat::Plain, requires = "dry_run")]
    format: OutputFormat,
    /// Only diplay the number of issues with each package
    #[arg(short, long)]
    summary: bool,
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

    let package_indices = run_context.glob_deps(&args.packages, PackageTarget::default())?;
    if package_indices.is_empty() && !run_context.is_empty() && !args.packages.is_empty() {
        error!(target: "user-log",
            "No packages found matching patterns: {}",
            serde_json::to_string(&args.packages).unwrap()
        );
        return Ok(());
    }
    for package_index in package_indices {
        let package = run_context.dep(package_index);
        match run_context.package_check(package_index) {
            Ok(check_status) => match check_status {
                PlanCheckStatus::CheckSucceeded(source_violations, artifact_violations) => {
                    output_violations(
                        &source_violations,
                        &artifact_violations,
                        format!("{:?}", package).as_str(),
                        true,
                        args.summary,
                    )?;
                }
                PlanCheckStatus::ArtifactNotFound => {
                    info!(target: "user-ui", "{}: {:?}: No artifact found","warning".bold().yellow(), package.red())
                }
            },
            Err(err) => {
                info!(target: "user-ui", "{}: Failed to check package {:?}: {:#}","error".bold().red(), package, err)
            }
        };
    }
    Ok(())
}

pub(crate) fn output_violations(
    source_violations: &[LeveledSourceCheckViolation],
    artifact_violations: &[LeveledArtifactCheckViolation],
    package: &str,
    header: bool,
    summary: bool,
) -> Result<()> {
    let source_error_count = source_violations
        .iter()
        .filter(|v| v.level == ViolationLevel::Error)
        .count();
    let source_warning_count = source_violations
        .iter()
        .filter(|v| v.level == ViolationLevel::Warn)
        .count();
    let artifact_error_count = artifact_violations
        .iter()
        .filter(|v| v.level == ViolationLevel::Error)
        .count();
    let artifact_warning_count = artifact_violations
        .iter()
        .filter(|v| v.level == ViolationLevel::Warn)
        .count();
    if header {
        let mut header = String::new();
        write!(header, "{}:", package.white())?;
        if artifact_error_count + source_error_count != 0 {
            write!(
                &mut header,
                " {}",
                format!("{} errors", artifact_error_count + source_error_count)
                    .red()
                    .bold()
            )?;
        }
        if artifact_warning_count + source_warning_count != 0 {
            write!(
                &mut header,
                " {}",
                format!("{} warnings", artifact_warning_count + source_warning_count)
                    .yellow()
                    .bold()
            )?;
        }
        if artifact_error_count + artifact_warning_count + source_error_count + source_warning_count
            != 0
        {
        } else {
            write!(
                &mut header,
                "{}",
                format!(" all checks passed").green().bold()
            )?;
        }
        info!(target: "user-ui", "{}", header);
    }
    if !summary {
        for violation in source_violations {
            if violation.level == ViolationLevel::Off {
                continue;
            }
            info!(target: "user-ui", "     {}", violation);
        }
        for violation in artifact_violations {
            if violation.level == ViolationLevel::Off {
                continue;
            }
            info!(target: "user-ui", "     {}", violation);
        }
    }
    Ok(())
}

fn output_plain(_dry_run: BuildPlan) -> Result<()> {
    todo!()
}

fn output_json(_dry_run: BuildPlan) -> Result<()> {
    todo!()
}
