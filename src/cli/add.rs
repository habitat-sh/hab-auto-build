use std::{env, path::PathBuf};

use clap::Args;
use tracing::{error, info};

use crate::core::{AddError, AddStatus, AutoBuildConfig, AutoBuildContext, PackageDepIdent};
use color_eyre::{
    eyre::{eyre, Context, Result},
    owo_colors::OwoColorize,
};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// List of packages to add to the change list
    packages: Vec<PackageDepIdent>,
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

    run_context.get_connection()?.exclusive_transaction(|connection| {
        for package in args.packages.iter() {
            match run_context.add_plans_to_changes(connection, package) {
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
                Err(AddError::PlansNotFound(_)) => {
                    error!(target: "user-log", "No plans found for {} in any repo", package);
                }
                Err(err) => return Err(eyre!(err)),
            }
        }
    
        Ok(())
    })
}
