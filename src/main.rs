use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use core::cmp::Ordering;
use dashmap::DashSet;
use futures::{stream::FuturesUnordered, StreamExt};
use names::{Generator, Name};
use petgraph::{
    algo::{self, greedy_feedback_arc_set},
    dot::{Config, Dot},
    stable_graph::{EdgeIndex, NodeIndex},
    visit::{EdgeRef, IntoEdgeReferences, IntoNodeReferences, NodeFiltered},
    Direction, Graph,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    env,
    ffi::OsString,
    fmt::Display,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tempdir::TempDir;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    task::JoinHandle,
};
use tracing::{debug, error, info, trace, warn};

lazy_static::lazy_static! {
    static ref PLAN_FILE_LOCATIONS: Vec<PathBuf> = vec![
        PathBuf::from("aarch64-linux").join("plan.sh"),
        PathBuf::from("aarch64-darwin").join("plan.sh"),
        PathBuf::from("x86_64-linux").join("plan.sh"),
        PathBuf::from("x86_64-windows").join("plan.sh"),
        PathBuf::from("habitat").join("aarch64-linux").join("plan.sh"),
        PathBuf::from("habitat").join("aarch64-darwin").join("plan.sh"),
        PathBuf::from("habitat").join("x86_64-linux").join("plan.sh"),
        PathBuf::from("habitat").join("x86_64-windows").join("plan.sh"),
        PathBuf::from("plan.sh"),
        PathBuf::from("habitat").join("plan.sh"),

    ];
    static ref HAB_PKGS_PATH: PathBuf = {
        let mut path = PathBuf::from("/hab");
        path.join("pkgs")
    };
    static ref PLAN_FILE_NAME: OsString =  OsString::from("plan.sh");
    static ref HAB_DEFAULT_BOOTSTRAP_STUDIO_PACKAGE: PackageDepIdent = PackageDepIdent {
        origin: String::from("core"),
        name: String::from("build-tools-hab-studio"),
        version: None,
        release: None,
    };
}

type ArtifactCacheIndex =
    HashMap<String, HashMap<String, BTreeMap<String, HashMap<PackageTarget, BTreeSet<String>>>>>;

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(try_from = "String")]
pub enum PackageTarget {
    AArch64Linux,
    AArch64Darwin,
    X86_64Linux,
    X86_64Windows,
}

impl Display for PackageTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageTarget::AArch64Linux => write!(f, "aarch64-linux"),
            PackageTarget::AArch64Darwin => write!(f, "aarch64-darwin"),
            PackageTarget::X86_64Linux => write!(f, "x86_64-linux"),
            PackageTarget::X86_64Windows => write!(f, "x86_64-windows"),
        }
    }
}

impl TryFrom<&str> for PackageTarget {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "aarch64-linux" => Ok(PackageTarget::AArch64Linux),
            "aarch64-darwin" => Ok(PackageTarget::AArch64Darwin),
            "x86_64-linux" => Ok(PackageTarget::X86_64Linux),
            "x86_64-windows" => Ok(PackageTarget::X86_64Windows),
            _ => Err(anyhow!("Unknown package target '{}'", value)),
        }
    }
}

impl TryFrom<String> for PackageTarget {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageTarget::try_from(value.as_str())
    }
}

const HAB_AUTO_BUILD_EXTRACT_SOURCE_FILES: [(&str, &[u8]); 2] = [
    ("extract.sh", include_bytes!("./scripts/extract.sh")),
    ("cache_index.sh", include_bytes!("./scripts/cache_index.sh")),
];

#[derive(Debug, Deserialize, Serialize)]
struct HabitatAutoBuildConfiguration {
    pub repos: Vec<RepoConfiguration>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RepoConfiguration {
    pub source: PathBuf,
    pub bootstrap_studio_package: Option<PackageDepIdent>,
    pub studio_package: Option<PackageDepIdent>,
    pub native_packages: Option<Vec<String>>,
    pub bootstrap_packages: Option<Vec<String>>,
}

impl HabitatAutoBuildConfiguration {
    pub async fn new(config_path: impl AsRef<Path>) -> Result<HabitatAutoBuildConfiguration> {
        Ok(
            serde_json::from_slice(tokio::fs::read(config_path).await?.as_ref())
                .context("Failed to read hab auto build configuration")?,
        )
    }
}

/// Habitat Auto Build allows you to automatically build multiple packages
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Build a set of packages
    Build(BuildArgs),
    /// Visualize dependencies between a set of packages
    Visualize(VisualizeArgs),
    /// Analyze dependencies between a set of packages
    Analyze(AnalyzeArgs),
}

#[derive(Debug, Args)]
struct AnalyzeArgs {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Visualize reverse dependencies
    #[arg(short, long)]
    reverse_deps: bool,
    /// Remove cycles in dependencies
    #[arg(short = 'n', long)]
    remove_cycles: bool,
    /// List of plans to start analysis
    #[arg(short, long)]
    start_packages: Vec<String>,
    /// List of plans to end analysis
    #[arg(short, long)]
    end_packages: Option<Vec<String>>,
    /// Analysis output file
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct VisualizeArgs {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Visualize reverse dependencies
    #[arg(short, long)]
    reverse_deps: bool,
    /// Remove cycles in dependencies
    #[arg(short = 'n', long)]
    remove_cycles: bool,
    /// List of plans to start analysis
    #[arg(short, long)]
    start_packages: Vec<String>,
    /// List of plans to end analysis
    #[arg(short, long)]
    end_packages: Option<Vec<String>>,
    /// Dependency graph output file
    #[arg(short, long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct BuildArgs {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Unique ID to identify the build
    #[arg(short = 'i', long)]
    session_id: Option<String>,
    /// Maximum number of parallel build workers
    #[arg(short, long)]
    workers: Option<usize>,
    /// List of updated plans
    updated_packages: Vec<String>,
}

struct Repo {
    pub path: PathBuf,
    pub config: RepoConfiguration,
}

impl Repo {
    pub async fn new(config: RepoConfiguration) -> Result<Repo> {
        let path = config.source.canonicalize()?;
        let metadata = tokio::fs::metadata(&path).await.with_context(|| {
            format!(
                "Failed to read file system metadata for '{}'",
                path.display()
            )
        })?;

        if !metadata.is_dir() {
            return Err(anyhow!(
                "Repository path '{}' must point to a directory",
                path.display()
            ));
        }
        Ok(Repo { path, config })
    }
    pub async fn scan(&self) -> Result<Vec<PackageSource>> {
        let mut package_sources = Vec::new();
        let mut next_dirs = VecDeque::new();
        next_dirs.push_back(self.path.clone());
        while !next_dirs.is_empty() {
            let current_dir = next_dirs.pop_front().unwrap();
            match PackageSource::new(current_dir.as_path(), self.path.as_path()).await {
                Ok(package_source) => {
                    debug!("Found package source at {}", current_dir.display());
                    package_sources.push(package_source);
                }
                Err(err) => {
                    trace!(
                        "No package source found at {}: {:#}",
                        current_dir.display(),
                        err
                    );
                    let mut read_dir = tokio::fs::read_dir(current_dir).await?;
                    while let Some(dir) = read_dir.next_entry().await? {
                        let dir_metadata = dir.metadata().await?;
                        if dir_metadata.is_dir() {
                            next_dirs.push_back(dir.path());
                        }
                    }
                }
            };
        }
        Ok(package_sources)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum PackageType {
    Native,
    Bootstrap,
    Standard,
}

impl TryFrom<String> for PackageType {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "native" => Ok(PackageType::Native),
            "bootstrap" => Ok(PackageType::Bootstrap),
            "standard" => Ok(PackageType::Standard),
            _ => Err(anyhow!("Unknown package type: {}", value)),
        }
    }
}

impl From<PackageType> for String {
    fn from(value: PackageType) -> Self {
        match value {
            PackageType::Native => String::from("native"),
            PackageType::Bootstrap => String::from("bootstrap"),
            PackageType::Standard => String::from("standard"),
        }
    }
}

#[derive(Clone)]
struct PackageBuild {
    pub plan: PlanMetadata,
    pub package_type: PackageType,
    pub repo: Arc<Repo>,
}

impl std::fmt::Debug for PackageBuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{:?}",
            match &self.package_type {
                PackageType::Standard => "",
                PackageType::Native => "native:",
                PackageType::Bootstrap => "bootstrap:",
            },
            self.plan
        )
    }
}

impl PackageBuild {
    fn new(repo: Arc<Repo>, plan: PlanMetadata) -> PackageBuild {
        let mut package_type = PackageType::Standard;
        if let Some(native_package_patterns) = repo.config.native_packages.as_ref() {
            for pattern in native_package_patterns.iter() {
                if let Ok(pattern) = glob::Pattern::new(pattern) {
                    if pattern.matches_path(plan.source.strip_prefix(plan.repo.as_path()).unwrap())
                    {
                        package_type = PackageType::Native
                    }
                } else {
                    warn!(
                        "Invalid pattern '{}' for matching native packages in '{}'",
                        pattern,
                        repo.path.display()
                    );
                }
            }
        }
        if let Some(bootstrap_package_patterns) = repo.config.bootstrap_packages.as_ref() {
            for pattern in bootstrap_package_patterns.iter() {
                if let Ok(pattern) = glob::Pattern::new(pattern) {
                    if pattern.matches_path(plan.source.strip_prefix(plan.repo.as_path()).unwrap())
                    {
                        if matches!(package_type, PackageType::Native) {
                            warn!(
                                "Package '{}' matches both bootstrap and native package pattern, considering it as a bootstrap package",
                                plan.ident
                            );
                        }
                        package_type = PackageType::Bootstrap
                    }
                } else {
                    warn!(
                        "Invalid pattern '{}' for matching bootstrap packages in '{}'",
                        pattern,
                        repo.path.display()
                    );
                }
            }
        }
        PackageBuild {
            plan,
            package_type,
            repo,
        }
    }
    async fn is_updated(&self, scripts: &Scripts) -> Result<bool> {
        let last_build = {
            let dep_ident = PackageDepIdent::from(&self.plan.ident);
            if let Ok(Some(artifact)) = dep_ident
                .latest_artifact(self.plan.ident.target, scripts)
                .await
            {
                Some((
                    artifact.clone(),
                    DateTime::<Utc>::from(
                        tokio::fs::metadata(
                            PathBuf::from("/hab")
                                .join("cache")
                                .join("artifacts")
                                .join(artifact.to_string()),
                        )
                        .await?
                        .modified()?,
                    ),
                ))
            } else {
                None
            }
        };
        if let Some((artifact, last_build_timestamp)) = last_build {
            let source_folder = self.source_folder();
            let mut next_entries = VecDeque::new();
            next_entries.push_back(source_folder);
            while !next_entries.is_empty() {
                let current_dir = next_entries.pop_front().unwrap();
                let metadata = tokio::fs::metadata(current_dir.as_path()).await?;
                let last_modified_timestamp = DateTime::<Utc>::from(metadata.modified()?);

                if metadata.is_file() && last_modified_timestamp > last_build_timestamp {
                    debug!("Package {} has a dependency {}[{}] that is modified after the last package build artifact {}[{}], considering it as changed", self.plan.ident, current_dir.display(), last_modified_timestamp, artifact ,last_build_timestamp);
                    return Ok(true);
                }
                if metadata.is_dir() {
                    if last_modified_timestamp > last_build_timestamp {
                        debug!("Package {} has a depedency {}[{:?}] that is modified after the last package build artifact {}[{:?}], considering it as changed", self.plan.ident, current_dir.display(), last_modified_timestamp, artifact ,last_build_timestamp);
                        return Ok(true);
                    }
                    let mut read_dir = tokio::fs::read_dir(current_dir.as_path()).await?;
                    while let Some(dir) = read_dir.next_entry().await? {
                        next_entries.push_back(dir.path());
                    }
                }
            }

            // Check if the build artifact was built after all it's dependent artifacts
            for dep in self.plan.deps.iter().chain(self.plan.build_deps.iter()) {
                if let Ok(Some(dep_artifact)) =
                    dep.latest_artifact(self.plan.ident.target, scripts).await
                {
                    if dep_artifact.release > artifact.release {
                        debug!("Package {} has a dependency build artifact {} that was updated after the last package build artifact {}, considering it as changed", self.plan.ident, dep_artifact,  artifact );
                        return Ok(true);
                    }
                }
            }
            debug!(
                "Package {} has a recent build artifact {}[{}], considering it as unchanged",
                artifact.to_string(),
                self.plan.ident,
                last_build_timestamp
            );
            Ok(false)
        } else {
            debug!(
                "Package {} has no recent build artifact, considering it as changed",
                self.plan.ident
            );
            Ok(true)
        }
    }
    fn repo_build_folder(&self, session_id: &str) -> PathBuf {
        self.plan
            .repo
            .join(".hab-auto-build")
            .join("builds")
            .join(&session_id)
    }
    fn package_build_folder(&self, session_id: &str) -> PathBuf {
        self.plan
            .repo
            .join(".hab-auto-build")
            .join("builds")
            .join(&session_id)
            .join(self.plan.ident.origin.as_str())
            .join(self.plan.ident.name.as_str())
    }
    fn package_studio_build_folder(&self, session_id: &str) -> PathBuf {
        PathBuf::from("/src")
            .join(".hab-auto-build")
            .join("builds")
            .join(&session_id)
            .join(self.plan.ident.origin.as_str())
            .join(self.plan.ident.name.as_str())
    }
    fn build_log_file(&self, session_id: &str) -> PathBuf {
        self.package_build_folder(session_id).join("build.log")
    }
    fn build_success_file(&self, session_id: &str) -> PathBuf {
        self.package_build_folder(session_id).join("BUILD_OK")
    }
    fn build_results_file(&self, session_id: &str) -> PathBuf {
        self.package_build_folder(session_id).join("last_build.env")
    }
    async fn last_build_artifact(&self, session_id: &str) -> Result<PackageArtifactIdent> {
        let metadata = tokio::fs::metadata(self.build_success_file(session_id)).await?;
        if metadata.is_file() {
            let build_results =
                tokio::fs::read_to_string(self.build_results_file(session_id)).await?;
            for line in build_results.lines() {
                if line.starts_with("pkg_artifact=") {
                    return PackageArtifactIdent::parse_with_build(
                        line.strip_prefix("pkg_artifact=").unwrap(),
                        self,
                    );
                }
            }
            Err(anyhow!(
                "The package {:?} does not have a build artifact mentioned in {}",
                self.plan.ident,
                self.build_results_file(session_id).display()
            ))
        } else {
            Err(anyhow!(
                "The package {:?} does not have a successful build",
                self.plan.ident
            ))
        }
    }
    fn source_folder(&self) -> PathBuf {
        self.plan
            .source
            .strip_prefix(self.plan.repo.as_path())
            .unwrap()
            .to_owned()
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PlanMetadata {
    pub path: PathBuf,
    pub source: PathBuf,
    pub repo: PathBuf,
    pub ident: PackageBuildIdent,
    pub deps: Vec<PackageDepIdent>,
    pub build_deps: Vec<PackageDepIdent>,
}

impl std::fmt::Debug for PlanMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{}",
            self.ident.origin, self.ident.name, self.ident.version
        )
    }
}
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PackageArtifactIdent {
    pub origin: String,
    pub name: String,
    pub version: String,
    pub release: String,
    pub target: PackageTarget,
}

impl PackageArtifactIdent {
    fn parse_with_ident(filename: &str, ident: &PackageIdent) -> Result<PackageArtifactIdent> {
        if let Some(target) = filename
            .strip_prefix(
                format!(
                    "{}-{}-{}-{}-",
                    ident.origin, ident.name, ident.version, ident.release
                )
                .as_str(),
            )
            .and_then(|filename| filename.strip_suffix(".hart"))
        {
            Ok(PackageArtifactIdent {
                origin: ident.origin.clone(),
                name: ident.name.clone(),
                version: ident.version.clone(),
                release: ident.release.to_string(),
                target: PackageTarget::try_from(target)?,
            })
        } else {
            Err(anyhow!(
                "Invalid package artifact {} for ident {}",
                filename,
                ident
            ))
        }
    }
    fn parse_with_build(filename: &str, build: &PackageBuild) -> Result<PackageArtifactIdent> {
        if let Some(release) = filename
            .strip_prefix(
                format!(
                    "{}-{}-{}-",
                    build.plan.ident.origin, build.plan.ident.name, build.plan.ident.version
                )
                .as_str(),
            )
            .and_then(|filename| {
                filename.strip_suffix(format!("-{}.hart", build.plan.ident.target).as_str())
            })
        {
            Ok(PackageArtifactIdent {
                origin: build.plan.ident.origin.clone(),
                name: build.plan.ident.name.clone(),
                version: build.plan.ident.version.clone(),
                release: release.to_string(),
                target: build.plan.ident.target.clone(),
            })
        } else {
            Err(anyhow!(
                "Invalid package artifact {} for build {}",
                filename,
                build.plan.ident
            ))
        }
    }
}

impl PartialOrd for PackageArtifactIdent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self.target.eq(&other.target) {
            true => match self.origin.partial_cmp(&other.origin) {
                Some(Ordering::Equal) => match self.name.partial_cmp(&other.name) {
                    Some(Ordering::Equal) => match self.version.partial_cmp(&other.version) {
                        Some(Ordering::Equal) => self.release.partial_cmp(&other.release),
                        Some(Ordering::Greater) => Some(Ordering::Greater),
                        Some(Ordering::Less) => Some(Ordering::Less),
                        _ => None,
                    },
                    ord => None,
                },
                ord => None,
            },
            false => None,
        }
    }
}

impl std::fmt::Display for PackageArtifactIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}-{}-{}-{}.hart",
            self.origin, self.name, self.version, self.release, self.target
        )
    }
}

impl Into<String> for PackageArtifactIdent {
    fn into(self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct PackageIdent {
    pub origin: String,
    pub name: String,
    pub version: String,
    pub release: String,
}

impl PackageIdent {
    pub fn artifact(&self, target: PackageTarget) -> PackageArtifactIdent {
        PackageArtifactIdent {
            origin: self.origin.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            release: self.release.clone(),
            target,
        }
    }
}

impl TryFrom<String> for PackageIdent {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageIdent::try_from(value.as_str())
    }
}

impl TryFrom<&str> for PackageIdent {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut origin = None;
        let mut name = None;
        let mut version = None;
        let mut release = None;
        for (index, part) in value.split('/').enumerate() {
            match index {
                0 => origin = Some(String::from(part)),
                1 => name = Some(String::from(part)),
                2 => version = Some(String::from(part)),
                3 => release = Some(String::from(part)),
                _ => return Err(anyhow!("Invalid package identifier '{}'", value)),
            }
        }
        Ok(PackageIdent {
            origin: origin.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
            name: name.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
            version: version.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
            release: release.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
        })
    }
}

impl From<PackageIdent> for String {
    fn from(value: PackageIdent) -> Self {
        value.to_string()
    }
}

impl std::fmt::Display for PackageIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{}/{}",
            self.origin, self.name, self.version, self.release
        )
    }
}

impl From<PackageArtifactIdent> for PackageIdent {
    fn from(ident: PackageArtifactIdent) -> Self {
        PackageIdent {
            origin: ident.origin,
            name: ident.name,
            version: ident.version,
            release: ident.release,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, Eq)]
pub struct PackageBuildIdent {
    pub target: PackageTarget,
    pub origin: String,
    pub name: String,
    pub version: String,
}

impl PartialOrd for PackageBuildIdent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self.target.eq(&other.target) {
            true => match self.origin.partial_cmp(&other.origin) {
                Some(Ordering::Equal) => match self.name.partial_cmp(&other.name) {
                    Some(Ordering::Equal) => self.version.partial_cmp(&other.version),
                    ord => None,
                },
                ord => None,
            },
            false => None,
        }
    }
}

impl std::fmt::Display for PackageBuildIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.version.is_empty() {
            write!(f, "{}/{}", self.origin, self.name)
        } else {
            write!(f, "{}/{}/{}", self.origin, self.name, self.version)
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct PackageDepIdent {
    pub origin: String,
    pub name: String,
    pub version: Option<String>,
    pub release: Option<String>,
}

impl PackageDepIdent {
    pub fn matches_build(&self, ident: &PackageBuildIdent) -> bool {
        self.origin == ident.origin
            && self.name == ident.name
            && self
                .version
                .as_ref()
                .map_or(true, |version| ident.version == *version)
    }
    pub fn matches_artifact(&self, ident: &PackageArtifactIdent) -> bool {
        self.origin == ident.origin
            && self.name == ident.name
            && self
                .version
                .as_ref()
                .map_or(true, |version| ident.version == *version)
    }
    pub async fn latest_artifact(
        &self,
        target: PackageTarget,
        scripts: &Scripts,
    ) -> Result<Option<PackageArtifactIdent>> {
        let cache_index = scripts.cache_index(&self.origin, &self.name).await.unwrap();
        if let Some(version_index) = cache_index
            .get(&self.origin)
            .and_then(|c| c.get(&self.name))
        {
            if let Some(version) = self.version.as_ref() {
                if let Some(release) = self.release.as_ref() {
                    // Exact match
                    if version_index
                        .get(version)
                        .and_then(|t| t.get(&target))
                        .and_then(|r| r.get(release))
                        .is_some()
                    {
                        Ok(Some(PackageArtifactIdent {
                            origin: self.origin.clone(),
                            name: self.name.clone(),
                            version: version.clone(),
                            release: release.clone(),
                            target,
                        }))
                    } else {
                        Ok(None)
                    }
                } else {
                    // Latest release
                    if let Some(release) = version_index
                        .get(version)
                        .and_then(|t| t.get(&target))
                        .and_then(|r| r.iter().last())
                    {
                        Ok(Some(PackageArtifactIdent {
                            origin: self.origin.clone(),
                            name: self.name.clone(),
                            version: version.clone(),
                            release: release.clone(),
                            target,
                        }))
                    } else {
                        Ok(None)
                    }
                }
            } else {
                // Latest version, latest release
                if let Some((version, release)) = version_index
                    .iter()
                    .last()
                    .and_then(|(version, c)| c.get(&target).map(|releases| (version, releases)))
                    .and_then(|(version, releases)| releases.iter().last().map(|r| (version, r)))
                {
                    Ok(Some(PackageArtifactIdent {
                        origin: self.origin.clone(),
                        name: self.name.clone(),
                        version: version.clone(),
                        release: release.clone(),
                        target,
                    }))
                } else {
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }
}

impl TryFrom<String> for PackageDepIdent {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut origin = None;
        let mut name = None;
        let mut version = None;
        let mut release = None;
        for (index, part) in value.split('/').enumerate() {
            match index {
                0 => origin = Some(String::from(part)),
                1 => name = Some(String::from(part)),
                2 => version = Some(String::from(part)),
                3 => release = Some(String::from(part)),
                _ => return Err(anyhow!("Invalid package identifier '{}'", value)),
            }
        }
        Ok(PackageDepIdent {
            origin: origin.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
            name: name.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
            version,
            release,
        })
    }
}

impl From<PackageDepIdent> for String {
    fn from(value: PackageDepIdent) -> Self {
        value.to_string()
    }
}

impl std::fmt::Display for PackageDepIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.origin)?;
        f.write_str("/")?;
        f.write_str(&self.name)?;
        if let Some(version) = self.version.as_ref() {
            if !version.is_empty() {
                f.write_str("/")?;
                f.write_str(version)?;
            }
        }
        if let Some(release) = self.release.as_ref() {
            if !release.is_empty() {
                f.write_str("/")?;
                f.write_str(release)?;
            }
        }
        Ok(())
    }
}

impl From<&PackageBuildIdent> for PackageDepIdent {
    fn from(ident: &PackageBuildIdent) -> Self {
        PackageDepIdent {
            origin: ident.origin.clone(),
            name: ident.name.clone(),
            version: if ident.version.is_empty() {
                None
            } else {
                Some(ident.version.clone())
            },
            release: None,
        }
    }
}

pub struct PlanSource {
    pub path: PathBuf,
    pub src: PathBuf,
    pub repo: PathBuf,
}

impl PlanSource {
    pub async fn new(
        path: impl AsRef<Path>,
        src: impl AsRef<Path>,
        repo: impl AsRef<Path>,
    ) -> Result<PlanSource> {
        if let Some(file_name) = path.as_ref().file_name() {
            if file_name == PLAN_FILE_NAME.as_os_str() {
                let metadata = tokio::fs::metadata(path.as_ref()).await.with_context(|| {
                    format!(
                        "Failed to read file system metadata for '{}'",
                        path.as_ref().display()
                    )
                })?;
                if !metadata.is_file() {
                    return Err(anyhow!(
                        "Plan source path '{}' must point to a file",
                        path.as_ref().display()
                    ));
                }
                Ok(PlanSource {
                    path: path.as_ref().into(),
                    src: src.as_ref().into(),
                    repo: repo.as_ref().into(),
                })
            } else {
                Err(anyhow!(
                    "Plan source '{}' must point to a 'plan.sh' file",
                    path.as_ref().display()
                ))
            }
        } else {
            Err(anyhow!(
                "Plan source '{}' must point to a 'plan.sh' file",
                path.as_ref().display()
            ))
        }
    }

    pub async fn metadata(&self, target: PackageTarget, script: &Scripts) -> Result<PlanMetadata> {
        script.metadata_extract(target, self).await
    }
}

pub struct PackageSource {
    pub path: PathBuf,
    pub repo: PathBuf,
}

impl PackageSource {
    pub async fn new(path: impl AsRef<Path>, repo: impl AsRef<Path>) -> Result<PackageSource> {
        let metadata = tokio::fs::metadata(path.as_ref()).await.with_context(|| {
            format!(
                "Failed to read file system metadata for package source '{}'",
                path.as_ref().display()
            )
        })?;
        if !metadata.is_dir() {
            return Err(anyhow!(
                "Package source path '{}' must point to a directory",
                path.as_ref().display()
            ));
        }
        let mut plan_found = false;
        for location in PLAN_FILE_LOCATIONS.iter() {
            match PlanSource::new(path.as_ref().join(location), path.as_ref(), repo.as_ref()).await
            {
                Ok(_) => {
                    plan_found = true;
                    break;
                }
                Err(err) => {
                    trace!("No plan found at {}: {:#}", location.display(), err);
                    continue;
                }
            }
        }
        if !plan_found {
            return Err(anyhow!(
                "Folder '{}' does not contain a habitat plan",
                path.as_ref().display()
            ));
        }
        Ok(PackageSource {
            path: path.as_ref().into(),
            repo: repo.as_ref().into(),
        })
    }
    pub async fn metadata(&self, target: PackageTarget, script: &Scripts) -> Result<PlanMetadata> {
        // Search for target specific plan
        let plan_source = PlanSource::new(
            self.path.join(target.to_string()).join("plan.sh"),
            self.path.as_path(),
            self.repo.as_path(),
        )
        .await
        .or(PlanSource::new(
            self.path
                .join("habitat")
                .join(target.to_string())
                .join("plan.sh"),
            self.path.as_path(),
            self.repo.as_path(),
        )
        .await)
        .or(PlanSource::new(
            self.path.join("plan.sh"),
            self.path.as_path(),
            self.repo.as_path(),
        )
        .await)
        .or(PlanSource::new(
            self.path.join("habitat").join("plan.sh"),
            self.path.as_path(),
            self.repo.as_path(),
        )
        .await)?;
        plan_source.metadata(target, script).await
    }
}

pub struct Scripts {
    tmp_dir: TempDir,
    script_paths: HashMap<String, PathBuf>,
}

impl Scripts {
    pub async fn new() -> Result<Scripts> {
        let tmp_dir = TempDir::new("hab-auto-build")?;
        let mut script_paths = HashMap::new();
        for (script_file_name, script_file_data) in HAB_AUTO_BUILD_EXTRACT_SOURCE_FILES {
            let script_path = tmp_dir.path().join(script_file_name);
            File::create(&script_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to create plan build source file '{}'",
                        script_path.display()
                    )
                })?
                .write_all(script_file_data)
                .await
                .with_context(|| {
                    format!(
                        "Failed to write data to plan build source \
                                                    file '{}'",
                        script_path.display()
                    )
                })?;
            script_paths.insert(script_file_name.to_string(), script_path);
        }

        Ok(Scripts {
            tmp_dir,
            script_paths,
        })
    }

    pub async fn cache_index(&self, origin: &str, name: &str) -> Result<ArtifactCacheIndex> {
        let output = tokio::process::Command::new("bash")
            .arg(self.script_paths.get("cache_index.sh").unwrap().as_path())
            .arg(origin)
            .arg(name)
            .output()
            .await?;
        let cache_data = String::from_utf8_lossy(output.stdout.as_slice());
        let mut cache: ArtifactCacheIndex = HashMap::new();
        for line in cache_data.lines() {
            let parts = line.split('=').collect::<Vec<_>>();
            let pkg_ident = PackageIdent::try_from(parts[0])?;
            let pkg_artifact = PackageArtifactIdent::parse_with_ident(parts[1], &pkg_ident)?;
            cache
                .entry(pkg_ident.origin)
                .or_default()
                .entry(pkg_ident.name)
                .or_default()
                .entry(pkg_ident.version)
                .or_default()
                .entry(pkg_artifact.target)
                .or_default()
                .insert(pkg_artifact.release);
        }
        Ok(cache)
    }

    pub async fn metadata_extract(
        &self,
        target: PackageTarget,
        plan: &PlanSource,
    ) -> Result<PlanMetadata> {
        let output = tokio::process::Command::new("bash")
            .arg(self.script_paths.get("extract.sh").unwrap().as_path())
            .arg(plan.path.as_path())
            .arg(plan.src.as_path())
            .arg(plan.repo.as_path())
            .env("BUILD_PKG_TARGET", target.to_string())
            .output()
            .await?;

        serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "Failed to deserialize plan metadata json: {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }
}

async fn dep_graph_build(
    start_package_idents: Vec<PackageDepIdent>,
    end_package_idents: Vec<PackageDepIdent>,
    auto_build_config: HabitatAutoBuildConfiguration,
    detect_updates: bool,
    scripts: Arc<Scripts>,
) -> Result<(
    Graph<PackageBuild, ()>,
    Vec<NodeIndex>,
    Vec<NodeIndex>,
    Vec<NodeIndex>,
)> {
    let mut dep_graph = Graph::new();
    let mut packages = HashMap::new();
    let mut source_package_nodes = Vec::new();
    let mut sink_package_nodes = Vec::new();
    let mut updated_package_nodes = Vec::new();

    for repo_config in auto_build_config.repos {
        info!(
            "Scanning directory '{}' for Habitat plans",
            repo_config.source.display()
        );
        let repo = Arc::new(Repo::new(repo_config).await?);
        let package_sources = repo.scan().await?;
        if package_sources.is_empty() {
            info!("No Habitat plans found in {}", repo.path.display());
            continue;
        }
        let mut bootstrap_studio_package_node = None;
        let mut studio_package_node = None;
        for package_source in package_sources {
            let metadata = package_source
                .metadata(PackageTarget::AArch64Linux, &scripts)
                .await?;
            let build = PackageBuild::new(repo.clone(), metadata.clone());
            let build_is_updated = if detect_updates {
                build.is_updated(&scripts).await?
            } else {
                false
            };
            let node = dep_graph.add_node(build);
            if build_is_updated {
                updated_package_nodes.push(node);
            }
            if repo
                .config
                .bootstrap_studio_package
                .as_ref()
                .map_or(false, |package| package.matches_build(&metadata.ident))
            {
                bootstrap_studio_package_node = Some(node);
            }
            if repo
                .config
                .studio_package
                .as_ref()
                .map_or(false, |package| package.matches_build(&metadata.ident))
            {
                studio_package_node = Some(node);
            }
            if start_package_idents
                .iter()
                .any(|ident| ident.matches_build(&metadata.ident))
            {
                source_package_nodes.push(node);
            }
            if end_package_idents
                .iter()
                .any(|ident| ident.matches_build(&metadata.ident))
            {
                sink_package_nodes.push(node);
            }

            packages.insert(metadata.ident.clone(), (node, metadata.clone()));
        }

        for (ident, (node, metadata)) in packages.iter() {
            match (
                &dep_graph[*node].package_type,
                bootstrap_studio_package_node.as_ref(),
                studio_package_node.as_ref(),
            ) {
                (PackageType::Bootstrap, Some(studio_node), _)
                | (PackageType::Standard, _, Some(studio_node)) => {
                    dep_graph.add_edge(*node, *studio_node, ());
                }
                _ => {}
            }
            for dep in metadata.build_deps.iter().chain(metadata.deps.iter()) {
                let mut dep_package = None;
                for (dep_ident, (dep_node, dep_metadata)) in packages.iter() {
                    if dep.matches_build(dep_ident) {
                        if let Some(dep_version) = dep.version.as_ref() {
                            if &dep_ident.version == dep_version {
                                dep_package = Some((dep_ident, dep_node));
                                break;
                            }
                        } else if let Some((existing_dep_ident, _)) = dep_package {
                            if dep_ident > existing_dep_ident {
                                dep_package = Some((dep_ident, dep_node));
                            }
                        } else {
                            dep_package = Some((dep_ident, dep_node));
                        }
                    }
                }
                if let Some((_, dep_node)) = dep_package {
                    dep_graph.add_edge(*node, *dep_node, ());
                }
            }
        }
    }
    Ok((
        dep_graph,
        source_package_nodes,
        updated_package_nodes,
        sink_package_nodes,
    ))
}

async fn visualize(args: VisualizeArgs) -> Result<()> {
    let scripts = Arc::new(Scripts::new().await?);
    let start_package_idents = args
        .start_packages
        .into_iter()
        .map(|value| PackageDepIdent::try_from(value))
        .collect::<Result<Vec<PackageDepIdent>, _>>()?;
    let end_package_idents = if let Some(end_packages) = args.end_packages {
        end_packages
            .into_iter()
            .map(|value| PackageDepIdent::try_from(value))
            .collect::<Result<Vec<PackageDepIdent>, _>>()?
    } else {
        Vec::new()
    };
    let auto_build_config = HabitatAutoBuildConfiguration::new(
        args.config_path
            .unwrap_or(env::current_dir()?.join("hab-auto-build.json")),
    )
    .await
    .context("Failed to load habitat auto build configuration")?;

    let (dep_graph, start_package_nodes, _, end_package_nodes) = dep_graph_build(
        start_package_idents,
        end_package_idents,
        auto_build_config,
        false,
        scripts,
    )
    .await?;

    let output = {
        let build_graph = NodeFiltered::from_fn(&dep_graph, |node| {
            let mut include = true;
            for start_package_node in start_package_nodes.iter() {
                if args.reverse_deps {
                    if !algo::has_path_connecting(&dep_graph, node, *start_package_node, None) {
                        include = false;
                        break;
                    }
                } else {
                    if !algo::has_path_connecting(&dep_graph, *start_package_node, node, None) {
                        include = false;
                        break;
                    }
                }
            }
            for end_package_node in end_package_nodes.iter() {
                if node == *end_package_node {
                    break;
                }
                if args.reverse_deps {
                    if algo::has_path_connecting(&dep_graph, node, *end_package_node, None) {
                        include = false;
                        break;
                    }
                } else {
                    if algo::has_path_connecting(&dep_graph, *end_package_node, node, None) {
                        include = false;
                        break;
                    }
                }
            }
            include
        });

        if args.remove_cycles {
            let mut acyclic_deps = Graph::new();
            let mut node_map = HashMap::new();
            for (node_index, node) in build_graph.node_references() {
                let mapped_node = acyclic_deps.add_node((*node).clone());
                node_map.insert(node_index, mapped_node);
            }
            for edge in build_graph.edge_references() {
                if let Some((from, to)) = dep_graph.edge_endpoints(edge.id()) {
                    acyclic_deps.add_edge(
                        *node_map.get(&from).unwrap(),
                        *node_map.get(&to).unwrap(),
                        (),
                    );
                };
            }
            acyclic_deps.reverse();
            let fas: Vec<EdgeIndex> = greedy_feedback_arc_set(&acyclic_deps)
                .map(|e| e.id())
                .collect();

            // Remove edges in feedback arc set from original graph
            for edge_id in fas {
                if let Some((from, to)) = acyclic_deps.edge_endpoints(edge_id) {
                    info!(
                        "Removing cyclic dependency {} -> {}",
                        acyclic_deps[from].plan.ident, acyclic_deps[to].plan.ident
                    );
                }
                acyclic_deps.remove_edge(edge_id);
            }
            acyclic_deps.reverse();
            format!(
                "{:?}",
                Dot::with_config(&acyclic_deps, &[Config::EdgeNoLabel])
            )
        } else {
            format!(
                "{:?}",
                Dot::with_config(&build_graph, &[Config::EdgeNoLabel])
            )
        }
    };
    let output = output.replace("digraph {", "digraph { rankdir=LR; node [shape=rectangle, color=blue, fillcolor=lightskyblue, style=filled ]; edge [color=darkgreen];");
    let mut output_file = tokio::fs::File::create(args.output).await?;
    output_file.write_all(output.as_bytes()).await?;
    output_file.shutdown().await?;

    Ok(())
}

async fn analyze(args: AnalyzeArgs) -> Result<()> {
    let scripts = Arc::new(Scripts::new().await?);
    let start_package_idents = args
        .start_packages
        .into_iter()
        .map(|value| PackageDepIdent::try_from(value))
        .collect::<Result<Vec<PackageDepIdent>, _>>()?;

    let end_package_idents = if let Some(end_packages) = args.end_packages {
        end_packages
            .into_iter()
            .map(|value| PackageDepIdent::try_from(value))
            .collect::<Result<Vec<PackageDepIdent>, _>>()?
    } else {
        Vec::new()
    };
    let auto_build_config = HabitatAutoBuildConfiguration::new(
        args.config_path
            .unwrap_or(env::current_dir()?.join("hab-auto-build.json")),
    )
    .await
    .context("Failed to load habitat auto build configuration")?;

    let (dep_graph, start_package_nodes, _, end_package_nodes) = dep_graph_build(
        start_package_idents,
        end_package_idents,
        auto_build_config,
        false,
        scripts,
    )
    .await?;

    let packages = {
        let build_graph = NodeFiltered::from_fn(&dep_graph, |node| {
            let mut include = true;
            for start_package_node in start_package_nodes.iter() {
                if args.reverse_deps {
                    if !algo::has_path_connecting(&dep_graph, node, *start_package_node, None) {
                        include = false;
                        break;
                    }
                } else {
                    if !algo::has_path_connecting(&dep_graph, *start_package_node, node, None) {
                        include = false;
                        break;
                    }
                }
            }
            for end_package_node in end_package_nodes.iter() {
                if node == *end_package_node {
                    break;
                }
                if args.reverse_deps {
                    if algo::has_path_connecting(&dep_graph, node, *end_package_node, None) {
                        include = false;
                        break;
                    }
                } else {
                    if algo::has_path_connecting(&dep_graph, *end_package_node, node, None) {
                        include = false;
                        break;
                    }
                }
            }
            include
        });

        if args.remove_cycles {
            let mut acyclic_deps = Graph::new();
            let mut node_map = HashMap::new();
            for (node_index, node) in build_graph.node_references() {
                let mapped_node = acyclic_deps.add_node((*node).clone());
                node_map.insert(node_index, mapped_node);
            }
            for edge in build_graph.edge_references() {
                if let Some((from, to)) = dep_graph.edge_endpoints(edge.id()) {
                    acyclic_deps.add_edge(
                        *node_map.get(&from).unwrap(),
                        *node_map.get(&to).unwrap(),
                        (),
                    );
                };
            }
            acyclic_deps.reverse();
            let fas: Vec<EdgeIndex> = greedy_feedback_arc_set(&acyclic_deps)
                .map(|e| e.id())
                .collect();

            // Remove edges in feedback arc set from original graph
            for edge_id in fas {
                if let Some((from, to)) = acyclic_deps.edge_endpoints(edge_id) {
                    info!(
                        "Removing cyclic dependency {} -> {}",
                        acyclic_deps[from].plan.ident, acyclic_deps[to].plan.ident
                    );
                }
                acyclic_deps.remove_edge(edge_id);
            }
            acyclic_deps.reverse();
            let mut packages = Vec::new();
            for (_, node) in acyclic_deps.node_references() {
                packages.push(format!("{}", node.plan.ident))
            }
            packages
        } else {
            let mut packages = Vec::new();
            for (_, node) in build_graph.node_references() {
                packages.push(format!("{}", node.plan.ident))
            }
            packages
        }
    };

    if let Some(output_file_path) = args.output {
        let mut output_file = tokio::fs::File::create(output_file_path).await?;
        output_file
            .write_all(packages.join("\n").as_bytes())
            .await?;
        output_file.shutdown().await?;
    } else {
        for package in packages {
            println!("{}", package);
        }
    }
    Ok(())
}

async fn build(args: BuildArgs) -> Result<()> {
    let scripts = Arc::new(Scripts::new().await?);
    let manually_updated_package_idents = args
        .updated_packages
        .into_iter()
        .map(|value| PackageDepIdent::try_from(value))
        .collect::<Result<Vec<PackageDepIdent>, _>>()?;

    let auto_build_config = HabitatAutoBuildConfiguration::new(
        args.config_path
            .unwrap_or(env::current_dir()?.join("hab-auto-build.json")),
    )
    .await
    .context("Failed to load habitat auto build configuration")?;

    let (dep_graph, manually_updated_package_nodes, updated_package_nodes, _) = dep_graph_build(
        manually_updated_package_idents,
        Vec::new(),
        auto_build_config,
        true,
        scripts.clone(),
    )
    .await?;

    for updated_package_node in updated_package_nodes.iter() {
        info!(
            "Detected changes in {} at {}",
            dep_graph[*updated_package_node].plan.ident,
            dep_graph[*updated_package_node].plan.source.display()
        );
    }

    let build_graph = NodeFiltered::from_fn(&dep_graph, |node| {
        let mut is_affected = false;
        for updated_package_node in updated_package_nodes.iter() {
            if !manually_updated_package_nodes.is_empty() {
                let mut should_include = false;
                for manually_updated_package_node in manually_updated_package_nodes.iter() {
                    if algo::has_path_connecting(
                        &dep_graph,
                        *updated_package_node,
                        *manually_updated_package_node,
                        None,
                    ) {
                        should_include = true;
                        break;
                    }
                }
                if !should_include {
                    continue;
                }
            }
            if algo::has_path_connecting(&dep_graph, node, *updated_package_node, None) {
                is_affected = true;
                break;
            }
        }
        is_affected
    });

    let mut build_order =
        algo::toposort(&build_graph, None).map_err(|err| anyhow!("Cycle detected: {:?}", err))?;
    build_order.reverse();
    let build_order = Arc::new(build_order);

    debug!(
        "Build order: {:?}",
        build_order
            .iter()
            .map(|node| &dep_graph[*node])
            .collect::<Vec<&PackageBuild>>()
    );

    let mut scheduler = Scheduler::new(
        args.session_id.unwrap_or_else(|| {
            let mut generator = Generator::with_naming(Name::Numbered);
            generator.next().unwrap()
        }),
        build_order.clone(),
        Arc::new(dep_graph),
        scripts,
    );

    info!(
        "Beginning build {}, {} packages to be built",
        scheduler.session_id,
        build_order.len()
    );

    for _ in 0..args.workers.unwrap_or(1) {
        scheduler.thread_start();
    }

    scheduler.await_completion().await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // a builder for `FmtSubscriber`.
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Build(args) => build(args).await,
        Commands::Visualize(args) => visualize(args).await,
        Commands::Analyze(args) => analyze(args).await,
    }
}

struct Scheduler {
    session_id: String,
    scripts: Arc<Scripts>,
    built_packages: Arc<DashSet<NodeIndex>>,
    pending_packages: Arc<DashSet<NodeIndex>>,
    build_order: Arc<Vec<NodeIndex>>,
    dep_graph: Arc<Graph<PackageBuild, ()>>,
    handles: FuturesUnordered<JoinHandle<Result<(), anyhow::Error>>>,
}

struct PackageBuilder<'a> {
    session_id: String,
    worker_index: usize,
    build: &'a PackageBuild,
}

impl<'a> PackageBuilder<'a> {
    fn new(session_id: &str, worker_index: usize, build: &'a PackageBuild) -> PackageBuilder<'a> {
        PackageBuilder {
            session_id: session_id.to_owned(),
            worker_index,
            build,
        }
    }
    async fn build(
        self,
        deps_in_current_build: Vec<&PackageBuild>,
        scripts: Arc<Scripts>,
    ) -> Result<()> {
        let PackageBuilder {
            session_id,
            worker_index,
            build,
        } = self;
        info!(
            worker = worker_index,
            "Building {:?} with {}",
            build,
            build.plan.path.display()
        );

        tokio::fs::create_dir_all(&build.package_build_folder(&session_id))
            .await
            .with_context(|| {
                format!(
                    "Failed to create build folder '{}' for package '{:?}'",
                    build.package_build_folder(&session_id).display(),
                    build.plan
                )
            })?;

        // if let Ok(true) = tokio::fs::metadata(build.build_success_file(&session_id).as_path())
        //     .await
        //     .map(|metadata| metadata.is_file() )
        // {
        //     info!("Package {:?} already built", build.plan);
        //     return Ok(());
        // }
        let mut build_log_file = File::create(build.build_log_file(&session_id))
            .await
            .context(format!(
                "Failed to create build log file for package '{:?}'",
                build.plan
            ))?;
        let repo = build.plan.repo.as_path();
        let source = build.plan.source.strip_prefix(repo)?;

        let mut child = match build.package_type {
            PackageType::Native => {
                info!(
                    "Building native package {} in {}, view log at {}",
                    source.display(),
                    repo.display(),
                    build.build_log_file(&session_id).display()
                );
                tokio::process::Command::new("hab")
                    .arg("pkg")
                    .arg("build")
                    .arg("-N")
                    .arg(build.source_folder())
                    .env("HAB_FEAT_NATIVE_PACKAGE_SUPPORT", "1")
                    .env("HAB_OUTPUT_PATH", build.package_build_folder(&session_id))
                    .current_dir(build.repo.path.as_path())
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("Failed to invoke hab build command")
            }
            PackageType::Bootstrap => {
                let mut pkg_deps = Vec::new();
                for dep in build.plan.deps.iter().chain(build.plan.build_deps.iter()) {
                    let mut resolved_dep = None;
                    for dep_in_current_build in deps_in_current_build.iter() {
                        if !dep.matches_build(&dep_in_current_build.plan.ident) {
                            continue;
                        }
                        if let Ok(artifact) =
                            dep_in_current_build.last_build_artifact(&session_id).await
                        {
                            resolved_dep = Some(
                                PathBuf::from("/hab")
                                    .join("cache")
                                    .join("artifacts")
                                    .join(artifact.to_string()),
                            );
                            break;
                        }
                    }
                    if resolved_dep.is_none() {
                        if let Ok(Some(artifact)) =
                            dep.latest_artifact(build.plan.ident.target, &scripts).await
                        {
                            resolved_dep = Some(
                                PathBuf::from("/hab")
                                    .join("cache")
                                    .join("artifacts")
                                    .join(artifact.to_string()),
                            );
                        } else {
                            warn!(
                                "Failed to find local build artifact for {}, required by {}",
                                dep, build.plan.ident
                            );
                        }
                    }
                    if let Some(resolved_dep) = resolved_dep {
                        pkg_deps.push(format!("{}", resolved_dep.display()))
                    }
                }
                info!(
                    "Building package {} in {} with bootstrap studio, view log at {}",
                    source.display(),
                    repo.display(),
                    build.build_log_file(&session_id).display()
                );
                tokio::process::Command::new("sudo")
                    .arg("-E")
                    .arg("hab")
                    .arg("pkg")
                    .arg("exec")
                    .arg(
                        build
                            .repo
                            .config
                            .bootstrap_studio_package
                            .as_ref()
                            .unwrap_or(&HAB_DEFAULT_BOOTSTRAP_STUDIO_PACKAGE)
                            .to_string(),
                    )
                    .arg("hab-studio")
                    .arg("-t")
                    .arg("bootstrap")
                    .arg("-r")
                    .arg(PathBuf::from("/hab").join("studios").join(format!(
                        "{}-{}-{}",
                        session_id, build.plan.ident.origin, build.plan.ident.name
                    )))
                    .arg("build")
                    .arg(source)
                    .env("HAB_ORIGIN", build.plan.ident.origin.as_str())
                    .env("HAB_LICENSE", "accept-no-persist")
                    .env("HAB_PKG_DEPS", pkg_deps.join(":"))
                    .env("HAB_STUDIO_SECRET_STUDIO_ENTER", "1")
                    .env(
                        "HAB_STUDIO_SECRET_HAB_OUTPUT_PATH",
                        build.package_studio_build_folder(&session_id),
                    )
                    .current_dir(repo)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("Failed to invoke hab build command")
            }
            PackageType::Standard => {
                info!(
                    "Building package {} in {} with standard studio, view log at {}",
                    source.display(),
                    repo.display(),
                    build.build_log_file(&session_id).display()
                );
                tokio::process::Command::new("hab")
                    .arg("pkg")
                    .arg("build")
                    .arg(source)
                    .env(
                        "HAB_STUDIO_SECRET_OUTPUT_PATH",
                        build.package_studio_build_folder(&session_id),
                    )
                    .current_dir(repo)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("Failed to invoke hab build command")
            }
        };

        let stdout = child
            .stdout
            .take()
            .expect("child did not have a handle to stdout");
        let stderr = child
            .stderr
            .take()
            .expect("child did not have a handle to stderr");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        loop {
            tokio::select! {
                result = stdout_reader.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            build_log_file.write_all(line.as_bytes()).await?;
                            build_log_file.write_all(b"\n").await?;
                        },
                        Ok(None) => continue,
                        Err(err) => return Err(anyhow!("Failed to write build process output from stdout: {:?}", err)),
                    }
                }
                result = stderr_reader.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            build_log_file.write_all(line.as_bytes()).await?;
                            build_log_file.write_all(b"\n").await?;
                        }
                        Ok(None) => continue,
                        Err(err) => return Err(anyhow!("Failed to write build process output from stderr: {:?}", err)),
                    }
                }
                result = child.wait() => {
                    build_log_file.shutdown().await?;
                    match result {
                        Ok(exit_code) => {
                            if exit_code.success() {
                                let mut success_file = File::create(build.build_success_file(&session_id)).await.context(format!(
                                    "Failed to create build success file for package '{:?}'",
                                    build.plan
                                ))?;
                                success_file.shutdown().await?;
                                info!(
                                    worker = worker_index,
                                    "Built {:?}", build.plan
                                );
                                return Ok(())
                            } else {
                                error!(worker = worker_index, "Failed to build {:?}, build process exited with {}, please the build log for errors: {}", build.plan, exit_code, build.build_log_file(&session_id).display());
                                return Err(anyhow!("Failed to build {:?}",  build.plan));
                            }
                        }
                        Err(err) => return Err(anyhow!("Failed to wait for build process to exit: {:?}", err)),
                    }
                }
            };
        }
    }
}

pub enum NextPackageBuild {
    Ready(NodeIndex),
    Waiting,
    Done,
}

impl Scheduler {
    pub fn new(
        session_id: String,
        build_order: Arc<Vec<NodeIndex>>,
        dep_graph: Arc<Graph<PackageBuild, ()>>,
        scripts: Arc<Scripts>,
    ) -> Scheduler {
        Scheduler {
            session_id,
            scripts,
            built_packages: Arc::new(DashSet::new()),
            pending_packages: Arc::new(DashSet::new()),
            build_order,
            dep_graph,
            handles: FuturesUnordered::new(),
        }
    }
    fn mark_complete(built_packages: Arc<DashSet<NodeIndex>>, package_index: NodeIndex) {
        built_packages.insert(package_index);
    }
    fn next(
        built_packages: Arc<DashSet<NodeIndex>>,
        pending_packages: Arc<DashSet<NodeIndex>>,
        build_order: Arc<Vec<NodeIndex>>,
        dep_graph: Arc<Graph<PackageBuild, ()>>,
    ) -> NextPackageBuild {
        for package in build_order.iter() {
            if built_packages.contains(package) {
                continue;
            }
            let deps_affected = dep_graph
                .neighbors_directed(*package, Direction::Outgoing)
                .filter(|node| build_order.contains(node))
                .count();
            let deps_built = dep_graph
                .neighbors_directed(*package, Direction::Outgoing)
                .filter(|node| built_packages.contains(node))
                .count();
            if deps_built == deps_affected {
                if pending_packages.insert(*package) {
                    return NextPackageBuild::Ready(*package);
                } else {
                    continue;
                }
            }
        }
        if built_packages.len() == build_order.len() {
            NextPackageBuild::Done
        } else {
            NextPackageBuild::Waiting
        }
    }

    pub fn thread_start(&self) {
        let built_packages = self.built_packages.clone();
        let scripts = self.scripts.clone();
        let pending_packages = self.pending_packages.clone();
        let build_order = self.build_order.clone();
        let dep_graph = self.dep_graph.clone();
        let worker_index = self.handles.len() + 1;
        let session_id = self.session_id.clone();
        let handle = tokio::spawn(async move {
            loop {
                match Scheduler::next(
                    built_packages.clone(),
                    pending_packages.clone(),
                    build_order.clone(),
                    dep_graph.clone(),
                ) {
                    NextPackageBuild::Ready(package_index) => {
                        let build = &dep_graph[package_index];
                        let builder = PackageBuilder::new(&session_id, worker_index, build);
                        let build_deps = dep_graph
                            .neighbors_directed(package_index, Direction::Outgoing)
                            .into_iter()
                            .map(|dep_index| &dep_graph[dep_index])
                            .collect::<Vec<_>>();
                        builder.build(build_deps, scripts.clone()).await?;
                        Scheduler::mark_complete(built_packages.clone(), package_index);
                    }
                    NextPackageBuild::Waiting => {
                        debug!(worker = worker_index, "Waiting for build");
                        tokio::time::sleep(Duration::from_secs(1)).await
                    }
                    NextPackageBuild::Done => break,
                }
            }
            Ok(())
        });
        self.handles.push(handle);
    }

    pub async fn await_completion(&mut self) -> Result<()> {
        while let Some(result) = self.handles.next().await {
            result.context("Build thread failed")??
        }
        Ok(())
    }
}
