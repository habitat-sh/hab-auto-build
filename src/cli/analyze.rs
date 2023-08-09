use color_eyre::eyre::{eyre, Context, Result};
use owo_colors::OwoColorize;
use serde_json::json;
use std::{collections::HashSet, env, path::PathBuf};
use tera::Tera;
use tracing::{error, info};

use clap::Args;

use crate::{
    cli::output::OutputFormat,
    core::{
        AnalysisType, AutoBuildConfig, AutoBuildContext, Dependency, DependencyAnalysis,
        PackageDepGlob, PackageTarget,
    },
};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Forces the plan's studio package to be considered as a build dependency for a plan
    #[arg(short = 's', long, default_value_t = false)]
    strict_build_order: bool,
    /// Output format
    #[arg(value_enum, short = 'f', long, default_value_t = OutputFormat::Plain)]
    format: OutputFormat,
    /// Detect runtime dependencies
    #[arg(long, default_value_t = false)]
    deps: bool,
    /// Detect build dependencies
    #[arg(long, default_value_t = false)]
    build_deps: bool,
    /// Detect transitive runtime dependencies
    #[arg(long, default_value_t = false)]
    tdeps: bool,
    /// Detect transitive build dependencies
    #[arg(long, default_value_t = false)]
    build_tdeps: bool,
    /// Detect studio dependency
    #[arg(long, default_value_t = false)]
    studio_dep: bool,
    /// Detect reverse runtime dependencies
    #[arg(long, default_value_t = false)]
    rdeps: bool,
    /// Detect reverse build dependencies
    #[arg(long, default_value_t = false)]
    build_rdeps: bool,
    #[arg(long)]
    template: Option<String>,
    /// List of packages to include
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

    let mut analysis_types = HashSet::new();
    if args.studio_dep {
        analysis_types.insert(AnalysisType::StudioDependency);
    }
    if args.deps {
        analysis_types.insert(AnalysisType::Dependencies);
    }
    if args.build_deps {
        analysis_types.insert(AnalysisType::BuildDependencies);
    }
    if args.tdeps {
        analysis_types.insert(AnalysisType::TransitiveDependencies);
    }
    if args.build_tdeps {
        analysis_types.insert(AnalysisType::TransitiveBuildDependencies);
    }
    if args.rdeps {
        analysis_types.insert(AnalysisType::ReverseDependencies);
    }
    if args.build_rdeps {
        analysis_types.insert(AnalysisType::ReverseBuildDependencies);
    }

    let package_indices = run_context.glob_deps(&args.packages, PackageTarget::default())?;
    if package_indices.is_empty() && !run_context.is_empty() && !args.packages.is_empty() {
        error!(target: "user-log",
            "No packages found matching patterns: {}",
            serde_json::to_string(&args.packages).unwrap()
        );
        return Ok(());
    }
    let plan_analysis_list = package_indices
        .into_iter()
        .map(|package_index| run_context.dep_analysis(package_index, &analysis_types))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .collect::<Vec<_>>();

    match args.format {
        OutputFormat::Plain => output_plain(plan_analysis_list)?,
        OutputFormat::Json => output_json(plan_analysis_list, args.template)?,
    }

    Ok(())
}

fn output_plain(dep_analysis_list: Vec<DependencyAnalysis>) -> Result<()> {
    for dep_analysis in dep_analysis_list {
        if let (Some(plan_ctx)) = dep_analysis.plan_ctx {
            info!(
                target: "user-ui",
                "{}\n{}\n",
                "Package:".white().bold(),
                plan_ctx.id.as_ref()
            );
            info!(
                target: "user-ui",
                "{}\n{}\n",
                "Repo:".white().bold(),
                plan_ctx.repo.path.as_ref().display()
            );
            info!(
                target: "user-ui",
                "{}\n{}\n",
                "Plan:".white().bold(),
                plan_ctx.plan_path.as_ref().display()
            );
            if let Some(dep) = dep_analysis.studio_dep.as_ref() {
                if let Some(dep) = dep {
                    info!(target: "user-ui", "{}\n{:?}\n", "Studio:".white().bold(), dep);
                } else {
                    info!(target: "user-ui", "{}\nNATIVE\n", "Studio:".white().bold());
                }
            }
        } else {
            match dep_analysis.dep_ctx {
                Dependency::ResolvedDep(dep_ident) => {
                    info!(target: "user-ui", "{}\n{}\n", "Resolved Dependency:".white().bold(), dep_ident);
                }
                Dependency::RemoteDep(dep_ident) => {
                    info!(target: "user-ui", "{}\n{}\n", "Remote Dependency:".white().bold(), dep_ident);
                }
                Dependency::LocalPlan(_) => {}
            }
        }

        for (analysis_type, deps) in [
            (AnalysisType::Dependencies, &dep_analysis.deps),
            (AnalysisType::BuildDependencies, &dep_analysis.build_deps),
            (AnalysisType::TransitiveDependencies, &dep_analysis.tdeps),
            (
                AnalysisType::TransitiveBuildDependencies,
                &dep_analysis.build_tdeps,
            ),
            (AnalysisType::ReverseDependencies, &dep_analysis.rdeps),
            (
                AnalysisType::ReverseBuildDependencies,
                &dep_analysis.build_rdeps,
            ),
        ] {
            if let Some(deps) = deps.as_ref() {
                info!(target: "user-ui", "{}", format!("{}:",analysis_type).white().bold());
                if !deps.is_empty() {
                    for dep in deps {
                        info!(target: "user-ui", "{:?}", dep);
                    }
                    info!(target: "user-ui", "");
                } else {
                    info!(target: "user-ui", "NO DEPENDENCIES\n");
                }
            }
        }
    }
    Ok(())
}

fn output_json(
    plan_analysis_list: Vec<DependencyAnalysis>,
    template: Option<String>,
) -> Result<()> {
    if let Some(template) = template {
        let context = tera::Context::from_serialize(json!({ "data": plan_analysis_list }))?;
        let template = snailquote::unescape(format!("\"{}\"", template).as_str())?;
        let result = Tera::one_off(&template, &context, false)?;
        info!(target: "user-ui", "{}", result);
    } else {
        info!(
            target: "user-ui",
            "{}",
            serde_json::to_string_pretty(&plan_analysis_list)
                .context("Failed to serialize plan analysis into JSON")?
        );
    }
    Ok(())
}

fn output_pretty(_deps: Vec<&Dependency>) {
    todo!()
}
