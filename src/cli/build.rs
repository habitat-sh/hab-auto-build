use chrono::Duration;
use chrono_humanize::{Accuracy, HumanTime, Tense};
use clap::{Args, ValueEnum};
use color_eyre::eyre::{eyre, Context, Result};
use owo_colors::OwoColorize;
use std::{env, path::PathBuf};
use tracing::{error, info};

use crate::{
    check::ViolationLevel,
    cli::{
        check::{self, output_violations},
        output::OutputFormat,
    },
    core::{
        habitat::BuildError, AutoBuildConfig, AutoBuildContext, BuildOrder, BuildPlan, BuildStep,
        BuildStepError, ChangeDetectionMode, Dependency, DownloadStatus, PackageDepGlob,
        PackageTarget, PlanCheckStatus,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CheckLevel {
    AllowAll,
    AllowWarnings,
    Strict,
}

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Output format
    #[arg(value_enum, short = 'f', long, default_value_t = OutputFormat::Plain, requires = "dry_run")]
    format: OutputFormat,
    /// Do a dry run of the build, does not actually build anything
    #[arg(short = 'd', long)]
    dry_run: bool,
    /// Build ordering to use with respect to the build's studio
    #[arg(value_enum, short = 'b', long, default_value_t = BuildOrder::Strict)]
    build_order: BuildOrder,
    /// Method to use to detect changes to packages
    #[arg(value_enum, short = 'm', long, default_value_t = ChangeDetectionMode::Disk)]
    change_detection_mode: ChangeDetectionMode,
    /// Allow use of packages from a remote habitat builder instance specified by HAB_BLDR_URL
    #[arg(short = 'r', long)]
    allow_remote: bool,
    /// Level of checks to perform
    #[arg(value_enum, short = 'l', long, default_value_t = CheckLevel::Strict)]
    check_level: CheckLevel,
    /// List of packages to build
    packages: Vec<PackageDepGlob>,
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

    let package_indices = run_context.glob_deps(&args.packages, PackageTarget::default())?;
    if package_indices.is_empty() && !run_context.is_empty() && !args.packages.is_empty() {
        error!(target: "user-log",
            "No packages found matching patterns: {}",
            serde_json::to_string(&args.packages).unwrap()
        );
        return Ok(());
    }
    let build_plan = run_context.build_plan_generate(
        package_indices,
        args.change_detection_mode,
        args.build_order,
        PackageTarget::default(),
        args.allow_remote,
    )?;
    if args.dry_run {
        match args.format {
            OutputFormat::Plain => output_plain(build_plan)?,
            OutputFormat::Json => output_json(build_plan)?,
        }
    } else {
        let mut all_checks_passed = true;
        for step in build_plan.check_steps {
            let mut step_check_passed = true;
            match step.dependency {
                Dependency::ResolvedDep(resolved_dep) => {
                    info!(target: "user-ui", "{} [remote] {}", "     Checking".green().bold(), resolved_dep);
                }
                Dependency::RemoteDep(remote_dep) => {
                    info!(target: "user-ui", "{} [remote] {}", "     Checking".green().bold(), remote_dep);
                }
                Dependency::LocalPlan(plan_ctx) => {
                    info!(target: "user-ui", "{} [plan] {}", "     Checking".green().bold(), plan_ctx.id);
                }
            }
            match run_context.package_check(step.index) {
                Ok(check_status) => match check_status {
                    PlanCheckStatus::CheckSucceeded(
                        plan_config_path,
                        source_violations,
                        artifact_violations,
                    ) => {
                        check::output_violations(
                            plan_config_path,
                            &source_violations,
                            &artifact_violations,
                            "",
                            false,
                            false,
                        )?;
                        let source_warnings = source_violations
                            .iter()
                            .filter(|v| v.level == ViolationLevel::Warn)
                            .count();
                        let source_errors = source_violations
                            .iter()
                            .filter(|v| v.level == ViolationLevel::Error)
                            .count();
                        let artifact_warnings = artifact_violations
                            .iter()
                            .filter(|v| v.level == ViolationLevel::Warn)
                            .count();
                        let artifact_errors = artifact_violations
                            .iter()
                            .filter(|v| v.level == ViolationLevel::Error)
                            .count();
                        match args.check_level {
                            CheckLevel::AllowWarnings if source_errors + artifact_errors > 0 => {
                                all_checks_passed = false;
                                step_check_passed = false;
                            }
                            CheckLevel::Strict
                                if source_errors
                                    + source_warnings
                                    + artifact_errors
                                    + artifact_warnings
                                    > 0 =>
                            {
                                all_checks_passed = false;
                                step_check_passed = false;
                            }
                            _ => {}
                        };
                        if !step_check_passed {
                            match step.dependency {
                                Dependency::ResolvedDep(resolved_dep) => {
                                    info!(target: "user-ui", "{} [remote] {}", "Check Failure".red().bold(), resolved_dep);
                                }
                                Dependency::RemoteDep(remote_dep) => {
                                    info!(target: "user-ui", "{} [remote] {}", "Check Failure".red().bold(), remote_dep);
                                }
                                Dependency::LocalPlan(plan_ctx) => {
                                    info!(target: "user-ui", "{} [plan] {}", "Check Failure".red().bold(), plan_ctx.id);
                                }
                            }
                        } else {
                            match step.dependency {
                                Dependency::ResolvedDep(resolved_dep) => {
                                    info!(target: "user-ui", "{} [remote] {}", "Check Success".green().bold(), resolved_dep);
                                }
                                Dependency::RemoteDep(remote_dep) => {
                                    info!(target: "user-ui", "{} [remote] {}", "Check Success".green().bold(), remote_dep);
                                }
                                Dependency::LocalPlan(plan_ctx) => {
                                    info!(target: "user-ui", "{} [plan] {}", "Check Success".green().bold(), plan_ctx.id);
                                }
                            }
                        }
                    }
                    PlanCheckStatus::ArtifactNotFound => {
                        info!(target: "user-ui", "{}: No artifact found for {:?}", "error".bold().red(), step.dependency);
                        return Ok(());
                    }
                },
                Err(err) => {
                    info!(target: "user-ui", "{}: Failed to check package {:?}: {:#?}", "error".bold().red(), step.dependency, err);
                    return Ok(());
                }
            };
        }
        if !all_checks_passed {
            info!(target: "user-ui", "{}: Found issues with dependency packages, you should fix them before building more packages", "error".bold().red());
            return Ok(());
        }
        for step in build_plan.build_steps {
            info!(target: "user-ui", "{} [{}] {}", "     Building".green().bold(), step.studio, step.plan_ctx.id);
            match run_context.download_plan_source(step.plan_ctx, true)? {
                DownloadStatus::Downloaded(_source_ctx, _, _, _, source_violations)
                | DownloadStatus::AlreadyDownloaded(_source_ctx, _, _, source_violations) => {
                    let source_warnings = source_violations
                        .iter()
                        .filter(|v| v.level == ViolationLevel::Warn)
                        .count();
                    let source_errors = source_violations
                        .iter()
                        .filter(|v| v.level == ViolationLevel::Error)
                        .count();
                    match args.check_level {
                        CheckLevel::AllowWarnings if source_errors > 0 => all_checks_passed = false,
                        CheckLevel::Strict if source_errors + source_warnings > 0 => {
                            all_checks_passed = false
                        }
                        _ => {}
                    };
                    output_violations(
                        if !all_checks_passed {
                            Some(step.plan_ctx.plan_path.plan_config_path())
                        } else {
                            None
                        },
                        &source_violations,
                        &[],
                        &step.plan_ctx.id.to_string(),
                        false,
                        false,
                    )?;
                    if !all_checks_passed {
                        info!(target: "user-ui", "{} [{}] {}", "Build Failure".red().bold(), step.studio, step.plan_ctx.id);
                        info!(target: "user-ui", "{}: Found issues with the package {}, you should fix the plan at {} before re-attempting the build.", "error".bold().red(), step.plan_ctx.id.yellow(), step.plan_ctx.plan_path.as_ref().display().blue());
                        return Ok(());
                    }
                }
                DownloadStatus::MissingSource(_) => {}
                DownloadStatus::NoSource => {
                    unreachable!()
                }
                DownloadStatus::InvalidArchive(_, source, actual_shasum, _) => {
                    return Err(eyre!(
                        "Failed to download package source, package shasum mismatch. Expected shasum {}, found shasum {}", source.shasum, actual_shasum
                    ));
                }
            }
            match run_context.build_step_execute(&step) {
                Ok(build_result) => {
                    output_violations(
                        Some(step.plan_ctx.plan_path.plan_config_path()),
                        &[],
                        &build_result.artifact_violations,
                        &step.plan_ctx.id.to_string(),
                        false,
                        false,
                    )?;

                    let artifact_warnings = build_result
                        .artifact_violations
                        .iter()
                        .filter(|v| v.level == ViolationLevel::Warn)
                        .count();
                    let artifact_errors = build_result
                        .artifact_violations
                        .iter()
                        .filter(|v| v.level == ViolationLevel::Error)
                        .count();
                    match args.check_level {
                        CheckLevel::AllowWarnings if artifact_errors > 0 => {
                            all_checks_passed = false
                        }
                        CheckLevel::Strict if artifact_errors + artifact_warnings > 0 => {
                            all_checks_passed = false
                        }
                        _ => {}
                    };

                    if !all_checks_passed {
                        info!(target: "user-ui", "{} [{}] {}", "Build Failure".red().bold(), step.studio, build_result.artifact_ident.artifact_name());
                        info!(target: "user-ui", "{}: Found issues with the package {}, you should fix the plan at {} before re-attempting the build. You can find the build log at {}", "error".bold().red(), step.plan_ctx.id.yellow(), step.plan_ctx.plan_path.as_ref().display().blue(), build_result.build_log.display().blue());
                        return Ok(());
                    } else {
                        info!(target: "user-ui", "{} [{}] {}", "Build Success".green().bold(), step.studio, build_result.artifact_ident.artifact_name());
                    }
                }
                Err(BuildStepError::Build(
                    BuildError::Native(_, build_log)
                    | BuildError::Bootstrap(_, build_log)
                    | BuildError::Standard(_, build_log),
                )) => {
                    info!(target: "user-ui", "{} [{}] {}", "Build Failure".red().bold(), step.studio, step.plan_ctx.id);
                    info!(target: "user-ui", "{}: Failed to complete build of package {}, you should fix the plan at {} before re-attempting the build. You can find the build log at {}", "error".bold().red(), step.plan_ctx.id.yellow(), step.plan_ctx.plan_path.as_ref().display().blue(), build_log.display().blue());
                    return Ok(());
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
    Ok(())
}

fn output_plain(build_plan: BuildPlan) -> Result<()> {
    if build_plan.build_steps.is_empty() {
        info!(target: "user-log", "{}", "All plans built");
        return Ok(());
    }
    if !build_plan.check_steps.is_empty() {
        info!(target: "user-ui", "{}", "Dependencies to Check:");
        for (index, step) in build_plan.check_steps.iter().enumerate() {
            match step.dependency {
                Dependency::ResolvedDep(resolved_dep) => {
                    info!(target: "user-ui", "{:>4} - [remote] {}", index + 1, resolved_dep);
                }
                Dependency::RemoteDep(remote_dep) => {
                    info!(target: "user-ui", "{:>4} - [remote] {}", index + 1, remote_dep);
                }
                Dependency::LocalPlan(plan_ctx) => {
                    info!(target: "user-ui", "{:>4} - [plan] {}", index + 1, plan_ctx.id);
                }
            }
        }
    }
    info!(target: "user-ui", "{}", "Plan Build Order:");
    let mut all_durations_known = true;
    let mut total_duration = Duration::seconds(0);
    for (index, step) in build_plan.build_steps.iter().enumerate() {
        let BuildStep {
            plan_ctx,
            studio,
            remote_deps,
            causes,
            build_duration,
            ..
        } = step;
        if let Some(build_duration) = build_duration {
            total_duration = total_duration.checked_add(build_duration).unwrap();
        } else {
            all_durations_known = false;
        };

        if !remote_deps.is_empty() {
            info!(target: "user-ui",
                "{:>4} - [{}] {} {} {}",
                (index + 1).to_string(),
                studio,
                plan_ctx.id,
                causes
                    .iter()
                    .map(|cause| {
                        cause.to_emoji()
                    })
                    .collect::<Vec<_>>().join(""),
                format!("[{} remote deps]", remote_deps.len()).yellow()
            );
            for (index, remote_dep) in remote_deps.iter().enumerate() {
                match remote_dep {
                    Dependency::ResolvedDep(resolved_dep) => {
                        info!(target: "user-ui",
                            "       {}",
                            format!("{} - {}", index, resolved_dep).yellow()
                        );
                    }
                    Dependency::RemoteDep(remote_dep) => {
                        info!(target: "user-ui",
                            "       {}",
                            format!("{} - {}", index, remote_dep).yellow()
                        );
                    }
                    Dependency::LocalPlan(_) => {}
                }
            }
        } else {
            info!(target: "user-ui",
                "{:>4} - [{}] {} {}",
                (index + 1).to_string(),
                studio,
                plan_ctx.id,
                causes
                .iter()
                .map(|cause| {
                    cause.to_emoji()
                })
                .collect::<Vec<_>>().join("")
            );
        }
    }
    if all_durations_known {
        info!(target: "user-ui", "Estimated build time: {}", HumanTime::from(total_duration).to_text_en(Accuracy::Rough, Tense::Present));
    } else {
        info!(target: "user-ui", "Minimum estimated build time: {}", HumanTime::from(total_duration).to_text_en(Accuracy::Rough, Tense::Present));
    }
    Ok(())
}

fn output_json(_dry_run: BuildPlan) -> Result<()> {
    todo!()
}
