use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Display,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{mpsc::channel, Arc, RwLock},
    time::Instant,
};

use chrono::Duration;
use color_eyre::{
    eyre::{eyre, Context, Result},
    Help,
};
use diesel::{
    r2d2::{ConnectionManager, PooledConnection},
    Connection, SqliteConnection,
};

use ignore::WalkBuilder;
use lazy_static::lazy_static;
use petgraph::{algo, stable_graph::NodeIndex, visit::IntoNodeReferences};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info, trace};

use crate::{
    check::{
        ArtifactCheck, Checker, CheckerContext, LeveledArtifactCheckViolation,
        LeveledSourceCheckViolation, PlanContextConfig, SourceCheck,
    },
    core::{
        ArtifactCache, ArtifactCachePath, Dependency, DependencyDepth, DependencyDirection,
        DependencyType, PackageSourceDownloadError, SourceContext,
    },
    store::{self, InvalidPackageSourceArchiveStorePath, Store},
};

use super::{
    habitat::{self, BuildError},
    ChangeDetectionMode, DepGraph, DepGraphData, DependencyChangeCause, PackageBuildVersion,
    PackageDepGlob, PackageDepIdent, PackageIdent, PackageName, PackageOrigin, PackageSha256Sum,
    PackageSource, PackageTarget, PackageVersion, PlanContext, PlanContextID,
    PlanContextPathGitSyncStatus, PlanScannerBuilder, RepoConfig, RepoContext, RepoContextID,
};

lazy_static! {
    pub static ref STANDARD_BUILD_STUDIO_PACKAGE: PackageDepIdent =
        PackageDepIdent::parse("core/hab-studio").unwrap();
    pub static ref BOOTSTRAP_BUILD_STUDIO_PACKAGE: PackageDepIdent =
        PackageDepIdent::parse("core/build-tools-hab-studio").unwrap();
    pub static ref DEFAULT_STORE_PATH: PathBuf = PathBuf::from(".hab-auto-build");
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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
    #[serde(default)]
    pub ignore_cycles: bool,
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
    studios: BuildStudioConfig,
    store: Store,
    repos: HashMap<RepoContextID, RepoContext>,
    dep_graph: DepGraph,
    artifact_cache: Arc<RwLock<ArtifactCache>>,
}

#[derive(Debug, Clone)]
pub(crate) struct PackageDiff {
    pub source: BTreeSet<PackageBuildVersion>,
    pub target: BTreeSet<PackageBuildVersion>,
}

pub(crate) struct DependencyChange<'a> {
    pub plan_ctx: &'a PlanContext,
    pub causes: Vec<DependencyChangeCause>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildStepStudio {
    Native,
    Bootstrap,
    Standard,
}

impl Display for BuildStepStudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildStepStudio::Native => write!(f, "native"),
            BuildStepStudio::Bootstrap => write!(f, "bootstrap"),
            BuildStepStudio::Standard => write!(f, "standard"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct CheckStep<'a> {
    pub index: NodeIndex,
    pub dependency: &'a Dependency,
}

#[derive(Debug)]
pub(crate) struct BuildStep<'a> {
    pub index: NodeIndex,
    pub repo_ctx: &'a RepoContext,
    pub plan_ctx: &'a PlanContext,
    pub studio: BuildStepStudio,
    pub allow_remote: bool,
    pub studio_package: Option<&'a PackageDepIdent>,
    pub origins: HashSet<PackageOrigin>,
    pub deps_to_install: Vec<&'a PlanContextID>,
    pub remote_deps: Vec<&'a Dependency>,
    pub causes: Vec<DependencyChangeCause>,
    pub build_duration: Option<Duration>,
}

#[derive(Debug)]
pub(crate) struct BuildStepResult {
    pub artifact_ident: PackageIdent,
    pub artifact_violations: Vec<LeveledArtifactCheckViolation>,
    pub build_log: PathBuf,
}

#[derive(Debug, Error)]
pub(crate) enum BuildStepError {
    #[error("Failed to complete build")]
    Build(#[from] BuildError),
    #[error("Failed to execute build step due to unexpected error")]
    Unexpected(#[from] color_eyre::eyre::Error),
}

pub(crate) struct BuildPlan<'a> {
    pub check_steps: Vec<CheckStep<'a>>,
    pub build_steps: Vec<BuildStep<'a>>,
}

pub(crate) enum AddStatus {
    Added(PlanContextID),
    AlreadyAdded(PlanContextID),
}

#[derive(Debug, Error)]
pub(crate) enum AddError {
    #[error("Encountered an unexpected error while trying to add the package to the change list")]
    UnexpectedError(#[from] color_eyre::eyre::Error),
}

pub(crate) enum RemoveStatus {
    Removed(PlanContextID),
    AlreadyRemoved(PlanContextID),
    CannotRemove(PlanContextID, Vec<DependencyChangeCause>),
}

#[derive(Debug, Error)]
pub(crate) enum RemoveError {
    #[error(
        "Encountered an unexpected error while trying to remove the package from the change list"
    )]
    UnexpectedError(#[from] color_eyre::eyre::Error),
}

pub(crate) struct PlanContextGitSyncStatus {
    pub id: PlanContextID,
    pub file_statuses: Vec<PlanContextPathGitSyncStatus>,
}

#[derive(Debug, Error)]
pub(crate) enum GitSyncError {
    #[error("Encountered an unexpected error while trying to sync the package changes with git")]
    UnexpectedError(#[from] color_eyre::eyre::Error),
}

pub(crate) enum PlanCheckStatus {
    CheckSucceeded(
        Vec<LeveledSourceCheckViolation>,
        Vec<LeveledArtifactCheckViolation>,
    ),
    ArtifactNotFound,
}

pub(crate) enum DownloadStatus {
    Downloaded(
        SourceContext,
        PlanContext,
        PackageSource,
        Duration,
        Vec<LeveledSourceCheckViolation>,
    ),
    AlreadyDownloaded(
        SourceContext,
        PlanContext,
        PackageSource,
        Vec<LeveledSourceCheckViolation>,
    ),
    MissingSource(PlanContext),
    NoSource,
    InvalidArchive(
        PlanContext,
        PackageSource,
        PackageSha256Sum,
        InvalidPackageSourceArchiveStorePath,
    ),
}

#[derive(Debug, Error)]
pub(crate) enum DownloadError {
    #[error("Sources for plan {0} is corrupt")]
    CorruptedSource(PlanContextID),
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
        let store = Store::new(&store_path).with_context(|| {
            format!(
                "Failed to initialize hab-auto-build store at {}",
                store_path.display()
            )
        })?;

        // Scan artifact cache
        let artifact_cache = ArtifactCache::new(ArtifactCachePath::default(), &store)?;

        let mut dir_walk_builder: Option<WalkBuilder> = None;
        for repo_ctx in repos.values() {
            if let Some(dir_walk_builder) = dir_walk_builder.as_mut() {
                dir_walk_builder.add(repo_ctx.path.as_ref());
            } else {
                let mut new_walk_builder = WalkBuilder::new(repo_ctx.path.as_ref());
                new_walk_builder.follow_links(false);
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

        let dep_graph = DepGraph::new(&config.studios, plans, config.ignore_cycles)?;

        Ok(AutoBuildContext {
            path: auto_build_ctx_path,
            studios: config.studios.clone(),
            store,
            repos,
            dep_graph,
            artifact_cache: Arc::new(RwLock::new(artifact_cache)),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.dep_graph.build_graph.node_count() == 0
    }

    pub fn dep_graph_data(&self) -> DepGraphData {
        DepGraphData::from(&self.dep_graph)
    }

    pub fn glob_deps(
        &self,
        globs: &[PackageDepGlob],
        target: PackageTarget,
    ) -> Result<Vec<NodeIndex>> {
        let mut results = Vec::new();
        for glob in globs {
            let glob = glob.matcher();
            results.extend(self.dep_graph.glob_deps(&glob, target));
        }
        Ok(results)
    }

    pub fn dep(&self, dep_node_index: NodeIndex) -> &Dependency {
        self.dep_graph.dep(dep_node_index)
    }

    pub fn dep_analysis<'a>(
        &'a self,
        dep_node_index: NodeIndex,
        analysis_types: &HashSet<AnalysisType>,
    ) -> Result<DependencyAnalysis<'a>> {
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

        Ok(DependencyAnalysis {
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
        })
    }

    pub fn compare(
        &self,
        source: &AutoBuildContext,
    ) -> HashMap<(PackageTarget, PackageOrigin, PackageName), PackageDiff> {
        let mut diffs: HashMap<(PackageTarget, PackageOrigin, PackageName), PackageDiff> =
            HashMap::new();
        for target_node_index in self.dep_graph.build_graph.node_indices() {
            let target_node = &self.dep_graph.build_graph[target_node_index];
            match target_node {
                Dependency::ResolvedDep(_) => {}
                Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(target_plan) => {
                    for source_node_index in source.dep_graph.build_graph.node_indices() {
                        let source_node = &source.dep_graph.build_graph[source_node_index];
                        match source_node {
                            Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                            Dependency::LocalPlan(source_plan) => {
                                let target_id = target_plan.id.as_ref();
                                let source_id = source_plan.id.as_ref();
                                if target_id.target == source_id.target
                                    && target_id.origin == source_id.origin
                                    && target_id.name == source_id.name
                                {
                                    let entry = diffs
                                        .entry((
                                            target_id.target,
                                            target_id.origin.clone(),
                                            target_id.name.clone(),
                                        ))
                                        .or_insert_with(|| PackageDiff {
                                            source: BTreeSet::default(),
                                            target: BTreeSet::default(),
                                        });
                                    entry.source.insert(source_plan.id.as_ref().version.clone());
                                    entry.target.insert(target_plan.id.as_ref().version.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        diffs
    }

    pub fn download_dep_source(
        &self,
        package_index: NodeIndex,
        check_source: bool,
    ) -> Result<DownloadStatus, DownloadError> {
        if let Some(plan_ctx) = self.dep_graph.dep(package_index).plan_ctx() {
            self.download_plan_source(plan_ctx, check_source)
        } else {
            Ok(DownloadStatus::NoSource)
        }
    }

    pub fn download_plan_source(
        &self,
        plan_ctx: &PlanContext,
        check_source: bool,
    ) -> Result<DownloadStatus, DownloadError> {
        if let Some(source) = &plan_ctx.source {
            let source_store_path = self.store.package_source_store_path(source);
            let source_archive_path = source_store_path.archive_data_path();

            let invalid_source_store_path = self.store.invalid_source_store_path(source);
            let invalid_source_archive_path = invalid_source_store_path.archive_data_path();

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
                        let source_ctx = if let Some(existing_source_ctx) = existing_source_ctx {
                            existing_source_ctx
                        } else {
                            let new_source_ctx = SourceContext::read_from_disk(
                                source_archive_path,
                                Some(source.shasum.clone()),
                            )
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
                        let source_violations = if check_source {
                            let checker = Checker::new();
                            checker.source_context_check_with_plan(
                                &plan_ctx.config(),
                                &plan_ctx,
                                &source_ctx,
                            )
                        } else {
                            vec![]
                        };
                        return Ok(DownloadStatus::AlreadyDownloaded(
                            source_ctx,
                            plan_ctx.clone(),
                            source.clone(),
                            source_violations,
                        ));
                    }
                    Err(_) => {
                        error!(target: "user-log", "Source for package {} is corrupted", plan_ctx.id);
                        return Err(DownloadError::CorruptedSource(plan_ctx.id.clone()));
                    }
                }
            }
            let tmp_dir = self
                .store
                .temp_dir("download")
                .map_err(DownloadError::UnexpectedError)?;
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
                    let source_ctx = SourceContext::read_from_disk(
                        source_archive_path,
                        Some(source.shasum.clone()),
                    )
                    .map_err(DownloadError::UnexpectedError)?;
                    self.store
                        .get_connection()
                        .map_err(DownloadError::UnexpectedError)?
                        .transaction(|connection| {
                            store::source_context_put(connection, &source.shasum, &source_ctx)
                        })
                        .map_err(DownloadError::UnexpectedError)?;
                    let source_violations = if check_source {
                        let checker = Checker::new();
                        checker.source_context_check_with_plan(
                            &plan_ctx.config(),
                            &plan_ctx,
                            &source_ctx,
                        )
                    } else {
                        vec![]
                    };
                    Ok(DownloadStatus::Downloaded(
                        source_ctx,
                        plan_ctx.clone(),
                        source.clone(),
                        download_duration,
                        source_violations,
                    ))
                }
                Err(PackageSourceDownloadError::Sha256SumMismatch(_expected, actual)) => {
                    std::fs::create_dir_all(invalid_source_store_path.as_ref())
                        .map_err(DownloadError::UnexpectedIOError)?;
                    std::fs::rename(
                        temp_file_path.as_path(),
                        invalid_source_archive_path.as_ref(),
                    )
                    .map_err(DownloadError::UnexpectedIOError)?;
                    Ok(DownloadStatus::InvalidArchive(
                        plan_ctx.clone(),
                        source.clone(),
                        actual,
                        invalid_source_archive_path,
                    ))
                }
                Err(err) => return Err(DownloadError::UnexpectedDownloadError(err)),
            }
        } else {
            Ok(DownloadStatus::MissingSource(plan_ctx.clone()))
        }
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

    pub fn changes(
        &self,
        package_indices: &[NodeIndex],
        change_detection_mode: ChangeDetectionMode,
        build_target: PackageTarget,
    ) -> Vec<RepoChanges<'_>> {
        self.dep_graph
            .detect_changes_in_repos(change_detection_mode, build_target)
            .into_iter()
            .map(|(repo_ctx_id, changes)| RepoChanges {
                repo: self.repos.get(&repo_ctx_id).unwrap(),
                changes: changes
                    .into_iter()
                    .filter_map(|(dep_index, causes)| {
                        if package_indices.contains(&dep_index) {
                            match &self.dep_graph.build_graph[dep_index] {
                                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => None,
                                Dependency::LocalPlan(plan_ctx) => {
                                    Some(DependencyChange { plan_ctx, causes })
                                }
                            }
                        } else {
                            None
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
        plan_node_indices: &[NodeIndex],
        build_target: PackageTarget,
    ) -> Result<Vec<AddStatus>, AddError> {
        let mut results = Vec::new();
        let plan_node_changes = self.dep_graph.detect_changes_in_deps(
            plan_node_indices,
            ChangeDetectionMode::Disk,
            build_target,
        );
        let artifact_cache = self.artifact_cache.read().unwrap();
        for plan_node_index in plan_node_indices {
            match self.dep_graph.dep_mut(*plan_node_index) {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(ref mut plan_ctx) => {
                    let causes = plan_node_changes.get(&plan_node_index);
                    if causes.is_none() {
                        let latest_plan_artifact = artifact_cache
                            .latest_plan_artifact(&plan_ctx.id)
                            .expect("Plan artifact must be present");

                        // Delete any modifications for the plan context that may be present
                        store::plan_context_alternate_modified_at_delete(
                            connection,
                            &plan_ctx.context_path,
                        )?;
                        plan_ctx.determine_changes(
                            Some(connection),
                            None,
                            Some(latest_plan_artifact),
                        )?;
                        if plan_ctx.files_changed_on_disk.is_empty() {
                            let alternate_modified_at =
                                latest_plan_artifact.created_at + Duration::seconds(1);
                            store::file_alternate_modified_at_put(
                                connection,
                                &plan_ctx.context_path,
                                plan_ctx.target_context_path.clone(),
                                plan_ctx.target_context_last_modified_at,
                                alternate_modified_at,
                            )?;
                            plan_ctx.determine_changes(
                                Some(connection),
                                None,
                                Some(latest_plan_artifact),
                            )?;
                        }
                        debug!(
                            "Plan {} has been forcefully added to the change list",
                            plan_ctx.id
                        );
                        results.push(AddStatus::Added(plan_ctx.id.clone()));
                    } else {
                        debug!("Plan {} already in change list", plan_ctx.id);
                        results.push(AddStatus::AlreadyAdded(plan_ctx.id.clone()));
                    }
                    assert!({
                        let plan_node_changes = self.dep_graph.detect_changes_in_deps(
                            &[*plan_node_index],
                            ChangeDetectionMode::Disk,
                            build_target,
                        );
                        let causes = plan_node_changes.get(&plan_node_index);
                        causes.is_some()
                    })
                }
            }
        }
        Ok(results)
    }

    pub fn sync_plans_with_git(
        &mut self,
        connection: &mut SqliteConnection,
        plan_node_indices: &[NodeIndex],
        is_dry_run: bool,
        build_target: PackageTarget,
    ) -> Result<BTreeMap<RepoContextID, Vec<PlanContextGitSyncStatus>>, GitSyncError> {
        let mut results: BTreeMap<RepoContextID, Vec<PlanContextGitSyncStatus>> = BTreeMap::new();
        for plan_node_index in plan_node_indices {
            match self.dep_graph.dep_mut(*plan_node_index) {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(ref mut plan_ctx) => {
                    let sync_results = plan_ctx.sync_changes_with_git(is_dry_run)?;
                    if !sync_results.is_empty() {
                        results.entry(plan_ctx.repo_id.clone()).or_default().push(
                            PlanContextGitSyncStatus {
                                id: plan_ctx.id.clone(),
                                file_statuses: sync_results,
                            },
                        );
                    }
                    if !is_dry_run {
                        // Delete any modifications for the plan context that may be present
                        store::plan_context_alternate_modified_at_delete(
                            connection,
                            &plan_ctx.context_path,
                        )?;
                    }
                }
            }
        }
        Ok(results)
    }

    pub fn remove_plans_from_changes(
        &mut self,
        connection: &mut SqliteConnection,
        plan_node_indices: &[NodeIndex],
        build_target: PackageTarget,
    ) -> Result<Vec<RemoveStatus>, RemoveError> {
        let mut results = Vec::new();
        let plan_node_changes = self.dep_graph.detect_changes_in_deps(
            plan_node_indices,
            ChangeDetectionMode::Disk,
            build_target,
        );
        let artifact_cache = self.artifact_cache.read().unwrap();
        for plan_node_index in plan_node_indices {
            match self.dep_graph.dep_mut(*plan_node_index) {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(ref mut plan_ctx) => {
                    let causes = plan_node_changes.get(&plan_node_index);
                    if let Some(causes) = causes {
                        let mut blocking_causes = Vec::new();
                        for cause in causes {
                            match cause {
                                DependencyChangeCause::PlanContextChanged { .. }
                                | DependencyChangeCause::DependencyStudioNeedRebuild { .. } => {}
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
                        let latest_plan_artifact = artifact_cache
                            .latest_plan_artifact(&plan_ctx.id)
                            .expect("Plan artifact must be present");
                        // Delete any modifications for the plan context that may be present
                        store::plan_context_alternate_modified_at_delete(
                            connection,
                            &plan_ctx.context_path,
                        )?;
                        plan_ctx.determine_changes(
                            Some(connection),
                            None,
                            artifact_cache.latest_plan_artifact(&plan_ctx.id),
                        )?;
                        if !plan_ctx.files_changed_on_disk.is_empty() {
                            for changed_file in plan_ctx.files_changed_on_disk.iter() {
                                store::file_alternate_modified_at_put(
                                    connection,
                                    &plan_ctx.context_path,
                                    changed_file.path.clone(),
                                    changed_file.real_last_modified_at,
                                    latest_plan_artifact.created_at,
                                )?;
                            }
                            plan_ctx.determine_changes(
                                Some(connection),
                                None,
                                artifact_cache.latest_plan_artifact(&plan_ctx.id),
                            )?;
                        }
                        results.push(RemoveStatus::Removed(plan_ctx.id.clone()));
                    } else {
                        results.push(RemoveStatus::AlreadyRemoved(plan_ctx.id.clone()));
                    }
                    assert!({
                        let plan_node_changes = self.dep_graph.detect_changes_in_deps(
                            &[*plan_node_index],
                            ChangeDetectionMode::Disk,
                            build_target,
                        );
                        let causes = plan_node_changes.get(&plan_node_index);
                        causes.is_none()
                    })
                }
            }
        }
        Ok(results)
    }

    pub fn build_plan_generate(
        &self,
        package_indices: Vec<NodeIndex>,
        change_detection_mode: ChangeDetectionMode,
        build_target: PackageTarget,
        allow_remote: bool,
    ) -> Result<BuildPlan> {
        let base_changes_graph = self
            .dep_graph
            .detect_changes(change_detection_mode, build_target);

        let mut changes_graph = base_changes_graph.filter_map(
            |_node_index, node| Some(node),
            |_edge_index, edge| Some(edge),
        );

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
                |_edge_index, edge| Some(*edge),
            );
        }
        let node_indices = changes_graph.node_indices().into_iter().collect::<Vec<_>>();
        let mut check_deps = self.dep_graph.get_deps(
            &node_indices,
            vec![
                DependencyType::Runtime,
                DependencyType::Build,
                DependencyType::Studio,
            ]
            .into_iter()
            .collect(),
            DependencyDepth::Transitive,
            DependencyDirection::Forward,
            false,
            true,
        );
        check_deps.reverse();
        let mut build_order = algo::toposort(&changes_graph, None).unwrap();
        build_order.reverse();
        self.store.get_connection()?.transaction(|connection| {
            Ok(BuildPlan {
                check_steps: check_deps
                    .into_iter()
                    .filter(|node_index| !build_order.contains(node_index))
                    .map(|node_index| CheckStep {
                        index: node_index,
                        dependency: &self.dep_graph.build_graph[node_index],
                    })
                    .collect(),
                build_steps: build_order
                    .into_iter()
                    .map(|node_index| {
                        let (studio, studio_package) = match self
                            .node_dep_analysis(node_index, AnalysisType::StudioDependency)
                            .unwrap()
                            .pop()
                        {
                            Some(package_dep)
                                if package_dep.matches_dep_ident(&self.studios.bootstrap) =>
                            {
                                (BuildStepStudio::Bootstrap, Some(&self.studios.bootstrap))
                            }
                            Some(package_dep)
                                if package_dep.matches_dep_ident(&self.studios.standard) =>
                            {
                                (BuildStepStudio::Standard, Some(&self.studios.standard))
                            }
                            None => (BuildStepStudio::Native, None),
                            Some(package_dep) => {
                                panic!("Invalid studio dependency {:?}", package_dep);
                            }
                        };
                        let deps_to_install = self
                            .dep_graph
                            .get_deps(
                                Some(node_index).iter(),
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
                                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => None,
                                Dependency::LocalPlan(plan_ctx) => Some(&plan_ctx.id),
                            })
                            .collect::<Vec<_>>();
                        let origins = self
                            .dep_graph
                            .get_deps(
                                Some(node_index).iter(),
                                [DependencyType::Build, DependencyType::Runtime]
                                    .into_iter()
                                    .collect(),
                                DependencyDepth::Transitive,
                                DependencyDirection::Forward,
                                true,
                                false,
                            )
                            .into_iter()
                            .filter_map(|d| match &self.dep_graph.build_graph[d] {
                                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => None,
                                Dependency::LocalPlan(plan_ctx) => {
                                    Some(plan_ctx.id.as_ref().origin.clone())
                                }
                            })
                            .collect::<HashSet<_>>();
                        let plan_ctx = self.dep_graph.build_graph[node_index]
                            .plan_ctx()
                            .expect("Dependency must be a plan");
                        let repo_ctx = self
                            .repos
                            .get(&plan_ctx.repo_id)
                            .expect("Plan must belong to a repo");
                        let build_duration =
                            store::build_time_get(connection, plan_ctx.id.as_ref())?
                                .map(|value| Duration::seconds(value.duration_in_secs as i64));
                        let remote_deps = self
                            .dep_graph
                            .get_deps(
                                Some(node_index).iter(),
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
                            .collect::<Vec<_>>();
                        Ok(BuildStep {
                            index: node_index,
                            repo_ctx,
                            plan_ctx,
                            studio,
                            studio_package,
                            deps_to_install,
                            origins,
                            allow_remote,
                            remote_deps,
                            causes: changes_graph[node_index].clone(),
                            build_duration,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            })
        })
    }

    pub fn package_check(&self, package_index: NodeIndex) -> Result<PlanCheckStatus> {
        let artifact_cache = self.artifact_cache.read().unwrap();
        let (plan_config, artifact) = {
            match &self.dep_graph.build_graph[package_index] {
                Dependency::ResolvedDep(ident) => {
                    (PlanContextConfig::default(), artifact_cache.artifact(ident))
                }
                Dependency::RemoteDep(resolved_dep_ident) => (
                    PlanContextConfig::default(),
                    artifact_cache.latest_artifact(resolved_dep_ident),
                ),
                Dependency::LocalPlan(plan_ctx) => (
                    plan_ctx.config(),
                    artifact_cache.latest_plan_artifact(&plan_ctx.id),
                ),
            }
        };
        let source_violations = match self.download_dep_source(package_index, true)? {
            DownloadStatus::Downloaded(_source_ctx, _plan_ctx, _, _, source_violations) => {
                Some(source_violations)
            }
            DownloadStatus::AlreadyDownloaded(_source_ctx, _plan_ctx, _, source_violations) => {
                Some(source_violations)
            }
            DownloadStatus::MissingSource(_) | DownloadStatus::InvalidArchive(_, _, _, _) => None,
            DownloadStatus::NoSource => {
                panic!("Cannot check dependencies that are not plans")
            }
        };
        let artifact_violations = if let Some(artifact) = artifact {
            let checker = Checker::new();
            let mut checker_context = CheckerContext::default();
            Some(checker.artifact_context_check(
                &plan_config,
                &mut checker_context,
                &artifact_cache,
                artifact,
            ))
        } else {
            None
        };
        Ok(PlanCheckStatus::CheckSucceeded(
            source_violations.unwrap_or_default(),
            artifact_violations.unwrap_or_default(),
        ))
    }

    pub fn build_step_execute(
        &self,
        build_step: &BuildStep<'_>,
    ) -> Result<BuildStepResult, BuildStepError> {
        let mut artifact_cache = self.artifact_cache.write().unwrap();
        let start = Instant::now();
        let build_output = {
            match build_step.studio {
                BuildStepStudio::Native => {
                    habitat::native_package_build(&build_step, &artifact_cache, &self.store)?
                }
                BuildStepStudio::Bootstrap => {
                    habitat::bootstrap_package_build(&build_step, &artifact_cache, &self.store, 1)?
                }
                BuildStepStudio::Standard => {
                    habitat::standard_package_build(&build_step, &artifact_cache, &self.store, 1)?
                }
            }
        };
        // Add the artifact to the cache
        let artifact_ident = artifact_cache.artifact_add(&self.store, build_output.artifact)?;
        let artifact_ctx = artifact_cache.artifact(&artifact_ident).unwrap();
        // Check the artifact for violations
        let checker = Checker::new();
        let mut checker_context = CheckerContext::default();
        let artifact_violations = checker.artifact_context_check(
            &build_step.plan_ctx.config(),
            &mut checker_context,
            &artifact_cache,
            &artifact_ctx,
        );
        let elapsed_duration_in_secs = start.elapsed().as_secs() as i32;
        self.store.get_connection()?.transaction(|connection| {
            store::build_time_put(
                connection,
                build_step.plan_ctx.id.as_ref(),
                elapsed_duration_in_secs,
            )
        })?;

        Ok(BuildStepResult {
            artifact_ident,
            artifact_violations,
            build_log: build_output.build_log,
        })
    }
}
