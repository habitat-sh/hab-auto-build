use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    fs::File,
    path::{Path, PathBuf},
    sync::mpsc::channel,
    time::Instant,
};

use chrono::{DateTime, Duration, Utc};
use color_eyre::{
    eyre::{eyre, Context, Result},
    Help,
};
use diesel::{
    r2d2::{ConnectionManager, PooledConnection},
    Connection, SqliteConnection,
};
use globset::{Glob, GlobBuilder};
use ignore::WalkBuilder;
use lazy_static::lazy_static;
use petgraph::{algo, stable_graph::NodeIndex};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, trace};

use crate::{
    check::{
        ArtifactCheck, Checker, CheckerContext, ContextRules, LeveledArtifactCheckViolation,
        LeveledSourceCheckViolation,
    },
    core::{
        ArtifactCache, ArtifactCachePath, Dependency, DependencyDepth, DependencyDirection,
        DependencyType, PackageSourceDownloadError, SourceContext,
    },
    store::{self, Store},
};

use super::{
    DepGraph, DependencyChangeCause, PackageDepGlob, PackageDepIdent, PackageSha256Sum,
    PackageSource, PackageTarget, PlanContext, PlanContextID, PlanScannerBuilder, RepoConfig,
    RepoContext, RepoContextID, PackageResolvedDepIdent,
};

lazy_static! {
    pub static ref STANDARD_BUILD_STUDIO_PACKAGE: PackageDepIdent =
        PackageDepIdent::parse("core/hab-studio").unwrap();
    pub static ref BOOTSTRAP_BUILD_STUDIO_PACKAGE: PackageDepIdent =
        PackageDepIdent::parse("core/build-tools-hab-studio").unwrap();
    pub static ref DEFAULT_STORE_PATH: PathBuf = PathBuf::from(".hab-auto-build");
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BuildStudioConfig {
    pub standard: PackageDepIdent,
    pub bootstrap: PackageDepIdent,
}

impl Default for BuildStudioConfig {
    fn default() -> Self {
        BuildStudioConfig {
            standard: STANDARD_BUILD_STUDIO_PACKAGE.clone(),
            bootstrap: BOOTSTRAP_BUILD_STUDIO_PACKAGE.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AutoBuildConfig {
    #[serde(default)]
    pub studios: BuildStudioConfig,
    pub store: Option<PathBuf>,
    pub repos: Vec<RepoConfig>,
}

impl AutoBuildConfig {
    pub fn new(config_path: impl AsRef<Path>) -> Result<AutoBuildConfig> {
        let config_path = config_path
            .as_ref()
            .canonicalize()
            .context("Failed to canonicalize path to configuration file")
            .with_suggestion(|| {
                format!(
                    "Make sure '{}' is a valid hab-auto-build json configuration",
                    config_path.as_ref().display()
                )
            })?;
        trace!("Reading configuration file '{}'", config_path.display());
        let config_file = File::open(&config_path).with_context(|| {
            eyre!(
                "Failed to find hab-auto-build configuration at '{}'",
                config_path.display()
            )
        })?;
        let config = serde_json::from_reader(config_file)
            .with_context(|| {
                eyre!(
                    "Failed to read configuration file '{}'",
                    config_path.display()
                )
            })
            .with_suggestion(|| {
                format!(
                    "Make sure '{}' is a valid hab-auto-build json configuration",
                    config_path.display()
                )
            })?;
        debug!("Configuration file '{}' loaded", config_path.display());
        Ok(config)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct AutoBuildContextPath(PathBuf);

impl From<PathBuf> for AutoBuildContextPath {
    fn from(value: PathBuf) -> Self {
        AutoBuildContextPath(value)
    }
}

impl AsRef<Path> for AutoBuildContextPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum AnalysisType {
    StudioDependency,
    Dependencies,
    BuildDependencies,
    TransitiveDependencies,
    TransitiveBuildDependencies,
    ReverseDependencies,
    ReverseBuildDependencies,
}
impl Display for AnalysisType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisType::StudioDependency => write!(f, "Studio Dependency"),
            AnalysisType::Dependencies => write!(f, "Dependencies"),
            AnalysisType::BuildDependencies => write!(f, "Build Dependencies"),
            AnalysisType::TransitiveDependencies => write!(f, "Transitive Dependencies"),
            AnalysisType::TransitiveBuildDependencies => write!(f, "Transitive Build Dependencies"),
            AnalysisType::ReverseDependencies => write!(f, "Reverse Dependencies"),
            AnalysisType::ReverseBuildDependencies => write!(f, "Reverse Build Dependencies"),
        }
    }
}

pub(crate) struct AutoBuildContext {
    path: AutoBuildContextPath,
    store: Store,
    repos: HashMap<RepoContextID, RepoContext>,
    dep_graph: DepGraph,
    artifact_cache: ArtifactCache,
}

pub(crate) struct DependencyChange<'a> {
    pub plan_ctx: &'a PlanContext,
    pub causes: Vec<DependencyChangeCause>,
}

pub(crate) struct BuildDryRun<'a> {
    pub order: Vec<(
        &'a Dependency,
        Option<&'a Dependency>,
        Vec<&'a Dependency>,
        Vec<DependencyChangeCause>,
    )>,
}

pub(crate) enum AddStatus {
    Added(PlanContextID),
    AlreadyAdded(PlanContextID),
}

#[derive(Debug, Error)]
pub(crate) enum AddError {
    #[error("No plans for package '{0}' found")]
    PlansNotFound(PackageDepIdent),
    #[error("Encountered an unexpected error while trying to add the package to the change list")]
    UnexpectedError(#[source] color_eyre::eyre::Error),
}

pub(crate) enum RemoveStatus {
    Removed(PlanContextID),
    AlreadyRemoved(PlanContextID),
    CannotRemove(PlanContextID, Vec<DependencyChangeCause>),
}

#[derive(Debug, Error)]
pub(crate) enum RemoveError {
    #[error("No plans for package '{0}' found")]
    PlansNotFound(PackageDepIdent),
    #[error(
        "Encountered an unexpected error while trying to remove the package from the change list"
    )]
    UnexpectedError(#[source] color_eyre::eyre::Error),
}

pub(crate) enum CheckStatus {
    CheckSucceeded(
        PackageResolvedDepIdent,
        Vec<LeveledSourceCheckViolation>,
        Vec<LeveledArtifactCheckViolation>,
    ),
    ArtifactNotFound(PackageResolvedDepIdent),
}

pub(crate) enum DownloadStatus {
    Downloaded(SourceContext, PlanContext, PackageSource, Duration),
    AlreadyDownloaded(SourceContext, PlanContext, PackageSource),
    NoSource(PlanContext),
    InvalidArchive(PlanContext, PackageSource, PackageSha256Sum),
}

#[derive(Debug, Error)]
pub(crate) enum DownloadError {
    #[error("No plans for package '{0}' found")]
    PlansNotFound(PackageDepIdent),
    #[error("Encountered an unexpected error while trying to download the package sources")]
    UnexpectedDownloadError(#[source] PackageSourceDownloadError),
    #[error("Encountered an unexpected io error while trying to download the package sources")]
    UnexpectedIOError(#[source] std::io::Error),
    #[error("Encountered an unexpected error while trying to download the package sources")]
    UnexpectedError(#[source] color_eyre::eyre::Error),
}

#[derive(Debug, Serialize)]
pub(crate) struct DependencyAnalysis<'a> {
    pub dep_ctx: &'a Dependency,
    pub repo_ctx: Option<&'a RepoContext>,
    pub plan_ctx: Option<&'a PlanContext>,
    pub studio_dep: Option<Option<&'a Dependency>>,
    pub deps: Option<Vec<&'a Dependency>>,
    pub build_deps: Option<Vec<&'a Dependency>>,
    pub tdeps: Option<Vec<&'a Dependency>>,
    pub build_tdeps: Option<Vec<&'a Dependency>>,
    pub rdeps: Option<Vec<&'a Dependency>>,
    pub build_rdeps: Option<Vec<&'a Dependency>>,
}

pub(crate) struct RepoChanges<'a> {
    pub repo: &'a RepoContext,
    pub changes: Vec<DependencyChange<'a>>,
}

impl AutoBuildContext {
    pub fn new(
        config: &AutoBuildConfig,
        config_path: impl AsRef<Path>,
    ) -> Result<AutoBuildContext> {
        let start = Instant::now();

        let mut repos = HashMap::new();
        let auto_build_ctx_path = AutoBuildContextPath::from(
            config_path
                .as_ref()
                .parent()
                .ok_or(eyre!(
                    "Failed to determine parent folder of hab-auto-build configuration file"
                ))?
                .to_path_buf(),
        );

        for repo_config in config.repos.iter() {
            let repo_ctx = RepoContext::new(repo_config, &auto_build_ctx_path)?;
            repos.insert(repo_ctx.id.clone(), repo_ctx);
        }

        let store_path = config.store.as_ref().unwrap_or(&DEFAULT_STORE_PATH);
        let store_path = if store_path.is_absolute() {
            store_path.clone()
        } else {
            auto_build_ctx_path.as_ref().join(store_path)
        };
        let store = Store::new(store_path)?;

        // Scan artifact cache
        let artifact_cache = ArtifactCache::new(ArtifactCachePath::default(), &store)?;

        let mut dir_walk_builder: Option<WalkBuilder> = None;
        for repo_ctx in repos.values() {
            if let Some(dir_walk_builder) = dir_walk_builder.as_mut() {
                dir_walk_builder.add(repo_ctx.path.as_ref());
            } else {
                let mut new_walk_builder = WalkBuilder::new(repo_ctx.path.as_ref());
                new_walk_builder.follow_links(false);
                new_walk_builder.threads(0);
                dir_walk_builder = Some(new_walk_builder);
            }
        }

        let dir_walker = if let Some(dir_walk_builder) = dir_walk_builder {
            dir_walk_builder.build_parallel()
        } else {
            return Err(
                eyre!("No plan repos were specified in the hab-auto-build configuration")
                    .with_suggestion(|| {
                        "You need to specify atleast one repo object under the 'repos' key"
                    }),
            );
        };
        let mut plans: HashMap<PlanContextID, PlanContext> = HashMap::new();
        let modification_index = store.get_connection()?.transaction(|connection| {
            store::files_alternate_modified_at_get_full_index(connection)
        })?;
        let (sender, receiver) = channel();
        let mut dir_visitor_builder =
            PlanScannerBuilder::new(&repos, &modification_index, &artifact_cache, sender);
        std::thread::scope(|scope| {
            let walk_handle = scope.spawn(move || dir_walker.visit(&mut dir_visitor_builder));
            while let Ok(plan_ctx) = receiver.recv() {
                match plans.get(&plan_ctx.id) {
                    Some(existing_plan_ctx) => {
                        return Err(eyre!(
                        "Found multiple plans for the package '{}' at '{}' and previously at '{}'",
                        plan_ctx.id,
                        plan_ctx.plan_path.as_ref().display(),
                        existing_plan_ctx.plan_path.as_ref().display()
                    ))
                    }
                    None => {
                        plans.insert(plan_ctx.id.clone(), plan_ctx);
                    }
                }
            }
            walk_handle
                .join()
                .expect("Failed to join plan scanning directory walker thread");
            Ok(())
        })?;

        info!(
            "Detected {} plans across {} repos in {}s",
            plans.len(),
            repos.len(),
            start.elapsed().as_secs_f32()
        );

        let dep_graph = DepGraph::new(&config.studios, &artifact_cache, plans)?;

        Ok(AutoBuildContext {
            path: auto_build_ctx_path,
            store,
            repos,
            dep_graph,
            artifact_cache,
        })
    }

    pub fn glob_deps(
        &self,
        globs: &[PackageDepGlob],
        target: PackageTarget,
    ) -> Result<Vec<PackageDepIdent>> {
        let mut results = Vec::new();
        for glob in globs {
            let glob = glob.matcher()?;
            results.extend(self.dep_graph.glob_deps(&glob, target));
        }
        Ok(results)
    }

    pub fn dep_analysis<'a>(
        &'a self,
        dep_ident: &PackageDepIdent,
        analysis_types: &HashSet<AnalysisType>,
    ) -> Result<Vec<DependencyAnalysis<'a>>> {
        let mut results = Vec::new();
        for dep_node_index in self.dep_graph.get_dep_nodes(dep_ident) {
            let dep = &self.dep_graph.build_graph[dep_node_index];
            let (repo_ctx, plan_ctx) = match dep {
                Dependency::ResolvedDep(_) => (None, None),
                Dependency::RemoteDep(_) => (None, None),
                Dependency::LocalPlan(plan_ctx) => {
                    let repo_ctx = self
                        .repos
                        .get(&plan_ctx.repo_id)
                        .expect("Plan must belong to a repo");
                    (Some(repo_ctx), Some(plan_ctx))
                }
            };

            results.push(DependencyAnalysis {
                dep_ctx: &self.dep_graph.build_graph[dep_node_index],
                repo_ctx,
                plan_ctx,
                deps: analysis_types
                    .get(&AnalysisType::Dependencies)
                    .map(|t| self.node_dep_analysis(dep_node_index, *t))
                    .transpose()?,
                build_deps: analysis_types
                    .get(&AnalysisType::BuildDependencies)
                    .map(|t| self.node_dep_analysis(dep_node_index, *t))
                    .transpose()?,
                tdeps: analysis_types
                    .get(&AnalysisType::TransitiveDependencies)
                    .map(|t| self.node_dep_analysis(dep_node_index, *t))
                    .transpose()?,
                build_tdeps: analysis_types
                    .get(&AnalysisType::TransitiveBuildDependencies)
                    .map(|t| self.node_dep_analysis(dep_node_index, *t))
                    .transpose()?,
                rdeps: analysis_types
                    .get(&AnalysisType::ReverseDependencies)
                    .map(|t| self.node_dep_analysis(dep_node_index, *t))
                    .transpose()?,
                build_rdeps: analysis_types
                    .get(&AnalysisType::ReverseBuildDependencies)
                    .map(|t| self.node_dep_analysis(dep_node_index, *t))
                    .transpose()?,
                studio_dep: analysis_types
                    .get(&AnalysisType::StudioDependency)
                    .map(|t| self.node_dep_analysis(dep_node_index, *t))
                    .transpose()?
                    .map(|mut d| d.pop()),
            });
        }

        Ok(results)
    }

    pub fn download_source_archive(
        &self,
        package: &PackageDepIdent,
    ) -> Result<Vec<DownloadStatus>, DownloadError> {
        let mut results = Vec::new();
        let plan_node_indices = self.dep_graph.get_plan_nodes(package);
        if plan_node_indices.is_empty() {
            return Err(DownloadError::PlansNotFound(package.to_owned()));
        }
        let tmp_dir = self
            .store
            .temp_dir("download")
            .map_err(DownloadError::UnexpectedError)?;
        for plan_ctx in self
            .dep_graph
            .deps(&plan_node_indices)
            .into_iter()
            .filter_map(|d| d.plan_ctx())
        {
            if let Some(source) = plan_ctx.source.as_ref() {
                let source_store_path = self.store.package_source_store_path(source);
                let source_archive_path = source_store_path.archive_data_path();
                if source_archive_path.as_ref().is_file() {
                    match source.verify_pkg_archive(source_archive_path.as_ref()) {
                        Ok(_) => {
                            let existing_source_ctx = self
                                .store
                                .get_connection()
                                .map_err(DownloadError::UnexpectedError)?
                                .transaction(|connection| {
                                    store::source_context_get(connection, &source.shasum)
                                })
                                .map_err(DownloadError::UnexpectedError)?;
                            let source_ctx = if let Some(existing_source_ctx) = existing_source_ctx
                            {
                                existing_source_ctx
                            } else {
                                let new_source_ctx =
                                    SourceContext::read_from_disk(source_archive_path)
                                        .map_err(DownloadError::UnexpectedError)?;
                                self.store
                                    .get_connection()
                                    .map_err(DownloadError::UnexpectedError)?
                                    .transaction(|connection| {
                                        store::source_context_put(
                                            connection,
                                            &source.shasum,
                                            &new_source_ctx,
                                        )
                                    })
                                    .map_err(DownloadError::UnexpectedError)?;
                                new_source_ctx
                            };
                            results.push(DownloadStatus::AlreadyDownloaded(
                                source_ctx,
                                plan_ctx.clone(),
                                source.clone(),
                            ));
                            continue;
                        }
                        Err(_) => todo!(),
                    }
                }
                let temp_file_path = tmp_dir.path().join("download.part");
                info!(
                    "Downloading sources for package {} from {} to {}",
                    plan_ctx.id,
                    source.url,
                    temp_file_path.display()
                );
                match source.download_and_verify_pkg_archive(temp_file_path.as_path()) {
                    Ok(download_duration) => {
                        std::fs::create_dir_all(source_store_path.as_ref())
                            .map_err(DownloadError::UnexpectedIOError)?;
                        std::fs::rename(temp_file_path.as_path(), source_archive_path.as_ref())
                            .map_err(DownloadError::UnexpectedIOError)?;
                        let source_ctx = SourceContext::read_from_disk(source_archive_path)
                            .map_err(DownloadError::UnexpectedError)?;
                        self.store
                            .get_connection()
                            .map_err(DownloadError::UnexpectedError)?
                            .transaction(|connection| {
                                store::source_context_put(connection, &source.shasum, &source_ctx)
                            })
                            .map_err(DownloadError::UnexpectedError)?;

                        results.push(DownloadStatus::Downloaded(
                            source_ctx,
                            plan_ctx.clone(),
                            source.clone(),
                            download_duration,
                        ));
                    }
                    Err(PackageSourceDownloadError::Sha256SumMismatch(expected, actual)) => results
                        .push(DownloadStatus::InvalidArchive(
                            plan_ctx.clone(),
                            source.clone(),
                            actual,
                        )),
                    Err(err) => return Err(DownloadError::UnexpectedDownloadError(err)),
                }
            } else {
                results.push(DownloadStatus::NoSource(plan_ctx.clone()))
            }
        }
        Ok(results)
    }

    fn node_dep_analysis(
        &self,
        node_index: NodeIndex,
        analysis_type: AnalysisType,
    ) -> Result<Vec<&'_ Dependency>> {
        let nodes = match analysis_type {
            AnalysisType::Dependencies => self.dep_graph.get_deps(
                Some(&node_index),
                [DependencyType::Runtime].into_iter().collect(),
                DependencyDepth::Direct,
                DependencyDirection::Forward,
                false,
                true,
            ),
            AnalysisType::BuildDependencies => self.dep_graph.get_deps(
                Some(&node_index),
                [DependencyType::Build].into_iter().collect(),
                DependencyDepth::Direct,
                DependencyDirection::Forward,
                false,
                true,
            ),
            AnalysisType::TransitiveDependencies => self.dep_graph.get_deps(
                Some(&node_index),
                [DependencyType::Runtime].into_iter().collect(),
                DependencyDepth::Transitive,
                DependencyDirection::Forward,
                false,
                true,
            ),
            AnalysisType::TransitiveBuildDependencies => {
                let build_deps = self.dep_graph.get_deps(
                    Some(&node_index),
                    [DependencyType::Build].into_iter().collect(),
                    DependencyDepth::Direct,
                    DependencyDirection::Forward,
                    false,
                    true,
                );
                self.dep_graph.get_deps(
                    &build_deps,
                    [DependencyType::Build, DependencyType::Runtime]
                        .into_iter()
                        .collect(),
                    DependencyDepth::Transitive,
                    DependencyDirection::Forward,
                    true,
                    true,
                )
            }
            AnalysisType::StudioDependency => self.dep_graph.get_deps(
                Some(&node_index),
                [DependencyType::Studio].into_iter().collect(),
                DependencyDepth::Direct,
                DependencyDirection::Forward,
                false,
                true,
            ),
            AnalysisType::ReverseDependencies => self.dep_graph.get_deps(
                Some(&node_index),
                [DependencyType::Runtime].into_iter().collect(),
                DependencyDepth::Transitive,
                DependencyDirection::Reverse,
                false,
                true,
            ),
            AnalysisType::ReverseBuildDependencies => self.dep_graph.get_deps(
                Some(&node_index),
                [
                    DependencyType::Build,
                    DependencyType::Runtime,
                    DependencyType::Studio,
                ]
                .into_iter()
                .collect(),
                DependencyDepth::Transitive,
                DependencyDirection::Reverse,
                false,
                true,
            ),
        };
        Ok(nodes
            .into_iter()
            .map(|n| &self.dep_graph.build_graph[n])
            .collect())
    }

    pub fn changes(&self) -> Vec<RepoChanges<'_>> {
        self.dep_graph
            .detect_changes_in_repos()
            .into_iter()
            .map(|(repo_ctx_id, changes)| RepoChanges {
                repo: self.repos.get(&repo_ctx_id).unwrap(),
                changes: changes
                    .into_iter()
                    .filter_map(|(dep_index, causes)| {
                        match &self.dep_graph.build_graph[dep_index] {
                            Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => None,
                            Dependency::LocalPlan(plan_ctx) => {
                                Some(DependencyChange { plan_ctx, causes })
                            }
                        }
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn get_plan_contexts(&self, package: &PackageDepIdent) -> Vec<&PlanContext> {
        self.dep_graph
            .get_plan_nodes(package)
            .into_iter()
            .filter_map(|node_index| self.dep_graph.dep(node_index).plan_ctx())
            .collect()
    }

    pub fn get_connection(&self) -> Result<PooledConnection<ConnectionManager<SqliteConnection>>> {
        self.store.get_connection()
    }

    pub fn add_plans_to_changes(
        &mut self,
        connection: &mut SqliteConnection,
        package: &PackageDepIdent,
    ) -> Result<Vec<AddStatus>, AddError> {
        let plan_node_indices = self.dep_graph.get_plan_nodes(package);
        if plan_node_indices.is_empty() {
            return Err(AddError::PlansNotFound(package.to_owned()));
        }
        let mut results = Vec::new();
        for plan_node_index in plan_node_indices {
            let mut is_added = false;
            match self.dep_graph.dep_mut(plan_node_index) {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(ref mut plan_ctx) => {
                    // Find the file with the greatest last modification timestamp
                    // within the plan context, if it was previously altered.
                    let mut greatest_modified_at: Option<DateTime<Utc>> = None;
                    if let Some(existing_paths) = store::plan_context_alternate_modified_at_delete(
                        connection,
                        &plan_ctx.context_path,
                    )
                    .map_err(AddError::UnexpectedError)?
                    {
                        for (_, (real_modified_at, _)) in existing_paths {
                            if let Some(modified_at) = greatest_modified_at.as_mut() {
                                if *modified_at < real_modified_at {
                                    *modified_at = real_modified_at;
                                }
                            } else {
                                greatest_modified_at = Some(real_modified_at);
                            }
                        }
                        is_added = true;
                    }

                    if let Some(latest_plan_artifact) = plan_ctx.latest_artifact.as_ref() {
                        // If the file with the greatest modified timestamp is more
                        // recent than the last built artifact, we do not need to artifically
                        // make a more recently modified target context folder.
                        let mut touch_target_ctx = true;
                        if let Some(modified_at) = greatest_modified_at {
                            if modified_at > latest_plan_artifact.created_at {
                                touch_target_ctx = false;
                            }
                        }

                        // If we need to artificially modify the target context
                        // and the plan's context is older the last artifact update it.
                        if touch_target_ctx
                            && plan_ctx.target_context_last_modified_at
                                < latest_plan_artifact.created_at
                        {
                            let alternate_modified_at =
                                latest_plan_artifact.created_at + Duration::seconds(1);

                            if store::file_alternate_modified_at_put(
                                connection,
                                &plan_ctx.context_path,
                                plan_ctx.target_context_path.clone(),
                                plan_ctx.target_context_last_modified_at,
                                alternate_modified_at,
                            )
                            .map_err(AddError::UnexpectedError)?
                            {
                                is_added = true;
                            }
                        }
                    } else {
                        is_added = false
                    }
                    if is_added {
                        plan_ctx
                            .determine_changes(
                                Some(connection),
                                None,
                                self.artifact_cache.latest_plan_artifact(&plan_ctx.id),
                            )
                            .map_err(AddError::UnexpectedError)?;
                        results.push(AddStatus::Added(plan_ctx.id.clone()));
                    } else {
                        results.push(AddStatus::AlreadyAdded(plan_ctx.id.clone()));
                    }
                }
            }
        }
        Ok(results)
    }

    pub fn remove_plans_from_changes(
        &mut self,
        connection: &mut SqliteConnection,
        package: &PackageDepIdent,
    ) -> Result<Vec<RemoveStatus>, RemoveError> {
        let plan_node_indices = self.dep_graph.get_plan_nodes(package);
        if plan_node_indices.is_empty() {
            return Err(RemoveError::PlansNotFound(package.to_owned()));
        }
        let mut results = Vec::new();
        let plan_node_changes = self.dep_graph.detect_changes_in_deps(&plan_node_indices);
        for plan_node_index in plan_node_indices {
            let mut is_removed = false;
            match self.dep_graph.dep_mut(plan_node_index) {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(ref mut plan_ctx) => {
                    let causes = plan_node_changes.get(&plan_node_index);
                    if let Some(causes) = causes {
                        let mut blocking_causes = Vec::new();
                        for cause in causes {
                            match cause {
                                DependencyChangeCause::PlanContextChanged { .. } => {}
                                DependencyChangeCause::DependencyStudioNeedRebuild { .. } => {}
                                DependencyChangeCause::DependencyArtifactsUpdated { .. }
                                | DependencyChangeCause::DependencyPlansNeedRebuild { .. }
                                | DependencyChangeCause::NoBuiltArtifact => {
                                    blocking_causes.push(cause.clone())
                                }
                            }
                        }
                        if !blocking_causes.is_empty() {
                            results.push(RemoveStatus::CannotRemove(
                                plan_ctx.id.clone(),
                                blocking_causes,
                            ));
                            continue;
                        }
                    }

                    let latest_plan_artifact = plan_ctx.latest_artifact.as_ref().unwrap();
                    for changed_file in plan_ctx.files_changed.iter() {
                        if store::file_alternate_modified_at_put(
                            connection,
                            &plan_ctx.context_path,
                            changed_file.path.clone(),
                            changed_file.real_last_modified_at,
                            latest_plan_artifact.created_at,
                        )
                        .map_err(RemoveError::UnexpectedError)?
                        {
                            is_removed = true;
                        }
                    }

                    if is_removed {
                        plan_ctx
                            .determine_changes(
                                Some(connection),
                                None,
                                self.artifact_cache.latest_plan_artifact(&plan_ctx.id),
                            )
                            .map_err(RemoveError::UnexpectedError)?;
                        results.push(RemoveStatus::Removed(plan_ctx.id.clone()));
                    } else {
                        results.push(RemoveStatus::AlreadyRemoved(plan_ctx.id.clone()));
                    }
                }
            }
        }
        Ok(results)
    }

    pub fn build_dry_run(&self, packages: Vec<PackageDepIdent>) -> BuildDryRun {
        let package_indices = packages
            .into_iter()
            .flat_map(|dep_ident| self.dep_graph.get_dep_nodes(&dep_ident))
            .collect::<Vec<_>>();
        let base_changes_graph = self.dep_graph.detect_changes();

        let mut changes_graph = base_changes_graph
            .filter_map(|node_index, node| Some(node), |edge_index, edge| Some(edge));

        if !package_indices.is_empty() {
            changes_graph = changes_graph.filter_map(
                |node_index, node| {
                    for package_index in package_indices.iter() {
                        if changes_graph.contains_node(*package_index)
                            && algo::has_path_connecting(
                                &changes_graph,
                                *package_index,
                                node_index,
                                None,
                            )
                        {
                            return Some(*node);
                        }
                    }
                    None
                },
                |edge_index, edge| Some(*edge),
            );
        }

        let mut build_order = algo::toposort(&changes_graph, None).unwrap();
        build_order.reverse();

        BuildDryRun {
            order: build_order
                .into_iter()
                .map(|n| {
                    (
                        &self.dep_graph.build_graph[n],
                        self.node_dep_analysis(n, AnalysisType::StudioDependency)
                            .unwrap()
                            .pop(),
                        self.dep_graph
                            .get_deps(
                                Some(n).iter(),
                                [DependencyType::Build, DependencyType::Runtime]
                                    .into_iter()
                                    .collect(),
                                DependencyDepth::Direct,
                                DependencyDirection::Forward,
                                false,
                                false,
                            )
                            .into_iter()
                            .filter_map(|d| match &self.dep_graph.build_graph[d] {
                                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {
                                    Some(&self.dep_graph.build_graph[d])
                                }
                                Dependency::LocalPlan(_) => None,
                            })
                            .collect::<Vec<_>>(),
                        changes_graph[n].clone(),
                    )
                })
                .collect(),
        }
    }

    pub fn build(&self, packages: Vec<PackageDepIdent>) -> Result<()> {
        let package_indices = packages
            .into_iter()
            .flat_map(|dep_ident| self.dep_graph.get_dep_nodes(&dep_ident))
            .collect::<Vec<_>>();
        let base_changes_graph = self.dep_graph.detect_changes();

        let mut changes_graph = base_changes_graph
            .filter_map(|node_index, node| Some(node), |edge_index, edge| Some(edge));

        if !package_indices.is_empty() {
            changes_graph = changes_graph.filter_map(
                |node_index, node| {
                    for package_index in package_indices.iter() {
                        if changes_graph.contains_node(*package_index)
                            && algo::has_path_connecting(
                                &changes_graph,
                                *package_index,
                                node_index,
                                None,
                            )
                        {
                            return Some(*node);
                        }
                    }
                    None
                },
                |edge_index, edge| Some(*edge),
            );
        }

        let mut build_order = algo::toposort(&changes_graph, None).unwrap();
        build_order.reverse();

        for dep in self.dep_graph.deps(&build_order) {
            match dep {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(plan_ctx) => {
                    self.build_plan(plan_ctx);
                }
            }
        }
        Ok(())
    }

    pub fn check(&self, package: &PackageDepIdent) -> Result<Vec<CheckStatus>> {
        let mut results = Vec::new();
        let package = package.to_resolved_dep_ident(PackageTarget::default());
        if let Some(artifact) = self.artifact_cache.latest_artifact(&package) {
            let dep_ident = PackageDepIdent::from(&artifact.id);
            let rules = if let Some(plan_node_index) =
                self.dep_graph.get_dep_nodes(&dep_ident).into_iter().next()
            {
                match &self.dep_graph.build_graph[plan_node_index] {
                    Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {
                        ContextRules::default()
                    }
                    Dependency::LocalPlan(plan_ctx) => plan_ctx.context_rules(),
                }
            } else {
                ContextRules::default()
            };
            let checker = Checker::new();
            let mut checker_context = CheckerContext::default();
            let artifact_violations = checker.artifact_context_check(
                &rules,
                &mut checker_context,
                &self.artifact_cache,
                artifact,
            );
            results.push(CheckStatus::CheckSucceeded(
                package,
                Vec::new(),
                artifact_violations,
            ));
        } else {
            results.push(CheckStatus::ArtifactNotFound(package));
        }
        Ok(results)
    }

    pub fn build_plan(&self, plan_ctx: &PlanContext) {
        info!(target: "user-ui", "Building {}", plan_ctx.plan_path.as_ref().display());
    }
}

// - Download package source to temp dir
// - Verify downloaded file
// - If verified, copy to source archive store
// - Scan source archive for license files and cache them for license checks
