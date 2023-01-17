mod server;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use core::cmp::Ordering;
use dashmap::DashSet;
use futures::{stream::FuturesUnordered, StreamExt};
use inquire::{Confirm, MultiSelect};
use names::{Generator, Name};
use petgraph::{
    algo::{self, greedy_feedback_arc_set},
    dot::{Config, Dot},
    stable_graph::{EdgeIndex, NodeIndex},
    visit::{EdgeRef, IntoNodeReferences, NodeFiltered},
    Direction, Graph,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    env,
    ffi::OsString,
    fmt::Display,
    ops::Deref,
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
        let path = PathBuf::from("/hab");
        path.join("pkgs")
    };
    static ref PLAN_FILE_NAME: OsString =  OsString::from("plan.sh");
    static ref HAB_DEFAULT_BOOTSTRAP_STUDIO_PACKAGE: PackageDepIdent = PackageDepIdent {
        origin: String::from("core"),
        name: String::from("build-tools-hab-studio"),
        version: None,
        release: None,
    };
    static ref HAB_DEFAULT_STUDIO_PACKAGE: PackageDepIdent = PackageDepIdent {
        origin: String::from("core"),
        name: String::from("hab-studio"),
        version: None,
        release: None,
    };
}

type ArtifactCacheIndex =
    HashMap<String, HashMap<String, BTreeMap<String, HashMap<PackageTarget, BTreeSet<String>>>>>;

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub enum PackageTarget {
    AArch64Linux,
    AArch64Darwin,
    X86_64Linux,
    X86_64Darwin,
    X86_64Windows,
}

impl Default for PackageTarget {
    fn default() -> Self {
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            PackageTarget::AArch64Linux
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            PackageTarget::X86_64Linux
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            PackageTarget::X86_64Windows
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            PackageTarget::X86_64Darwin
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            PackageTarget::AArch64Darwin
        } else {
            panic!("Target platform does not support habitat packages")
        }
    }
}

impl Display for PackageTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageTarget::AArch64Linux => write!(f, "aarch64-linux"),
            PackageTarget::AArch64Darwin => write!(f, "aarch64-darwin"),
            PackageTarget::X86_64Linux => write!(f, "x86_64-linux"),
            PackageTarget::X86_64Darwin => write!(f, "x86_64-darwin"),
            PackageTarget::X86_64Windows => write!(f, "x86_64-windows"),
        }
    }
}

impl From<PackageTarget> for String {
    fn from(value: PackageTarget) -> Self {
        value.to_string()
    }
}

impl TryFrom<&str> for PackageTarget {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "aarch64-linux" => Ok(PackageTarget::AArch64Linux),
            "aarch64-darwin" => Ok(PackageTarget::AArch64Darwin),
            "x86_64-linux" => Ok(PackageTarget::X86_64Linux),
            "x86_64-darwin" => Ok(PackageTarget::X86_64Darwin),
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
    pub bootstrap_studio_package: Option<PackageDepIdent>,
    pub studio_package: Option<PackageDepIdent>,
    pub repos: Vec<RepoConfiguration>,
    #[serde(skip)]
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RepoConfiguration {
    pub source: PathBuf,
    pub native_packages: Option<Vec<String>>,
    pub ignored_packages: Option<Vec<String>>,
}

impl HabitatAutoBuildConfiguration {
    pub async fn new(config_path: impl AsRef<Path>) -> Result<HabitatAutoBuildConfiguration> {
        let mut config: HabitatAutoBuildConfiguration =
            serde_json::from_slice(tokio::fs::read(config_path.as_ref()).await?.as_ref())
                .context("Failed to read hab auto build configuration")?;
        config.config_path = config_path.as_ref().to_path_buf();
        Ok(config)
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
    /// Start a server to interactively explore packages
    Server(ServerArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DependencyAnalysis {
    Build,
    Runtime,
    Reverse,
}

#[derive(Debug, Args)]
struct ServerArgs {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// HTTP port to listen on
    #[arg(short, long)]
    port: u16,
}

#[derive(Debug, Args)]
struct AnalyzeArgs {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Type of dependencies to analyze
    #[arg(value_enum, short = 't', long)]
    analysis_type: DependencyAnalysis,
    /// List of packages to analyze
    #[arg(short, long)]
    packages: Vec<String>,
    /// Analysis output file
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct VisualizeArgs {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Type of dependencies to visualize
    #[arg(value_enum, short = 't', long)]
    analysis_type: DependencyAnalysis,
    /// List of packages to visualize
    #[arg(short, long)]
    packages: Vec<String>,
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

#[derive(Debug, Serialize, Deserialize, Default)]
struct PackageSkipList {
    pub updated_at: DateTime<Utc>,
    pub packages: Vec<PackageBuildIdent>,
}

impl PackageSkipList {
    pub async fn new(skipped_packages: impl AsRef<Path>) -> Result<PackageSkipList> {
        Ok(serde_json::from_str(
            &tokio::fs::read_to_string(&skipped_packages).await?,
        )?)
    }
}

struct Repo {
    pub path: PathBuf,
    pub config: RepoConfiguration,
}

impl Repo {
    pub async fn new(config: RepoConfiguration, config_path: impl AsRef<Path>) -> Result<Repo> {
        debug!(
            "Loading Habitat Auto Build configuration at {}",
            config_path.as_ref().canonicalize()?.display()
        );
        let path = if config.source.is_absolute() {
            config.source.canonicalize()?
        } else {
            config_path
                .as_ref()
                .parent()
                .unwrap()
                .join(config.source.as_path())
                .canonicalize()?
        };
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
            if let Some(ignored_package_patterns) = self.config.ignored_packages.as_ref() {
                let mut skip_folder = false;
                for pattern in ignored_package_patterns.iter() {
                    if let Ok(pattern) = glob::Pattern::new(pattern) {
                        if pattern
                            .matches_path(current_dir.strip_prefix(self.path.as_path()).unwrap())
                        {
                            debug!("Skipping folder {}", current_dir.display());
                            skip_folder = true;
                            break;
                        }
                    }
                }
                if skip_folder {
                    continue;
                }
            }
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
    Standard,
}

impl TryFrom<String> for PackageType {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "native" => Ok(PackageType::Native),
            "standard" => Ok(PackageType::Standard),
            _ => Err(anyhow!("Unknown package type: {}", value)),
        }
    }
}

impl From<PackageType> for String {
    fn from(value: PackageType) -> Self {
        match value {
            PackageType::Native => String::from("native"),
            PackageType::Standard => String::from("standard"),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum PackageStudioType {
    Native,
    Bootstrap,
    Standard,
}

impl TryFrom<String> for PackageStudioType {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "native" => Ok(PackageStudioType::Native),
            "bootstrap" => Ok(PackageStudioType::Bootstrap),
            "standard" => Ok(PackageStudioType::Standard),
            _ => Err(anyhow!("Unknown package type: {}", value)),
        }
    }
}

impl From<PackageStudioType> for String {
    fn from(value: PackageStudioType) -> Self {
        match value {
            PackageStudioType::Native => String::from("native"),
            PackageStudioType::Bootstrap => String::from("bootstrap"),
            PackageStudioType::Standard => String::from("standard"),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct PackageBuild {
    pub plan: PlanMetadata,
    pub package_type: PackageType,
    pub studio_type: Option<PackageStudioType>,
    #[serde(skip)]
    repo: Arc<Repo>,
}

impl std::fmt::Debug for PackageBuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{:?}",
            match &self.package_type {
                PackageType::Standard => "",
                PackageType::Native => "native:",
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
        PackageBuild {
            plan,
            package_type,
            studio_type: None,
            repo,
        }
    }
    async fn is_updated(
        &self,
        skip_list: Option<&PackageSkipList>,
        scripts: &Scripts,
    ) -> Result<Option<UpdateCause>> {
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
        let skip_timestamp = skip_list.and_then(|skip_list| {
            if skip_list.packages.contains(&self.plan.ident) {
                Some(skip_list.updated_at)
            } else {
                None
            }
        });

        if let Some((artifact, last_build_timestamp)) = last_build {
            let cutoff_timestamp = if let Some(skip_timestamp) = skip_timestamp {
                skip_timestamp.max(last_build_timestamp)
            } else {
                last_build_timestamp
            };
            let mut update_cause = None;
            let source_folder = self.source_folder();
            let mut next_entries = VecDeque::new();
            next_entries.push_back(source_folder);
            while !next_entries.is_empty() {
                let current_dir = next_entries.pop_front().unwrap();
                let metadata = tokio::fs::metadata(current_dir.as_path()).await?;
                let last_modified_timestamp = DateTime::<Utc>::from(metadata.modified()?);

                if last_modified_timestamp > cutoff_timestamp {
                    if let Some(skip_timestamp) = skip_timestamp {
                        debug!("Package {} has a dependency {} [{}] that is modified after the last package build artifact {} [{}] and skip list timestamp [{}]", self.plan.ident, current_dir.display(), last_modified_timestamp, artifact ,last_build_timestamp, skip_timestamp);
                    } else {
                        debug!("Package {} has a dependency {} [{}] that is modified after the last package build artifact {} [{}]", self.plan.ident, current_dir.display(), last_modified_timestamp, artifact ,last_build_timestamp);
                    }
                    update_cause = Some(UpdateCause::UpdatedSource);
                    break;
                }

                if metadata.is_dir() {
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
                        update_cause = Some(UpdateCause::UpdatedDependency);
                        break;
                    }
                }
            }
            debug!(
                "Package {} has a recent build artifact {} [{}], considering it as unchanged",
                artifact.to_string(),
                self.plan.ident,
                last_build_timestamp
            );
            Ok(update_cause)
        } else {
            debug!(
                "Package {} has no recent build artifact, considering it as changed",
                self.plan.ident
            );
            Ok(Some(UpdateCause::NoArtifact))
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
                    _ord => None,
                },
                _ord => None,
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
                    _ord => None,
                },
                _ord => None,
            },
            false => None,
        }
    }
}

impl std::fmt::Display for PackageBuildIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.version.is_empty() {
            write!(f, "{}/{}/DYNAMIC", self.origin, self.name)
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

impl TryFrom<&str> for PackageDepIdent {
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
        Ok(PackageDepIdent {
            origin: origin.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
            name: name.ok_or_else(|| anyhow!("Invalid package identifier '{}'", value))?,
            version,
            release,
        })
    }
}

impl TryFrom<String> for PackageDepIdent {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageDepIdent::try_from(value.as_str())
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum DependencyType {
    Runtime,
    Build,
    Studio,
}

#[derive(Debug, Clone)]
pub struct PackageDependencyGraph(Graph<PackageBuild, DependencyType>);

impl PackageDependencyGraph {
    pub fn new() -> PackageDependencyGraph {
        PackageDependencyGraph(Graph::new())
    }
    pub fn package(&self, node: &PackageNode) -> &PackageBuild {
        &self.0[**node]
    }
    pub fn package_mut(&mut self, node: &PackageNode) -> &mut PackageBuild {
        &mut self.0[**node]
    }
    pub fn add_package(&mut self, package_build: PackageBuild) -> PackageNode {
        PackageNode(self.0.add_node(package_build))
    }
    pub fn add_runtime_dependency(&mut self, source: PackageNode, target: PackageNode) {
        self.0.add_edge(source.0, target.0, DependencyType::Runtime);
    }
    pub fn add_build_dependency(&mut self, source: PackageNode, target: PackageNode) {
        self.0.add_edge(source.0, target.0, DependencyType::Build);
    }
    pub fn add_studio_dependency(&mut self, source: PackageNode, target: PackageNode) {
        self.0.add_edge(source.0, target.0, DependencyType::Studio);
    }
    pub fn remove_dependency(&mut self, edge_index: EdgeIndex) {
        self.0.remove_edge(edge_index).unwrap();
    }
    pub fn into_inner(self) -> Graph<PackageBuild, DependencyType> {
        self.0
    }
}

impl Deref for PackageDependencyGraph {
    type Target = Graph<PackageBuild, DependencyType>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PackageNode(NodeIndex);

impl PackageNode {
    pub fn is_dependency_of(&self, graph: &PackageDependencyGraph, other: &PackageNode) -> bool {
        algo::has_path_connecting(&graph.0, other.0, self.0, None)
    }
    pub fn is_reverse_dependency_of(
        &self,
        graph: &PackageDependencyGraph,
        other: &PackageNode,
    ) -> bool {
        algo::has_path_connecting(&graph.0, self.0, other.0, None)
    }
}

impl Deref for PackageNode {
    type Target = NodeIndex;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct PackageNodeUpdate {
    package: PackageNode,
    cause: UpdateCause,
}

#[derive(Debug)]
pub enum UpdateCause {
    UpdatedSource,
    UpdatedDependency,
    NoArtifact,
}

impl Display for UpdateCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateCause::UpdatedSource => write!(f, "updated source"),
            UpdateCause::UpdatedDependency => write!(f, "updated dependency"),
            UpdateCause::NoArtifact => write!(f, "no build artifact"),
        }
    }
}

pub struct StudioPackages {
    bootstrap_studio: Option<PackageNode>,
    studio: Option<PackageNode>,
}

async fn dep_graph_build(
    start_package_idents: Vec<PackageDepIdent>,
    auto_build_config: &HabitatAutoBuildConfiguration,
    detect_updates: bool,
    skip_list: Option<&PackageSkipList>,
    scripts: Arc<Scripts>,
) -> Result<(
    PackageDependencyGraph,
    Vec<PackageNode>,
    Vec<PackageNodeUpdate>,
    StudioPackages,
)> {
    let mut dep_graph = PackageDependencyGraph::new();
    let mut packages = HashMap::new();
    let mut source_package_nodes = Vec::new();
    let mut updated_package_nodes = Vec::new();

    let mut studio_packages = StudioPackages {
        bootstrap_studio: None,
        studio: None,
    };

    for repo_config in auto_build_config.repos.iter() {
        let repo = Arc::new(
            Repo::new(
                (*repo_config).clone(),
                auto_build_config.config_path.as_path(),
            )
            .await?,
        );
        info!(
            "Scanning directory '{}' for Habitat plans",
            repo.path.display()
        );
        let package_sources = repo.scan().await?;
        if package_sources.is_empty() {
            info!("No Habitat plans found in {}", repo.path.display());
            continue;
        }

        for package_source in package_sources {
            let metadata = package_source
                .metadata(PackageTarget::default(), &scripts)
                .await?;
            let build = PackageBuild::new(repo.clone(), metadata.clone());
            let build_is_updated = if detect_updates {
                build.is_updated(skip_list, &scripts).await?
            } else {
                None
            };
            let node = dep_graph.add_package(build);
            if let Some(update_cause) = build_is_updated {
                updated_package_nodes.push(PackageNodeUpdate {
                    package: node,
                    cause: update_cause,
                });
            }
            if auto_build_config
                .bootstrap_studio_package
                .as_ref()
                .map_or(false, |package| package.matches_build(&metadata.ident))
            {
                studio_packages.bootstrap_studio = Some(node);
            }
            if auto_build_config
                .studio_package
                .as_ref()
                .map_or(false, |package| package.matches_build(&metadata.ident))
            {
                studio_packages.studio = Some(node);
            }
            if start_package_idents
                .iter()
                .any(|ident| ident.matches_build(&metadata.ident))
            {
                source_package_nodes.push(node);
            }

            if let Some((_, existing_package)) =
                packages.insert(metadata.ident.clone(), (node, metadata.clone()))
            {
                error!(
                    "Found a package {} which a plan at {} and a duplicate plan at {}",
                    metadata.ident,
                    metadata.path.display(),
                    existing_package.path.display()
                );
                return Err(anyhow!("Duplicate package detected"));
            };
        }

        for (_ident, (node, metadata)) in packages.iter() {
            for dep in metadata.build_deps.iter() {
                let mut dep_package = None;
                for (dep_ident, (dep_node, _dep_metadata)) in packages.iter() {
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
                    dep_graph.add_build_dependency(*node, *dep_node);
                }
            }
            for dep in metadata.deps.iter() {
                let mut dep_package = None;
                for (dep_ident, (dep_node, _dep_metadata)) in packages.iter() {
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
                    dep_graph.add_runtime_dependency(*node, *dep_node);
                }
            }
        }
        for (_ident, (node, metadata)) in packages.iter() {
            dep_graph.package_mut(node).studio_type = match dep_graph.package(node).package_type {
                PackageType::Native => Some(PackageStudioType::Native),
                PackageType::Standard => match &studio_packages.studio {
                    Some(studio_package) => match node.is_dependency_of(&dep_graph, studio_package)
                    {
                        true => match &studio_packages.bootstrap_studio {
                            Some(bootstrap_studio_package) => {
                                match node.is_dependency_of(&dep_graph, &bootstrap_studio_package) {
                                    true => {
                                        error!("Cannot build {} dependency in a studio or a bootstrap studio as it is a dependency of both of them, maybe it should be a native package", _ident);
                                        None
                                    }
                                    false => {
                                        dep_graph.add_studio_dependency(
                                            *node,
                                            *bootstrap_studio_package,
                                        );
                                        Some(PackageStudioType::Bootstrap)
                                    }
                                }
                            }
                            None => {
                                error!("Cannot build {} dependency in studio as it is required to build a studio, maybe you should provide a bootstrap studio package to build it", _ident);
                                None
                            }
                        },
                        false => {
                            dep_graph.add_studio_dependency(*node, *studio_package);
                            Some(PackageStudioType::Standard)
                        }
                    },
                    None => match &studio_packages.bootstrap_studio {
                        Some(bootstrap_studio_package) => {
                            match node.is_dependency_of(&dep_graph, &bootstrap_studio_package) {
                                true => {
                                    error!("Cannot build {} dependency in a bootstrap studio as it is a dependency of the bootstrap studio, maybe it should be a native package", _ident);
                                    None
                                }
                                false => {
                                    dep_graph
                                        .add_studio_dependency(*node, *bootstrap_studio_package);
                                    Some(PackageStudioType::Bootstrap)
                                }
                            }
                        }
                        None => {
                            error!("Cannot build {} dependency as no studio or bootstrap studio package has been provided, maybe you should provide a bootstrap studio package to build it", _ident);
                            None
                        }
                    },
                },
            };
        }
    }
    Ok((
        dep_graph,
        source_package_nodes,
        updated_package_nodes,
        studio_packages,
    ))
}

async fn visualize(args: VisualizeArgs) -> Result<()> {
    let scripts = Arc::new(Scripts::new().await?);
    let selected_packages = args
        .packages
        .into_iter()
        .map(PackageDepIdent::try_from)
        .collect::<Result<Vec<PackageDepIdent>, _>>()?;

    let auto_build_config = HabitatAutoBuildConfiguration::new(
        args.config_path
            .unwrap_or(env::current_dir()?.join("hab-auto-build.json")),
    )
    .await
    .context("Failed to load habitat auto build configuration")?;

    let (dep_graph, selected_package_nodes, _, _) =
        dep_graph_build(selected_packages, &auto_build_config, false, None, scripts).await?;

    let output = {
        if selected_package_nodes.is_empty() {
            format!(
                "{:?}",
                Dot::with_config(&*dep_graph, &[Config::EdgeNoLabel])
            )
        } else {
            let build_graph = NodeFiltered::from_fn(&*dep_graph, |node| {
                let node = PackageNode(node);
                let mut include = false;
                for selected_package_node in selected_package_nodes.iter() {
                    match args.analysis_type {
                        DependencyAnalysis::Build | DependencyAnalysis::Runtime => {
                            if node.is_dependency_of(&dep_graph, selected_package_node) {
                                include = true;
                                break;
                            }
                        }
                        DependencyAnalysis::Reverse => {
                            if selected_package_node.is_dependency_of(&dep_graph, &node) {
                                include = true;
                                break;
                            }
                        }
                    }
                }
                include
            });
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
    let selected_packages = args
        .packages
        .into_iter()
        .map(PackageDepIdent::try_from)
        .collect::<Result<Vec<PackageDepIdent>, _>>()?;

    let auto_build_config = HabitatAutoBuildConfiguration::new(
        args.config_path
            .unwrap_or(env::current_dir()?.join("hab-auto-build.json")),
    )
    .await
    .context("Failed to load habitat auto build configuration")?;

    let (dep_graph, selected_package_nodes, _, _) =
        dep_graph_build(selected_packages, &auto_build_config, false, None, scripts).await?;

    let packages = if selected_package_nodes.is_empty() {
        let mut packages = Vec::new();
        for (_, node) in dep_graph.node_references() {
            packages.push(format!("{}", node.plan.ident))
        }
        packages
    } else {
        let build_graph = NodeFiltered::from_fn(&*dep_graph, |node| {
            let node = PackageNode(node);
            let mut include = false;
            for selected_package_node in selected_package_nodes.iter() {
                match args.analysis_type {
                    DependencyAnalysis::Build | DependencyAnalysis::Runtime => {
                        if node.is_dependency_of(&dep_graph, selected_package_node) {
                            include = true;
                            break;
                        }
                    }
                    DependencyAnalysis::Reverse => {
                        if selected_package_node.is_dependency_of(&dep_graph, &node) {
                            include = true;
                            break;
                        }
                    }
                }
            }
            include
        });

        let mut packages = Vec::new();
        for (_, node) in build_graph.node_references() {
            packages.push(format!("{}", node.plan.ident))
        }
        packages
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

async fn serve(args: ServerArgs) -> Result<()> {
    let scripts = Arc::new(Scripts::new().await?);
    let auto_build_config = HabitatAutoBuildConfiguration::new(
        args.config_path
            .unwrap_or(env::current_dir()?.join("hab-auto-build.json")),
    )
    .await
    .context("Failed to load habitat auto build configuration")?;
    let (dep_graph, _, _, _) =
        dep_graph_build(vec![], &auto_build_config, false, None, scripts).await?;
    server::start(dep_graph, args.port).await;
    Ok(())
}

async fn build(args: BuildArgs) -> Result<()> {
    let scripts = Arc::new(Scripts::new().await?);
    let manually_updated_package_idents = args
        .updated_packages
        .into_iter()
        .map(PackageDepIdent::try_from)
        .collect::<Result<Vec<PackageDepIdent>, _>>()?;

    let config_path = args
        .config_path
        .unwrap_or(env::current_dir()?.join("hab-auto-build.json"));
    let package_skip_path = config_path
        .parent()
        .expect("Hab auto build configuration has no parent directory")
        .join(".hab-build-ignore");

    let auto_build_config = HabitatAutoBuildConfiguration::new(config_path)
        .await
        .context("Failed to load habitat auto build configuration")?;

    let package_skip_list = PackageSkipList::new(package_skip_path).await.ok();

    let (dep_graph, manually_updated_package_nodes, mut package_node_updates, studio_packages) =
        dep_graph_build(
            manually_updated_package_idents,
            &auto_build_config,
            true,
            package_skip_list.as_ref(),
            scripts.clone(),
        )
        .await?;

    for package_node_update in package_node_updates.iter() {
        info!(
            "Detected update due to {} in {} at {}",
            package_node_update.cause,
            dep_graph[*package_node_update.package].plan.ident,
            dep_graph[*package_node_update.package]
                .plan
                .source
                .display()
        );
    }
    if !package_node_updates.is_empty() {
        let skip_packages = Confirm::new("Do you want to skip the build of certain updated packages?")
        .with_default(false)
        .with_help_message("This is useful to avoid rebuilding packages that have only trivial formatting, styling changes")
        .prompt()?;
        if skip_packages {
            let mut skipped_packages = package_skip_list
                .map_or(Vec::new(), |skip_list| skip_list.packages)
                .into_iter()
                .filter(|skipped_package| {
                    !package_node_updates.iter().any(|package_node_update| {
                        &dep_graph[*package_node_update.package].plan.ident == skipped_package
                    })
                })
                .collect::<Vec<_>>();
            let default_skipped_packages = skipped_packages
                .iter()
                .enumerate()
                .map(|(index, _)| index)
                .collect::<Vec<_>>();

            for package_node_update in package_node_updates.iter() {
                if !matches!(package_node_update.cause, UpdateCause::UpdatedDependency) {
                    let build_ident = dep_graph[*package_node_update.package].plan.ident.clone();
                    if !skipped_packages.contains(&build_ident) {
                        debug!(
                            "Adding {:?} to {:?} due to {:?}",
                            build_ident, skip_packages, package_node_update.cause
                        );
                        skipped_packages.push(build_ident);
                    }
                }
            }
            let skipped_packages =
                MultiSelect::new("Select the packages to be skipped:", skipped_packages)
                    .with_default(&default_skipped_packages)
                    .prompt()?;

            package_node_updates.retain(|package_node_update| {
                !skipped_packages.contains(&dep_graph[*package_node_update.package].plan.ident)
            });
            tokio::fs::write(
                ".hab-build-ignore",
                serde_json::to_string_pretty(&PackageSkipList {
                    updated_at: Utc::now(),
                    packages: skipped_packages,
                })?,
            )
            .await?;
        }
    }

    let feedback_edges: Vec<EdgeIndex> = greedy_feedback_arc_set(&*dep_graph)
        .map(|e| e.id())
        .collect();
    for feedback_edge in feedback_edges.iter() {
        if let Some((start, end)) = dep_graph.edge_endpoints(*feedback_edge) {
            warn!(
                "Package {:?} depends on {:?} which creates a cycle",
                dep_graph[start], dep_graph[end]
            );
        }
    }
    if !feedback_edges.is_empty() {
        return Err(anyhow!("Building cyclic dependencies it not allowed, please break cycles and attempt to build again."));
    }

    let build_graph = NodeFiltered::from_fn(&*dep_graph, |node| {
        let node = PackageNode(node);
        let mut is_affected = false;
        for package_node_update in package_node_updates.iter() {
            if node.is_reverse_dependency_of(&dep_graph, &package_node_update.package) {
                is_affected = true;
            }
            // filter out updates that are not reverse dependencies of our selected packages
            if !manually_updated_package_nodes.is_empty() {
                let mut should_include = false;
                for manually_updated_package_node in manually_updated_package_nodes.iter() {
                    if package_node_update
                        .package
                        .is_reverse_dependency_of(&dep_graph, manually_updated_package_node)
                    {
                        should_include = true;
                        break;
                    }
                }
                if !should_include {
                    if is_affected {
                        warn!("Skipping package {} that depends on package {} that was updated due to {}", dep_graph[*node].plan.ident, dep_graph[*package_node_update.package].plan.ident, package_node_update.cause);
                    }
                    continue;
                }
            }
            if is_affected {
                break;
            }
        }
        is_affected
    });

    let mut build_order =
        algo::toposort(&build_graph, None).map_err(|err| anyhow!("Cycle detected: {:?}", err))?;

    build_order.reverse();
    let build_order = Arc::new(build_order);

    info!(
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
        auto_build_config.bootstrap_studio_package,
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
        Commands::Server(args) => serve(args).await,
    }
}

struct Scheduler {
    session_id: String,
    scripts: Arc<Scripts>,
    built_packages: Arc<DashSet<NodeIndex>>,
    pending_packages: Arc<DashSet<NodeIndex>>,
    bootstrap_studio_package: Option<PackageDepIdent>,
    build_order: Arc<Vec<NodeIndex>>,
    dep_graph: Arc<PackageDependencyGraph>,
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
        bootstrap_studio_package: Option<PackageDepIdent>,
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

        let mut build_log_file = File::create(build.build_log_file(&session_id))
            .await
            .context(format!(
                "Failed to create build log file for package '{:?}'",
                build.plan
            ))?;
        let repo = build.plan.repo.as_path();
        let source = build.plan.source.strip_prefix(repo)?;

        let mut child = match build.studio_type {
            Some(PackageStudioType::Native) => {
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
            Some(PackageStudioType::Bootstrap) => {
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
                    .arg(bootstrap_studio_package.unwrap().to_string())
                    .arg("hab-studio")
                    .arg("-t")
                    .arg("bootstrap")
                    .arg("-r")
                    .arg(
                        PathBuf::from("/hab")
                            .join("studios")
                            .join(format!("hab-auto-build-{}", session_id,)),
                    )
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
            Some(PackageStudioType::Standard) => {
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
            None => {
                return Err(anyhow!("Unable to build package {}", source.display()));
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
        dep_graph: Arc<PackageDependencyGraph>,
        bootstrap_studio_package: Option<PackageDepIdent>,
        scripts: Arc<Scripts>,
    ) -> Scheduler {
        Scheduler {
            session_id,
            scripts,
            built_packages: Arc::new(DashSet::new()),
            pending_packages: Arc::new(DashSet::new()),
            bootstrap_studio_package,
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
        dep_graph: Arc<PackageDependencyGraph>,
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
        let handle = tokio::spawn({
            let built_packages = self.built_packages.clone();
            let scripts = self.scripts.clone();
            let pending_packages = self.pending_packages.clone();
            let build_order = self.build_order.clone();
            let dep_graph = self.dep_graph.clone();
            let worker_index = self.handles.len() + 1;
            let session_id = self.session_id.clone();
            let bootstrap_studio_package = self.bootstrap_studio_package.clone();
            async move {
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
                            builder
                                .build(
                                    build_deps,
                                    bootstrap_studio_package.clone(),
                                    scripts.clone(),
                                )
                                .await?;
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
            }
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
