use std::{env, path::PathBuf};

use clap::Args;
use tracing::{error, info, warn};

use crate::core::{
    AutoBuildConfig, AutoBuildContext, DependencyChangeCause, PackageDepIdent, RemoveError,
    RemoveStatus,
};
use color_eyre::{
    eyre::{eyre, Context, Result},
    owo_colors::OwoColorize,
};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// List of packages to remove from the change list
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
        match run_context.remove_plans_from_changes(connection, package) {
            Ok(statuses) => {
                for status in statuses {
                    match status {
                        RemoveStatus::Removed(plan_ctx_id) => {
                            info!(target: "user-log", "Plan {} removed from change list", plan_ctx_id);
                        }
                        RemoveStatus::AlreadyRemoved(plan_ctx_id) => {
                            info!(target: "user-log", "Plan {} already removed from change list", plan_ctx_id);
                        }
                        RemoveStatus::CannotRemove(plan_ctx_id, causes) => {
                            error!(target: "user-log", "Plan {} cannot be removed from change list due to causes other than a change of the plan's files", plan_ctx_id);
                            error!(target: "user-log", "You can see the full explanation of changes using `hab-auto-build changes --explain {}`", PackageDepIdent::from(plan_ctx_id.as_ref()));
                        }
                    }
                }
            }
            Err(RemoveError::PlansNotFound(_)) => {
                error!(target: "user-log", "No plans found for {} in any repo", package);
            }
            Err(err) => return Err(eyre!(err)),
        }
    }

    Ok(())
})
}
