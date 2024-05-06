use std::{env, path::PathBuf};

use chrono_humanize::{Accuracy, HumanTime};
use clap::{arg, Args};
use color_eyre::eyre::{eyre, Context, Result};
use owo_colors::OwoColorize;
use tracing::{error, info};

use crate::{
    cli::output::OutputFormat,
    core::{
        AutoBuildConfig, AutoBuildContext, BuildOrder, ChangeDetectionMode, DependencyChangeCause,
        PackageDepGlob, PackageTarget, RepoChanges,
    },
};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Output format
    #[arg(value_enum, short = 'f', long, default_value_t = OutputFormat::Plain)]
    format: OutputFormat,
    /// Build ordering to use with respect to the build's studio
    #[arg(value_enum, short = 'b', long, default_value_t = BuildOrder::Strict)]
    build_order: BuildOrder,
    /// Method to use to detect changes to packages
    #[arg(value_enum, short = 'm', long, default_value_t = ChangeDetectionMode::Disk)]
    change_detection_mode: ChangeDetectionMode,
    /// Display reasons for changes
    #[arg(short = 'e', long, default_value_t = false)]
    explain: bool,
    /// List of packages to check for changes
    packages: Option<Vec<PackageDepGlob>>,
}

pub(crate) fn execute(args: Params) -> Result<()> {
    let config_path = args.config_path.unwrap_or(
        env::current_dir()
            .context("Failed to determine current working directory")?
            .join("hab-auto-build.json"),
    );
    let config = AutoBuildConfig::new(&config_path)?;

    let run_context = AutoBuildContext::new(&config, &config_path, args.change_detection_mode)
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
    let changes = run_context.changes(
        &package_indices,
        args.change_detection_mode,
        args.build_order,
        PackageTarget::default(),
    );

    match args.format {
        OutputFormat::Plain => output_plain(changes, args.explain)?,
        OutputFormat::Json => todo!(),
    }
    Ok(())
}

fn output_plain(repo_statuses: Vec<RepoChanges<'_>>, explain: bool) -> Result<()> {
    for repo_status in repo_statuses {
        if repo_status.changes.is_empty() {
            info!(target: "user-ui",
                "{} No changes detected in repo",
                format!("{}:", repo_status.repo.id).cyan().bold(),
            );
        } else {
            info!(target: "user-ui",
                "{} {} changes detected in repo",
                format!("{}:", repo_status.repo.id).cyan().bold(),
                repo_status.changes.len().magenta(),
            );
            for change in repo_status.changes {
                info!(target: "user-ui",
                    "  {} {}",
                    format!("{}:", change.plan_ctx.id.as_ref())
                        .green()
                        .bold(),
                    change.plan_ctx.plan_path.as_ref().display()
                );
                if explain {
                    if let Some(latest_artifact) = change.plan_ctx.latest_artifact.as_ref() {
                        info!( target: "user-ui",
                            "    Latest artifact {} was built {} at {}",
                            latest_artifact.ident.magenta(),
                            HumanTime::from(latest_artifact.created_at)
                                .to_text_en(Accuracy::Rough, chrono_humanize::Tense::Past),
                            latest_artifact.created_at.blue(),
                        );
                    }
                    for cause in change.causes {
                        match cause {
                            DependencyChangeCause::DependencyStudioNeedRebuild { plan } => {
                                info!(target: "user-ui", "    Plan's studio {} has been modified", plan.magenta());
                            }
                            DependencyChangeCause::PlanContextChanged {
                                latest_plan_artifact,
                                files_changed_on_disk,
                                files_changed_on_git,
                            } => {
                                if !files_changed_on_disk.is_empty() {
                                    info!(target: "user-ui", "    Plan files modified on disk since last artifact was built");
                                    for file in files_changed_on_disk {
                                        info!(target: "user-ui",
                                            "      - [{}] {} {}",
                                            file.last_modified_at.blue(),
                                            file.path.as_ref().display(),
                                            format!(
                                                "({} later)",
                                                HumanTime::from(
                                                    file.last_modified_at.signed_duration_since(
                                                        latest_plan_artifact.created_at
                                                    )
                                                )
                                                .to_text_en(
                                                    Accuracy::Rough,
                                                    chrono_humanize::Tense::Present
                                                )
                                            )
                                            .italic()
                                        );
                                    }
                                }
                                if !files_changed_on_git.is_empty() {
                                    info!(target: "user-ui", "    Plan files modified on git since last artifact was built");
                                    for file in files_changed_on_git {
                                        info!(target: "user-ui",
                                            "      - [{}] {} {}",
                                            file.last_modified_at.blue(),
                                            file.path.as_ref().display(),
                                            format!(
                                                "({} later)",
                                                HumanTime::from(
                                                    file.last_modified_at.signed_duration_since(
                                                        latest_plan_artifact.created_at
                                                    )
                                                )
                                                .to_text_en(
                                                    Accuracy::Rough,
                                                    chrono_humanize::Tense::Present
                                                )
                                            )
                                            .italic()
                                        );
                                    }
                                }
                            }
                            DependencyChangeCause::DependencyArtifactsUpdated {
                                latest_plan_artifact,
                                updated_dep_artifacts,
                            } => {
                                info!(target: "user-ui",
                                    "    Plan dependencies re-built since the last time this plan was built:"
                                );
                                for updated_dep_artifact in updated_dep_artifacts {
                                    info!(target: "user-ui",
                                        "      - [{}] {} {}",
                                        updated_dep_artifact.created_at.blue(),
                                        updated_dep_artifact.ident,
                                        format!(
                                            "({} later)",
                                            HumanTime::from(
                                                updated_dep_artifact
                                                    .created_at
                                                    .signed_duration_since(
                                                        latest_plan_artifact.created_at
                                                    )
                                            )
                                            .to_text_en(
                                                Accuracy::Rough,
                                                chrono_humanize::Tense::Present
                                            )
                                        )
                                        .italic()
                                    );
                                }
                            }
                            DependencyChangeCause::NoBuiltArtifact => {
                                info!(target: "user-ui", "    Plan not built yet")
                            }
                            DependencyChangeCause::DependencyPlansNeedRebuild { plans } => {
                                info!(target: "user-ui",
                                    "    Plan dependencies that will be re-built due to changes:"
                                );
                                for (plan_dep_type, plan_ctx_id, plan_path) in plans {
                                    info!(target: "user-ui",
                                        "      - [{}] {}: {}",
                                        plan_dep_type.cyan(),
                                        plan_ctx_id,
                                        plan_path.as_ref().display()

                                    );
                                }
                            }
                        }
                    }
                    println!()
                }
            }
        }
    }

    Ok(())
}
