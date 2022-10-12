use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use core::cmp::Ordering;
use dashmap::DashSet;
use futures::{stream::FuturesUnordered, StreamExt};
use names::{Generator, Name};
use petgraph::{
    algo,
    dot::{Config, Dot},
    stable_graph::NodeIndex,
    visit::{IntoNodeReferences, NodeFiltered},
    Direction, Graph,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    env,
    ffi::OsString,
    fmt::Display,
    os::unix::prelude::OsStringExt,
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

const HAB_AUTO_BUILD_EXTRACT_SOURCE_FILE: (&str, &[u8]) =
    ("extract.sh", include_bytes!("./scripts/extract.sh"));

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
    build_id: Option<String>,
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
    fn repo_build_folder(&self, build_id: &str) -> PathBuf {
        self.plan
            .repo
            .join(".hab-auto-build")
            .join("builds")
            .join(&build_id)
    }
    fn package_build_folder(&self, build_id: &str) -> PathBuf {
        self.plan
            .repo
            .join(".hab-auto-build")
            .join("builds")
            .join(&build_id)
            .join(self.plan.ident.origin.as_str())
            .join(self.plan.ident.name.as_str())
    }
    fn package_studio_build_folder(&self, build_id: &str) -> PathBuf {
        PathBuf::from("/src")
            .join(".hab-auto-build")
            .join("builds")
            .join(&build_id)
            .join(self.plan.ident.origin.as_str())
            .join(self.plan.ident.name.as_str())
    }
    fn build_log_file(&self, build_id: &str) -> PathBuf {
        self.package_build_folder(build_id).join("build.log")
    }
    fn build_success_file(&self, build_id: &str) -> PathBuf {
        self.package_build_folder(build_id).join("BUILD_OK")
    }
    fn build_results_file(&self, build_id: &str) -> PathBuf {
        self.package_build_folder(build_id).join("last_build.env")
    }
    async fn last_build_artifact(&self, build_id: &str) -> Result<PackageArtifactIdent> {
        let metadata = tokio::fs::metadata(self.build_success_file(build_id)).await?;
        if metadata.is_file() {
            let build_results =
                tokio::fs::read_to_string(self.build_results_file(build_id)).await?;
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
                self.build_results_file(build_id).display()
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
        write!(f, "{}/{}/{}", self.origin, self.name, self.version,)
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
    ) -> Result<Option<PackageArtifactIdent>> {
        let output = tokio::process::Command::new("hab")
            .arg("pkg")
            .arg("path")
            .arg(self.to_string())
            .output()
            .await?
            .stdout;
        let path = PathBuf::from(OsString::from_vec(output));
        if let Ok(path) = path.strip_prefix(HAB_PKGS_PATH.as_path()) {
            let pkg_ident = PackageIdent::try_from(path.to_str().unwrap().trim())?;
            Ok(Some(pkg_ident.artifact(target)))
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
            f.write_str(version)?;
        }
        if let Some(release) = self.release.as_ref() {
            f.write_str(release)?;
        }
        Ok(())
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

    pub async fn metadata(
        &self,
        target: PackageTarget,
        script: &MetadataScript,
    ) -> Result<PlanMetadata> {
        script.execute(target, self).await
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
    pub async fn metadata(
        &self,
        target: PackageTarget,
        script: &MetadataScript,
    ) -> Result<PlanMetadata> {
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

pub struct MetadataScript {
    tmp_dir: TempDir,
    script_path: PathBuf,
}

impl MetadataScript {
    pub async fn new() -> Result<MetadataScript> {
        let tmp_dir = TempDir::new("hab-auto-build")?;
        let (script_file_name, script_file_data) = HAB_AUTO_BUILD_EXTRACT_SOURCE_FILE;
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
        Ok(MetadataScript {
            tmp_dir,
            script_path,
        })
    }

    pub async fn execute(&self, target: PackageTarget, plan: &PlanSource) -> Result<PlanMetadata> {
        let output = tokio::process::Command::new("bash")
            .arg(self.script_path.as_path())
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
) -> Result<(Graph<PackageBuild, ()>, Vec<NodeIndex>, Vec<NodeIndex>)> {
    let script = MetadataScript::new().await?;
    let mut dep_graph = Graph::new();
    let mut packages = HashMap::new();
    let mut source_package_nodes = Vec::new();
    let mut sink_package_nodes = Vec::new();

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
                .metadata(PackageTarget::AArch64Linux, &script)
                .await?;
            let build = PackageBuild::new(repo.clone(), metadata.clone());
            let node = dep_graph.add_node(build);
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
    Ok((dep_graph, source_package_nodes, sink_package_nodes))
}

async fn visualize(args: VisualizeArgs) -> Result<()> {
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

    let (dep_graph, start_package_nodes, end_package_nodes) =
        dep_graph_build(start_package_idents, end_package_idents, auto_build_config).await?;

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
        format!(
            "{:?}",
            Dot::with_config(&build_graph, &[Config::EdgeNoLabel])
        )
    };
    let output = output.replace("digraph {", "digraph { rankdir=LR; node [shape=rectangle, color=blue, fillcolor=lightskyblue, style=filled ]; edge [color=darkgreen];");
    let mut output_file = tokio::fs::File::create(args.output).await?;
    output_file.write_all(output.as_bytes()).await?;
    output_file.shutdown().await?;

    Ok(())
}

async fn analyze(args: AnalyzeArgs) -> Result<()> {
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

    let (dep_graph, start_package_nodes, end_package_nodes) =
        dep_graph_build(start_package_idents, end_package_idents, auto_build_config).await?;

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

async fn build(args: BuildArgs) -> Result<()> {
    let updated_package_idents = args
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

    let (dep_graph, updated_package_nodes, _) =
        dep_graph_build(updated_package_idents, Vec::new(), auto_build_config).await?;

    let build_graph = NodeFiltered::from_fn(&dep_graph, |node| {
        let mut is_affected = false;
        for updated_package_node in updated_package_nodes.iter() {
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
        args.build_id.unwrap_or_else(|| {
            let mut generator = Generator::with_naming(Name::Numbered);
            generator.next().unwrap()
        }),
        build_order.clone(),
        Arc::new(dep_graph),
    );

    info!(
        "Beginning build {}, {} packages to be built",
        scheduler.build_id,
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
    build_id: String,
    built_packages: Arc<DashSet<NodeIndex>>,
    pending_packages: Arc<DashSet<NodeIndex>>,
    build_order: Arc<Vec<NodeIndex>>,
    dep_graph: Arc<Graph<PackageBuild, ()>>,
    handles: FuturesUnordered<JoinHandle<Result<(), anyhow::Error>>>,
}

struct PackageBuilder<'a> {
    build_id: String,
    worker_index: usize,
    build: &'a PackageBuild,
}

impl<'a> PackageBuilder<'a> {
    fn new(build_id: &str, worker_index: usize, build: &'a PackageBuild) -> PackageBuilder<'a> {
        PackageBuilder {
            build_id: build_id.to_owned(),
            worker_index,
            build,
        }
    }
    async fn build(self, deps_in_current_build: Vec<&PackageBuild>) -> Result<()> {
        let PackageBuilder {
            build_id,
            worker_index,
            build,
        } = self;
        info!(
            worker = worker_index,
            "Building {:?} with {}",
            build,
            build.plan.path.display()
        );

        tokio::fs::create_dir_all(&build.package_build_folder(&build_id))
            .await
            .with_context(|| {
                format!(
                    "Failed to create build folder '{}' for package '{:?}'",
                    build.package_build_folder(&build_id).display(),
                    build.plan
                )
            })?;

        if let Ok(true) = tokio::fs::metadata(build.build_success_file(&build_id).as_path())
            .await
            .map(|metadata| metadata.is_file())
        {
            info!("Package {:?} already built", build.plan);
            return Ok(());
        }
        let mut build_log_file = File::create(build.build_log_file(&build_id))
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
                    build.build_log_file(&build_id).display()
                );
                tokio::process::Command::new("hab")
                    .arg("pkg")
                    .arg("build")
                    .arg("-N")
                    .arg(build.source_folder())
                    .env("HAB_FEAT_NATIVE_PACKAGE_SUPPORT", "1")
                    .env("HAB_OUTPUT_PATH", build.package_build_folder(&build_id))
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
                            dep_in_current_build.last_build_artifact(&build_id).await
                        {
                            resolved_dep = Some(PackageIdent::from(artifact));
                            break;
                        }
                    }
                    if resolved_dep.is_none() {
                        if let Ok(Some(artifact)) =
                            dep.latest_artifact(build.plan.ident.target).await
                        {
                            resolved_dep = Some(PackageIdent::from(artifact));
                        } else {
                            warn!(
                                "Failed to find local build artifact for {}, required by {}",
                                dep, build.plan.ident
                            );
                        }
                    }
                    if let Some(resolved_dep) = resolved_dep {
                        pkg_deps.push(format!("{}", resolved_dep))
                    }
                }
                info!(
                    "Building package {} in {} with bootstrap studio, view log at {}",
                    source.display(),
                    repo.display(),
                    build.build_log_file(&build_id).display()
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
                        build_id, build.plan.ident.origin, build.plan.ident.name
                    )))
                    .arg("build")
                    .arg(source)
                    .env("HAB_ORIGIN", build.plan.ident.origin.as_str())
                    .env("HAB_LICENSE", "accept-no-persist")
                    .env("HAB_PKG_DEPS", pkg_deps.join(":"))
                    .env("HAB_STUDIO_SECRET_STUDIO_ENTER", "1")
                    .env(
                        "HAB_STUDIO_SECRET_HAB_OUTPUT_PATH",
                        build.package_studio_build_folder(&build_id),
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
                    build.build_log_file(&build_id).display()
                );
                tokio::process::Command::new("hab")
                    .arg("pkg")
                    .arg("build")
                    .arg(source)
                    .env(
                        "HAB_STUDIO_SECRET_OUTPUT_PATH",
                        build.package_studio_build_folder(&build_id),
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
                                let mut success_file = File::create(build.build_success_file(&build_id)).await.context(format!(
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
                                error!(worker = worker_index, "Failed to build {:?}, build process exited with {}, please the build log for errors: {}", build.plan, exit_code, build.build_log_file(&build_id).display());
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
        build_id: String,
        build_order: Arc<Vec<NodeIndex>>,
        dep_graph: Arc<Graph<PackageBuild, ()>>,
    ) -> Scheduler {
        Scheduler {
            build_id,
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
        let pending_packages = self.pending_packages.clone();
        let build_order = self.build_order.clone();
        let dep_graph = self.dep_graph.clone();
        let worker_index = self.handles.len() + 1;
        let build_id = self.build_id.clone();
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
                        let builder = PackageBuilder::new(&build_id, worker_index, build);
                        let build_deps = dep_graph
                            .neighbors_directed(package_index, Direction::Outgoing)
                            .into_iter()
                            .map(|dep_index| &dep_graph[dep_index])
                            .collect::<Vec<_>>();
                        builder.build(build_deps).await?;
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
