use std::{env, path::PathBuf};

use clap::Args;
use tracing::{error, info};

use crate::core::{
    AddStatus, AutoBuildConfig, AutoBuildContext, PackageDepGlob,
    PackageTarget,
};
use color_eyre::eyre::{eyre, Context, Result};


#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// List of packages to add to the change list
    packages: Vec<PackageDepGlob>,
}

pub(crate) fn execute(args: Params) -> Result<()> {
    let config_path = args.config_path.unwrap_or(
        env::current_dir()
            .context("Failed to determine current working directory")?
            .join("hab-auto-build.json"),
    );
    let config = AutoBuildConfig::new(&config_path)?;

    let mut run_context = AutoBuildContext::new(&config, &config_path)
        .with_context(|| eyre!("Failed to initialize run"))?;

    let package_indices = run_context.glob_deps(&args.packages, PackageTarget::default())?;
    if package_indices.is_empty() && !run_context.is_empty() && !args.packages.is_empty() {
        error!(target: "user-log",
            "No packages found matching patterns: {}",
            serde_json::to_string(&args.packages).unwrap()
        );
        return Ok(());
    }

    run_context.get_connection()?.exclusive_transaction(|connection| {
        match run_context.add_plans_to_changes(connection, &package_indices) {
            Ok(statuses) => {
                for status in statuses {
                    match status {
                        AddStatus::Added(plan_ctx_id) => {
                            info!(target: "user-log", "Plan {} added to change list", plan_ctx_id);
                        }
                        AddStatus::AlreadyAdded(plan_ctx_id) => {
                            info!(target: "user-log", "Plan {} is already in change list", plan_ctx_id);
                        }
                    }
                }
            }
            Err(err) => return Err(eyre!(err)),
        }
        Ok(())
    })
}
