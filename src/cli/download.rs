use color_eyre::eyre::{eyre, Context, Result};

use std::{env, path::PathBuf};
use tracing::{error, info};

use clap::Args;

use crate::{
    cli::check::output_violations,
    core::{AutoBuildConfig, AutoBuildContext, DownloadStatus, PackageDepGlob, PackageTarget},
};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Check the source archive against the plan for issues
    #[arg(short, long, default_value_t = false)]
    check_source: bool,
    /// List of packages for which to download source archives
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
        let dep = run_context.dep(package_index);
        info!(target: "user-log", "Downloading source for {:?}", dep);
        match run_context.download_dep_source(package_index, args.check_source) {
            Ok(status) => match status {
                DownloadStatus::Downloaded(
                    _source_ctx,
                    plan_ctx,
                    source,
                    download_duration,
                    source_violations,
                ) => {
                    info!(target: "user-log", "Downloaded sources for {} from {} in {:.3}s", plan_ctx.id, source.url, download_duration.num_milliseconds() as f32 / 1000.0f32);
                    if args.check_source {
                        output_violations(&source_violations, &[], "", false, false)?;
                    }
                }
                DownloadStatus::AlreadyDownloaded(
                    _source_ctx,
                    plan_ctx,
                    source,
                    source_violations,
                ) => {
                    info!(target: "user-log", "Found existing sources for {} from {}", plan_ctx.id, source.url);
                    if args.check_source {
                        output_violations(&source_violations, &[], "", false, false)?;
                    }
                }
                DownloadStatus::MissingSource(plan_ctx) => {
                    info!(target: "user-log", "Plan {} has no 'pkg_source' attribute specified", plan_ctx.id);
                }
                DownloadStatus::InvalidArchive(plan_ctx, source, actual_shasum, archive_path) => {
                    error!(target: "user-log", "Downloaded source shasum for {} from {} does not match, expected '{}', found '{}'. You can inspect the downloaded file at {}", plan_ctx.id, source.url, source.shasum, actual_shasum, archive_path.as_ref().display());
                }
                DownloadStatus::NoSource => {
                    info!(target: "user-log", "Dependency {:?} cannot be downloaded", dep);
                }
            },
            Err(err) => return Err(eyre!(err)),
        }
    }

    Ok(())
}
