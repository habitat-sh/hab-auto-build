use crate::core::{BOOTSTRAP_BUILD_STUDIO_PACKAGE, STANDARD_BUILD_STUDIO_PACKAGE};

use super::{
    BuildStudioConfig, PackageBuildIdent, PackageBuildVersion, PackageDepGlobMatcher,
    PackageDepIdent, PackageIdent, PackageName, PackageOrigin, PackageRelease,
    PackageResolvedDepIdent, PackageTarget, PackageVersion, PlanContext,
    PlanContextFileChangeOnDisk, PlanContextFileChangeOnGit, PlanContextID,
    PlanContextLatestArtifact, PlanFilePath, RepoContextID,
};

use clap::ValueEnum;
use color_eyre::{
    eyre::{eyre, Result},
    Help, SectionExt,
};
use emoji_printer::print_emojis;

use petgraph::{
    algo::{self, greedy_feedback_arc_set},
    stable_graph::{NodeIndex, StableGraph},
    visit::{EdgeRef, IntoNodeReferences},
    Directed, Direction,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Display,
    hash::Hash,
    time::Instant,
};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum, PartialOrd, Ord, Serialize)]
pub(crate) enum DependencyType {
    #[serde(rename = "studio")]
    Studio,
    #[serde(rename = "runtime")]
    Runtime,
    #[serde(rename = "build")]
    Build,
}

impl Display for DependencyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DependencyType::Build => write!(f, "build"),
            DependencyType::Runtime => write!(f, "runtime"),
            DependencyType::Studio => write!(f, "studio"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DependencyDepth {
    Direct,
    Transitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DependencyDirection {
    Forward,
    Reverse,
}

impl From<DependencyDirection> for Direction {
    fn from(value: DependencyDirection) -> Self {
        match value {
            DependencyDirection::Forward => Direction::Outgoing,
            DependencyDirection::Reverse => Direction::Incoming,
        }
    }
}

type PackageVersionList = HashMap<
    PackageOrigin,
    HashMap<
        PackageName,
        HashMap<PackageTarget, BTreeMap<PackageBuildVersion, BTreeMap<PackageRelease, NodeIndex>>>,
    >,
>;

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "dependency_type", content = "data")]
pub(crate) enum Dependency {
    #[serde(rename = "resolved_dependency")]
    ResolvedDep(PackageIdent),
    #[serde(rename = "remote_dependency")]
    RemoteDep(PackageResolvedDepIdent),
    #[serde(rename = "local_plan")]
    LocalPlan(PlanContext),
}

impl std::fmt::Debug for Dependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResolvedDep(arg) => write!(f, "resolved:{:?}", arg),
            Self::RemoteDep(arg) => write!(f, "remote:{}", arg),
            Self::LocalPlan(arg) => write!(f, "plan:{}", arg.id),
        }
    }
}

impl Dependency {
    pub fn matches_glob(&self, glob: &PackageDepGlobMatcher, target: PackageTarget) -> bool {
        if self.target() != target {
            return false;
        }
        match self {
            Dependency::ResolvedDep(ident) => glob.matches_package_ident(ident),
            Dependency::RemoteDep(resolved_dep_ident) => {
                glob.matches_package_resolved_dep_ident(resolved_dep_ident)
            }
            Dependency::LocalPlan(plan_ctx) => {
                glob.matches_package_build_ident(plan_ctx.id.as_ref())
            }
        }
    }
    pub fn matches_dep_ident(&self, dep_ident: &PackageDepIdent) -> bool {
        match self {
            Dependency::ResolvedDep(ident) => ident.satisfies_dependency(dep_ident),
            Dependency::RemoteDep(resolved_dep_ident) => {
                resolved_dep_ident.satisfies_dependency(dep_ident)
            }
            Dependency::LocalPlan(plan_ctx) => plan_ctx.id.as_ref().satisfies_dependency(dep_ident),
        }
    }
    pub fn plan_ctx(&self) -> Option<&PlanContext> {
        match self {
            Dependency::ResolvedDep(_) => None,
            Dependency::RemoteDep(_) => None,
            Dependency::LocalPlan(plan_ctx) => Some(plan_ctx),
        }
    }
    pub fn target(&self) -> PackageTarget {
        match self {
            Dependency::ResolvedDep(ident) => ident.target,
            Dependency::RemoteDep(ident) => ident.target,
            Dependency::LocalPlan(plan_ctx) => plan_ctx.id.as_ref().target,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DependencyArtifactUpdated {
    latest_dep_artifact: PlanContextLatestArtifact,
    latest_plan_artifact: PlanContextLatestArtifact,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum DependencyChangeCause {
    PlanContextChanged {
        latest_plan_artifact: PlanContextLatestArtifact,
        files_changed_on_disk: Vec<PlanContextFileChangeOnDisk>,
        files_changed_on_git: Vec<PlanContextFileChangeOnGit>,
    },
    DependencyArtifactsUpdated {
        latest_plan_artifact: PlanContextLatestArtifact,
        updated_dep_artifacts: Vec<PlanContextLatestArtifact>,
    },
    DependencyStudioNeedRebuild {
        plan: PlanContextID,
    },
    DependencyPlansNeedRebuild {
        plans: BTreeSet<(DependencyType, PlanContextID, PlanFilePath)>,
    },
    NoBuiltArtifact,
}

impl DependencyChangeCause {
    pub fn to_emoji(&self) -> String {
        match self {
            DependencyChangeCause::PlanContextChanged { .. } => print_emojis(":scroll:"),
            DependencyChangeCause::DependencyArtifactsUpdated { .. } => print_emojis(":package:"),
            DependencyChangeCause::DependencyPlansNeedRebuild { .. } => {
                print_emojis(":construction:")
            }
            DependencyChangeCause::DependencyStudioNeedRebuild { .. } => {
                print_emojis(":studio_microphone:")
            }
            DependencyChangeCause::NoBuiltArtifact => print_emojis(":sparkles:"),
        }
    }
}

#[derive(Serialize, Debug)]
pub(crate) struct DepGraphData {
    pub nodes: HashMap<u32, Dependency>,
    pub edges: Vec<(u32, u32, DependencyType)>,
}

impl From<&DepGraph> for DepGraphData {
    fn from(dep_graph: &DepGraph) -> Self {
        let mut data = DepGraphData {
            nodes: HashMap::new(),
            edges: Vec::new(),
        };
        for node_index in dep_graph.build_graph.node_indices() {
            let node = dep_graph.build_graph[node_index].clone();
            data.nodes.insert(node_index.index() as u32, node);
        }
        for edge_index in dep_graph.build_graph.edge_indices() {
            if let Some((source, target)) = dep_graph.build_graph.edge_endpoints(edge_index) {
                let edge = dep_graph.build_graph[edge_index];
                data.edges
                    .push((source.index() as u32, target.index() as u32, edge));
            }
        }
        data
    }
}

pub(crate) struct DepGraph {
    pub build_graph: StableGraph<Dependency, DependencyType, Directed>,
    pub known_versions: PackageVersionList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ChangeDetectionMode {
    Git,
    Disk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum BuildOrder {
    Strict,
    Relaxed,
}

impl DepGraph {
    pub fn new(
        build_studio_config: &BuildStudioConfig,
        plan_ctxs: HashMap<PlanContextID, PlanContext>,
        ignore_cycles: bool,
    ) -> Result<DepGraph> {
        let start = Instant::now();
        let mut known_versions: PackageVersionList = PackageVersionList::default();
        let mut build_graph = StableGraph::new();
        let mut resolved_standard_build_studios: HashMap<PackageTarget, NodeIndex> = HashMap::new();
        let mut resolved_bootstrap_build_studios: HashMap<PackageTarget, NodeIndex> =
            HashMap::new();

        for (_, plan_ctx) in plan_ctxs {
            let build_ident = PackageBuildIdent::from(plan_ctx.id.clone());
            for package_dep in plan_ctx.deps.iter().chain(plan_ctx.build_deps.iter()) {
                // If the dep is fully specified we add it to our known versions
                if let Some(package_ident) = package_dep.to_ident() {
                    let dep_node_index =
                        build_graph.add_node(Dependency::ResolvedDep(package_ident.clone()));
                    known_versions
                        .entry(package_ident.origin)
                        .or_default()
                        .entry(package_ident.name)
                        .or_default()
                        .entry(package_ident.target)
                        .or_default()
                        .entry(PackageBuildVersion::Static(package_ident.version))
                        .or_default()
                        .entry(PackageRelease::Resolved(package_ident.release))
                        .or_insert(dep_node_index);
                }
            }
            let dep_node_index = build_graph.add_node(Dependency::LocalPlan(plan_ctx));
            // The current package is a build studio, record the node index
            if build_ident.satisfies_dependency(&build_studio_config.standard) {
                resolved_standard_build_studios.insert(build_ident.target, dep_node_index);
            }
            if build_ident.satisfies_dependency(&build_studio_config.bootstrap) {
                resolved_bootstrap_build_studios.insert(build_ident.target, dep_node_index);
            }
            known_versions
                .entry(build_ident.origin)
                .or_default()
                .entry(build_ident.name)
                .or_default()
                .entry(build_ident.target)
                .or_default()
                .entry(build_ident.version)
                .or_default()
                .entry(PackageRelease::Unresolved)
                .or_insert(dep_node_index);
        }

        let mut remote_deps: HashMap<PackageResolvedDepIdent, Vec<(NodeIndex, DependencyType)>> =
            HashMap::new();
        let mut dep_edges = Vec::new();
        for dep_node_index in build_graph.node_indices() {
            if let Dependency::LocalPlan(plan_ctx) = &build_graph[dep_node_index] {
                let dep_types = vec![
                    (DependencyType::Runtime, plan_ctx.deps.iter()),
                    (DependencyType::Build, plan_ctx.build_deps.iter()),
                ];
                for (dep_type, plan_deps) in dep_types.into_iter() {
                    for plan_dep in plan_deps {
                        let dep_plan_node_index = known_versions
                            .get(&plan_dep.origin)
                            .and_then(|v| v.get(&plan_dep.name))
                            .and_then(|v| v.get(&plan_dep.target))
                            .and_then(|v| match (&plan_dep.version, &plan_dep.release) {
                                // Get the resolved version and last or specified release
                                (PackageVersion::Resolved(version), release) => v
                                    .get(&PackageBuildVersion::Static(version.to_owned()))
                                    .and_then(|v| v.get(release)),
                                // Get the last version, last release
                                (PackageVersion::Unresolved, PackageRelease::Unresolved) => v
                                    .values()
                                    .rev()
                                    .next()
                                    .and_then(|v| v.values().rev().next()),
                                // This is impossible to reach
                                (PackageVersion::Unresolved, PackageRelease::Resolved(_)) => {
                                    panic!("Invalid package dependency: '{}'", plan_dep)
                                }
                            });
                        if let Some(dep_plan_node_index) = dep_plan_node_index {
                            dep_edges.push((dep_node_index, *dep_plan_node_index, dep_type));
                        } else {
                            remote_deps
                                .entry(plan_dep.to_owned())
                                .or_default()
                                .push((dep_node_index, dep_type));
                        }
                    }
                }
            }
        }
        for (a, b, dep_type) in dep_edges {
            build_graph.add_edge(a, b, dep_type);
        }
        for (remote_dep_ident, deps) in remote_deps {
            let remote_dep_index = build_graph.add_node(Dependency::RemoteDep(remote_dep_ident));
            for (dep_index, dep_type) in deps {
                build_graph.add_edge(dep_index, remote_dep_index, dep_type);
            }
        }

        let feedback_edges = greedy_feedback_arc_set(&build_graph)
            .map(|e| e.id())
            .collect::<Vec<_>>();
        for feedback_edge in feedback_edges.iter() {
            if ignore_cycles {
                build_graph.remove_edge(*feedback_edge);
            } else {
                if let Some((start, end)) = build_graph.edge_endpoints(*feedback_edge) {
                    error!(target: "user-log",
                        "Build dependency {:?} depends on {:?} which creates a cycle",
                        build_graph[start], build_graph[end]
                    );
                }
            }
        }

        info!(
            "Dependency graph with {} packages and {} dependencies built in {}s",
            build_graph.node_count(),
            build_graph.edge_count(),
            start.elapsed().as_secs_f32()
        );
        let mut dep_graph = DepGraph {
            build_graph,
            known_versions,
        };

        // If a package is native package then
        // Get Standard Studio's Deps
        let mut standard_build_studio_tdeps: HashMap<PackageTarget, Vec<NodeIndex>> =
            HashMap::new();
        let mut bootstrap_build_studio_tdeps: HashMap<PackageTarget, Vec<NodeIndex>> =
            HashMap::new();
        let dep_node_indices = dep_graph
            .build_graph
            .node_indices()
            .into_iter()
            .collect::<Vec<_>>();
        for dep_node_index in dep_node_indices {
            let dep_target = dep_graph.build_graph[dep_node_index].target();
            let standard_build_studio_node_index = resolved_standard_build_studios
                .entry(dep_target)
                .or_insert_with(|| {
                    dep_graph.build_graph.add_node(Dependency::RemoteDep(
                        STANDARD_BUILD_STUDIO_PACKAGE.to_resolved_dep_ident(dep_target),
                    ))
                })
                .to_owned();
            let bootstrap_build_studio_node_index = resolved_bootstrap_build_studios
                .entry(dep_target)
                .or_insert_with(|| {
                    dep_graph.build_graph.add_node(Dependency::RemoteDep(
                        BOOTSTRAP_BUILD_STUDIO_PACKAGE.to_resolved_dep_ident(dep_target),
                    ))
                })
                .to_owned();

            let standard_build_studio_tdeps = standard_build_studio_tdeps
                .entry(dep_target)
                .or_insert_with(|| {
                    dep_graph.get_deps(
                        Some(&standard_build_studio_node_index),
                        [DependencyType::Runtime, DependencyType::Build]
                            .into_iter()
                            .collect(),
                        DependencyDepth::Transitive,
                        DependencyDirection::Forward,
                        true,
                        true,
                    )
                });
            let bootstrap_build_studio_tdeps = bootstrap_build_studio_tdeps
                .entry(dep_target)
                .or_insert_with(|| {
                    dep_graph.get_deps(
                        Some(&bootstrap_build_studio_node_index),
                        [DependencyType::Runtime, DependencyType::Build]
                            .into_iter()
                            .collect(),
                        DependencyDepth::Transitive,
                        DependencyDirection::Forward,
                        true,
                        true,
                    )
                });
            let is_standard_build_studio_tdep =
                standard_build_studio_tdeps.contains(&dep_node_index);
            let is_bootstrap_build_studio_tdep =
                bootstrap_build_studio_tdeps.contains(&dep_node_index);

            match &dep_graph.build_graph[dep_node_index] {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => {}
                Dependency::LocalPlan(plan_ctx) => {
                    match (
                        is_standard_build_studio_tdep,
                        is_bootstrap_build_studio_tdep,
                    ) {
                        (true, true) => {
                            if !plan_ctx.is_native {
                                return Err(eyre!("The plan '{}' is a dependency of the standard studio({}) and the bootstrap studio ({})",plan_ctx.id, STANDARD_BUILD_STUDIO_PACKAGE.clone(), BOOTSTRAP_BUILD_STUDIO_PACKAGE.clone())
                                .with_suggestion(|| format!("Try making the plan '{}' a native package", plan_ctx.plan_path.as_ref().display()))
                                .with_section(|| format!("{:?}", standard_build_studio_tdeps.iter().map(|n| &dep_graph.build_graph[*n]).collect::<Vec<_>>()).header("Standard Build Studio Deps: "))
                                .with_section(|| format!("{:?}", bootstrap_build_studio_tdeps.iter().map(|n| &dep_graph.build_graph[*n]).collect::<Vec<_>>()).header("Bootstrap Build Studio Deps: ")));
                            }
                        }
                        (true, false) => {
                            if !plan_ctx.is_native {
                                dep_graph.build_graph.add_edge(
                                    dep_node_index,
                                    bootstrap_build_studio_node_index,
                                    DependencyType::Studio,
                                );
                            }
                        }
                        (false, true) => {
                            if !plan_ctx.is_native {
                                return Err(eyre!(
                                    "The plan '{}' is a dependency of the bootstrap studio({})",
                                    plan_ctx.id,
                                    BOOTSTRAP_BUILD_STUDIO_PACKAGE.clone()
                                )
                                .with_suggestion(|| {
                                    format!(
                                        "Try making the plan '{}' a native package",
                                        plan_ctx.plan_path.as_ref().display()
                                    )
                                })
                                .with_section(|| {
                                    format!(
                                        "{:?}",
                                        standard_build_studio_tdeps
                                            .iter()
                                            .map(|n| &dep_graph.build_graph[*n])
                                            .collect::<Vec<_>>()
                                    )
                                    .header("Standard Build Studio Deps: ")
                                })
                                .with_section(|| {
                                    format!(
                                        "{:?}",
                                        bootstrap_build_studio_tdeps
                                            .iter()
                                            .map(|n| &dep_graph.build_graph[*n])
                                            .collect::<Vec<_>>()
                                    )
                                    .header("Bootstrap Build Studio Deps: ")
                                }));
                            }
                        }
                        (false, false) => {
                            if !plan_ctx.is_native {
                                match (
                                    &dep_graph.build_graph[bootstrap_build_studio_node_index],
                                    &dep_graph.build_graph[standard_build_studio_node_index],
                                ) {
                                    (Dependency::ResolvedDep(_), Dependency::ResolvedDep(_))
                                    | (Dependency::ResolvedDep(_), Dependency::LocalPlan(_))
                                    | (Dependency::RemoteDep(_), Dependency::ResolvedDep(_))
                                    | (Dependency::RemoteDep(_), Dependency::RemoteDep(_))
                                    | (Dependency::RemoteDep(_), Dependency::LocalPlan(_))
                                    | (Dependency::LocalPlan(_), Dependency::ResolvedDep(_))
                                    | (Dependency::LocalPlan(_), Dependency::LocalPlan(_)) => {
                                        dep_graph.build_graph.add_edge(
                                            dep_node_index,
                                            standard_build_studio_node_index,
                                            DependencyType::Studio,
                                        );
                                    }
                                    // Use the bootstrap studio if there is a local / resolved bootstrap
                                    // studio plan and no local / resolved standard studio plan
                                    (Dependency::ResolvedDep(_), Dependency::RemoteDep(_))
                                    | (Dependency::LocalPlan(_), Dependency::RemoteDep(_)) => {
                                        dep_graph.build_graph.add_edge(
                                            dep_node_index,
                                            bootstrap_build_studio_node_index,
                                            DependencyType::Studio,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(dep_graph)
    }

    pub fn glob_deps(&self, glob: &PackageDepGlobMatcher, target: PackageTarget) -> Vec<NodeIndex> {
        self.build_graph
            .node_references()
            .filter_map(|(dep_node_index, dep)| {
                if dep.matches_glob(glob, target) {
                    Some(dep_node_index)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    }

    pub fn dep(&self, node_index: NodeIndex) -> &Dependency {
        &self.build_graph[node_index]
    }

    pub fn dep_mut(&mut self, node_index: NodeIndex) -> &mut Dependency {
        &mut self.build_graph[node_index]
    }

    pub fn deps<'a>(
        &self,
        node_indices: impl IntoIterator<Item = &'a NodeIndex>,
    ) -> Vec<&Dependency> {
        node_indices
            .into_iter()
            .map(|x| &self.build_graph[*x])
            .collect()
    }

    pub fn get_dep_nodes(&self, dep_ident: &PackageDepIdent) -> Vec<NodeIndex> {
        let mut dep_nodes = Vec::new();
        for (node_index, node) in self.build_graph.node_references() {
            match node {
                Dependency::ResolvedDep(package_ident) => {
                    if package_ident.satisfies_dependency(dep_ident) {
                        dep_nodes.push(node_index);
                    }
                }
                Dependency::RemoteDep(package_ident) => {
                    if package_ident.satisfies_dependency(dep_ident) {
                        dep_nodes.push(node_index);
                    }
                }
                Dependency::LocalPlan(plan_ctx) => {
                    if plan_ctx.id.as_ref().satisfies_dependency(dep_ident) {
                        dep_nodes.push(node_index);
                    }
                }
            }
        }
        dep_nodes
    }

    pub fn get_plan_nodes(&self, dep_ident: &PackageDepIdent) -> Vec<NodeIndex> {
        self.build_graph
            .node_references()
            .filter_map(|(node_index, node)| match node {
                Dependency::ResolvedDep(_) | Dependency::RemoteDep(_) => None,
                Dependency::LocalPlan(plan_ctx) => {
                    if plan_ctx.id.as_ref().satisfies_dependency(dep_ident) {
                        return Some(node_index);
                    } else {
                        None
                    }
                }
            })
            .collect()
    }

    pub fn detect_changes_in_deps<'a>(
        &self,
        node_indices: impl IntoIterator<Item = &'a NodeIndex>,
        change_detection_mode: ChangeDetectionMode,
        build_order: BuildOrder,
        build_target: PackageTarget,
    ) -> HashMap<NodeIndex, Vec<DependencyChangeCause>> {
        let node_indices = node_indices.into_iter().collect::<Vec<_>>();
        let changes = self.detect_changes(change_detection_mode, build_order, build_target);
        changes
            .node_references()
            .filter(|(key, _)| node_indices.contains(&key))
            .map(|(key, value)| (key, value.clone()))
            .collect()
    }

    pub fn detect_changes_in_repos(
        &self,
        change_detection_mode: ChangeDetectionMode,
        build_order: BuildOrder,
        build_target: PackageTarget,
    ) -> BTreeMap<RepoContextID, HashMap<NodeIndex, Vec<DependencyChangeCause>>> {
        let changed_deps = self.detect_changes(change_detection_mode, build_order, build_target);
        let mut changed_deps_by_repo: BTreeMap<
            RepoContextID,
            HashMap<NodeIndex, Vec<DependencyChangeCause>>,
        > = BTreeMap::new();

        for (node_index, causes) in changed_deps.node_references() {
            let repo_id = self.build_graph[node_index]
                .plan_ctx()
                .unwrap()
                .repo_id
                .clone();
            changed_deps_by_repo
                .entry(repo_id)
                .or_default()
                .insert(node_index, causes.clone());
        }
        changed_deps_by_repo
    }

    pub fn detect_changes(
        &self,
        change_detection_mode: ChangeDetectionMode,
        build_order: BuildOrder,
        build_target: PackageTarget,
    ) -> StableGraph<Vec<DependencyChangeCause>, DependencyType> {
        let dep_types = [
            DependencyType::Build,
            DependencyType::Runtime,
            DependencyType::Studio,
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        let mut changed_dep_causes: HashMap<NodeIndex, Vec<DependencyChangeCause>> = HashMap::new();
        for node_index in self.build_graph.node_indices() {
            let node = &self.build_graph[node_index];
            if let Dependency::LocalPlan(plan_ctx) = node {
                let mut causes = Vec::new();
                let PlanContext {
                    id,
                    latest_artifact,
                    files_changed_on_disk,
                    files_changed_on_git,
                    ..
                } = plan_ctx;
                if id.as_ref().target != build_target {
                    continue;
                }
                if let Some(latest_artifact) = latest_artifact {
                    match change_detection_mode {
                        ChangeDetectionMode::Git => {
                            if !files_changed_on_git.is_empty() {
                                causes.push(DependencyChangeCause::PlanContextChanged {
                                    latest_plan_artifact: latest_artifact.clone(),
                                    files_changed_on_disk: Vec::new(),
                                    files_changed_on_git: files_changed_on_git.clone(),
                                });
                            }
                        }
                        ChangeDetectionMode::Disk => {
                            if !files_changed_on_disk.is_empty() {
                                causes.push(DependencyChangeCause::PlanContextChanged {
                                    latest_plan_artifact: latest_artifact.clone(),
                                    files_changed_on_disk: files_changed_on_disk.clone(),
                                    files_changed_on_git: Vec::new(),
                                });
                            }
                        }
                    }

                    let mut updated_dep_artifacts = Vec::new();
                    for dep_node_index in self
                        .build_graph
                        .edges_directed(node_index, Direction::Outgoing)
                        .map(|e| e.target())
                    {
                        let dep_node = &self.build_graph[dep_node_index];
                        match dep_node {
                            Dependency::ResolvedDep(dep) => {
                                warn!(target: "user-log",
                                    "Not checking for updates to remote dependency: {}",
                                    dep
                                );
                            }
                            Dependency::RemoteDep(dep) => {
                                // TODO: Check whether the remote dependency was updated ?
                                warn!(target: "user-log",
                                    "Not checking for updates to remote dependency: {}",
                                    dep
                                );
                            }
                            Dependency::LocalPlan(dep_plan_ctx) => {
                                if let Some(latest_dep_artifact) =
                                    dep_plan_ctx.latest_artifact.as_ref()
                                {
                                    if latest_dep_artifact.created_at > latest_artifact.created_at {
                                        updated_dep_artifacts.push(latest_dep_artifact.clone());
                                    }
                                }
                            }
                        }
                    }
                    if !updated_dep_artifacts.is_empty() {
                        causes.push(DependencyChangeCause::DependencyArtifactsUpdated {
                            latest_plan_artifact: latest_artifact.clone(),
                            updated_dep_artifacts,
                        });
                    }
                } else {
                    causes.push(DependencyChangeCause::NoBuiltArtifact);
                }
                if !causes.is_empty() {
                    changed_dep_causes.entry(node_index).or_insert(causes);
                }
            }
        }
        // Get build_rdeps of changed dependencies
        let mut affected_node_indices = HashSet::new();
        let mut changed_node_indices = changed_dep_causes.keys().cloned().collect::<Vec<_>>();
        while !changed_node_indices.is_empty() {
            let changed_node_index = changed_node_indices.pop().unwrap();
            affected_node_indices.insert(changed_node_index);
            for (rev_dep_node_index, rev_dep_node_type) in self
                .build_graph
                .edges_directed(changed_node_index, Direction::Incoming)
                .filter(|e| dep_types.contains(e.weight()))
                .map(|e| (e.source(), e.weight()))
            {
                if !affected_node_indices.contains(&rev_dep_node_index) {
                    changed_node_indices.push(rev_dep_node_index);
                }
                let rev_dep_causes = changed_dep_causes.entry(rev_dep_node_index).or_default();
                if let DependencyType::Studio = rev_dep_node_type {
                    if rev_dep_causes
                        .iter_mut()
                        .find(|c| {
                            matches!(c, DependencyChangeCause::DependencyStudioNeedRebuild { .. })
                        })
                        .is_none()
                    {
                        let plan_ctx = self.build_graph[changed_node_index].plan_ctx().unwrap();
                        rev_dep_causes.push(DependencyChangeCause::DependencyStudioNeedRebuild {
                            plan: plan_ctx.id.clone(),
                        })
                    }
                } else {
                    if let Some(DependencyChangeCause::DependencyPlansNeedRebuild { plans }) =
                        rev_dep_causes.iter_mut().find(|c| {
                            matches!(c, DependencyChangeCause::DependencyPlansNeedRebuild { .. })
                        })
                    {
                        let plan_ctx = self.build_graph[changed_node_index].plan_ctx().unwrap();
                        plans.insert((
                            *rev_dep_node_type,
                            plan_ctx.id.clone(),
                            plan_ctx.plan_path.to_owned(),
                        ));
                    } else {
                        let plan_ctx = self.build_graph[changed_node_index].plan_ctx().unwrap();
                        rev_dep_causes.push(DependencyChangeCause::DependencyPlansNeedRebuild {
                            plans: vec![(
                                *rev_dep_node_type,
                                plan_ctx.id.clone(),
                                plan_ctx.plan_path.to_owned(),
                            )]
                            .into_iter()
                            .collect(),
                        })
                    }
                }
            }
        }
        self.build_graph.filter_map(
            |node_index, _node| {
                if let Some(causes) = changed_dep_causes.remove(&node_index) {
                    match build_order {
                        BuildOrder::Strict => Some(causes),
                        BuildOrder::Relaxed => {
                            // If a studio update is the only reason for a rebuild it is not necessary
                            if causes.len() == 1
                                && matches!(
                                    causes[0],
                                    DependencyChangeCause::DependencyStudioNeedRebuild { .. }
                                )
                            {
                                None
                            } else {
                                Some(causes)
                            }
                        }
                    }
                } else {
                    None
                }
            },
            |_edge_index, edge| Some(*edge),
        )
    }

    pub fn get_deps<'a>(
        &self,
        nodes: impl IntoIterator<Item = &'a NodeIndex>,
        dep_types: HashSet<DependencyType>,
        dep_depth: DependencyDepth,
        dep_direction: DependencyDirection,
        include_start_nodes: bool,
        topo_sort: bool,
    ) -> Vec<NodeIndex> {
        let mut node_indices = nodes.into_iter().cloned().collect::<Vec<_>>();
        let mut node_all_deps = HashSet::new();
        match dep_depth {
            DependencyDepth::Direct => {
                for node_index in node_indices {
                    for dep_node_index in self
                        .build_graph
                        .edges_directed(node_index, dep_direction.into())
                        .filter(|e| dep_types.contains(e.weight()))
                        .map(|e| match dep_direction {
                            DependencyDirection::Forward => e.target(),
                            DependencyDirection::Reverse => e.source(),
                        })
                    {
                        if !node_all_deps.contains(&dep_node_index) {
                            node_all_deps.insert(dep_node_index);
                        }
                    }
                    if include_start_nodes {
                        node_all_deps.insert(node_index);
                    }
                }
            }
            DependencyDepth::Transitive => {
                let mut nodes_to_skip = node_indices.len();
                while !node_indices.is_empty() {
                    let node_index = node_indices.pop().unwrap();
                    if include_start_nodes {
                        node_all_deps.insert(node_index);
                    } else {
                        if nodes_to_skip == 0 {
                            node_all_deps.insert(node_index);
                        }
                        if nodes_to_skip > 0 {
                            nodes_to_skip -= 1;
                        }
                    }
                    for dep_node_index in self
                        .build_graph
                        .edges_directed(node_index, dep_direction.into())
                        .filter(|e| dep_types.contains(e.weight()))
                        .map(|e| match dep_direction {
                            DependencyDirection::Forward => e.target(),
                            DependencyDirection::Reverse => e.source(),
                        })
                    {
                        if !node_all_deps.contains(&dep_node_index) {
                            node_indices.push(dep_node_index);
                        }
                    }
                }
            }
        }

        let dep_graph = self.build_graph.filter_map(
            |node_index, node| {
                if node_all_deps.contains(&node_index) {
                    Some(node)
                } else {
                    None
                }
            },
            |_, edge| Some(edge),
        );
        if topo_sort {
            match dep_direction {
                DependencyDirection::Forward => algo::toposort(&dep_graph, None)
                    .expect("Cycles detected")
                    .into_iter()
                    .collect::<Vec<_>>(),
                DependencyDirection::Reverse => algo::toposort(&dep_graph, None)
                    .expect("Cycles detected")
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>(),
            }
        } else {
            dep_graph.node_indices().into_iter().collect::<Vec<_>>()
        }
    }
}
