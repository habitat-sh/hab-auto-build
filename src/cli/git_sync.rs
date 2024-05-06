use std::{env, path::PathBuf};

use clap::Args;
use owo_colors::OwoColorize;
use tracing::{error, info};

use crate::core::{
    AutoBuildConfig, AutoBuildContext, ChangeDetectionMode, PackageDepGlob, PackageTarget,
    PlanContextPathGitSyncStatus,
};
use color_eyre::eyre::{eyre, Context, Result};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Do a dry run of the sync and output the potential changes
    #[arg(short = 'd', long)]
    dry_run: bool,
    /// List of packages to add to the change list
    packages: Option<Vec<PackageDepGlob>>,
}

pub(crate) fn execute(args: Params) -> Result<()> {
    let config_path = args.config_path.unwrap_or(
        env::current_dir()
            .context("Failed to determine current working directory")?
            .join("hab-auto-build.json"),
    );
    let config = AutoBuildConfig::new(&config_path)?;

    let mut run_context = AutoBuildContext::new(&config, &config_path, ChangeDetectionMode::Disk)
        .with_context(|| eyre!("Failed to initialize run"))?;

    let packages = &args
        .packages
        .clone()
        .unwrap_or(vec![PackageDepGlob::parse("*/*").unwrap()]);
    let package_indices = run_context.glob_deps(packages, PackageTarget::default())?;
    if package_indices.is_empty() && !run_context.is_empty() {
        error!(target: "user-log",
            "No packages found matching patterns: {}",
            serde_json::to_string(&args.packages).unwrap()
        );
        return Ok(());
    }

    run_context.get_connection()?.exclusive_transaction(|connection| {
        match run_context.sync_plans_with_git(connection, &package_indices, args.dry_run) {
            Ok(repo_statuses) => {
                for (repo_id, plan_statuses ) in repo_statuses {
                    info!(target: "user-ui", "{}:", repo_id.to_string().blue());
                for plan_status in plan_statuses {
                    if !plan_status.file_statuses.is_empty() {
                        info!(target: "user-ui", "  {}:", plan_status.id.to_string().blue());
                        for file_status in plan_status.file_statuses {
                            match file_status {
                                PlanContextPathGitSyncStatus::Synced(path, disk_modified_at, git_modified_at) => {
                                    info!(target: "user-ui", "    {}: synced from {} to {}", path.display().white(), disk_modified_at.green(), git_modified_at.green());
                                },
                                PlanContextPathGitSyncStatus::LocallyModified(path, disk_modified_at) => {
                                    info!(target: "user-ui", "    {}: local modification at {}", path.display().white(), disk_modified_at.yellow());
                                },
                            }
                        }
                    }
                }
            }
            }
            Err(err) => return Err(eyre!(err)),
        }
        Ok(())
    })
}
