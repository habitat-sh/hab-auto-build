use color_eyre::{
    eyre::{eyre, Context, Result},
    owo_colors::OwoColorize,
};
use std::{collections::HashSet, env, path::PathBuf};
use tracing::{error, info};

use clap::Args;

use crate::{
    check::{Checker, ContextRules, SourceCheck},
    core::{
        AnalysisType, AutoBuildConfig, AutoBuildContext, Dependency, DependencyAnalysis,
        DownloadError, DownloadStatus, PackageDepIdent,
    },
};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// List of packages for which to download source archives
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

    for package in args.packages {
        match run_context.download_source_archive(&package) {
            Ok(statuses) => {
                for status in statuses {
                    match status {
                        DownloadStatus::Downloaded(
                            source_ctx,
                            plan_ctx,
                            source,
                            download_duration,
                        ) => {
                            info!(target: "user-log", "Downloaded sources for {} from {} in {:.3}s", plan_ctx.id, source.url, download_duration.num_milliseconds() as f32 / 1000.0f32);
                            let checker = Checker::new();
                            checker.source_context_check_with_plan(
                                &plan_ctx.context_rules(),
                                &plan_ctx,
                                &source_ctx,
                            );
                        }
                        DownloadStatus::AlreadyDownloaded(source_ctx, plan_ctx, source) => {
                            info!(target: "user-log", "Found existing sources for {} from {}", plan_ctx.id, source.url);
                            let checker = Checker::new();
                            checker.source_context_check_with_plan(
                                &plan_ctx.context_rules(),
                                &plan_ctx,
                                &source_ctx,
                            );
                        }
                        DownloadStatus::NoSource(plan_ctx) => {
                            info!(target: "user-log", "Plan {} has no 'pkg_source' attribute specified", plan_ctx.id);
                        }
                        DownloadStatus::InvalidArchive(plan_ctx, source, actual_shasum) => {
                            error!(target: "user-log", "Downloaded source shasum for {} from {} does not match, expected '{}', found '{}'", plan_ctx.id, source.url, source.shasum, actual_shasum);
                        }
                    }
                }
            }
            Err(DownloadError::PlansNotFound(_)) => {
                error!(target: "user-log", "No plans found for {} in any repo", package);
            }
            Err(err) => return Err(eyre!(err)),
        }
    }

    Ok(())
}
