use std::{
    collections::HashMap,
    fmt::Display,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::Sender,
    time::Instant,
};
#[cfg(not(target_os = "windows"))]
use std::io::Read;
#[cfg(windows)]
use std::fs::{self, File};
#[cfg(windows)]
use std::io::BufWriter;

use chrono::{DateTime, Utc};
use color_eyre::{
    eyre::{eyre, Context, Result},
    Help, SectionExt,
};
use diesel::SqliteConnection;
use ignore::{ParallelVisitor, ParallelVisitorBuilder, WalkBuilder, WalkState};

use lazy_static::lazy_static;
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "windows")]
use sha2::{Digest, Sha256};

use tracing::{debug, error, info, trace};

use crate::{
    check::PlanContextConfig,
    store::{self, ModificationIndex},
};

use super::{
    ArtifactCache, ChangeDetectionMode, Metadata, MinimalArtifactContext, PackageBuildIdent,
    PackageBuildVersion, PackageDepIdent, PackageIdent, PackageName, PackageOrigin,
    PackageResolvedDepIdent, PackageSource, PackageTarget, RepoContext, RepoContextID,
};

fn get_platform_specific_paths() -> Vec<(PathBuf, PackageTarget)> {
    let mut paths = Vec::new();
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        paths.push((
            vec!["x86_64-linux", "plan.sh"],
            PackageTarget::parse("x86_64-linux").unwrap(),
        ));
        paths.push((
            vec!["habitat", "x86_64-linux", "plan.sh"],
            PackageTarget::parse("x86_64-linux").unwrap(),
        ));
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        paths.push((
            vec!["aarch64-linux", "plan.sh"],
            PackageTarget::parse("aarch64-linux").unwrap(),
        ));
        paths.push((
            vec!["habitat", "aarch64-linux", "plan.sh"],
            PackageTarget::parse("aarch64-linux").unwrap(),
        ));
    }

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        paths.push((
            vec!["x86_64-darwin", "plan.sh"],
            PackageTarget::parse("x86_64-darwin").unwrap(),
        ));
        paths.push((
            vec!["habitat", "x86_64-darwin", "plan.sh"],
            PackageTarget::parse("x86_64-darwin").unwrap(),
        ));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        paths.push((
            vec!["aarch64-darwin", "plan.sh"],
            PackageTarget::parse("aarch64-darwin").unwrap(),
        ));
        paths.push((
            vec!["habitat", "aarch64-darwin", "plan.sh"],
            PackageTarget::parse("aarch64-darwin").unwrap(),
        ));
    }

    #[cfg(any(target_os = "windows", target_arch = "x86_64"))]
    {
        paths.push((
            vec!["x86_64-windows", "plan.ps1"],
            PackageTarget::parse("x86_64-windows").unwrap(),
        ));
        paths.push((
            vec!["habitat", "x86_64-windows", "plan.sh"],
            PackageTarget::parse("x86_64-windows").unwrap(),
        ));
    }

    #[cfg(not(target_os = "windows"))]
    {
        paths.push((vec!["plan.sh"], PackageTarget::default()));
        paths.push((vec!["habitat", "plan.sh"], PackageTarget::default()));
    }

    #[cfg(target_os = "windows")]
    {
        paths.push((vec!["plan.ps1"], PackageTarget::default()));
        paths.push((vec!["habitat", "plan.ps1"], PackageTarget::default()));
    }

    paths
        .into_iter()
        .map(|(parts, target)| (parts.iter().collect::<PathBuf>(), target))
        .collect()
}

lazy_static! {
    static ref RELATIVE_PLAN_FILE_PATHS: Vec<(PathBuf, PackageTarget)> =
        get_platform_specific_paths();
}

#[cfg(not(target_os = "windows"))]
const PLAN_DATA_EXTRACT_SCRIPT: &[u8] = include_bytes!("../scripts/plan_data_extract.sh");
#[cfg(target_os = "windows")]
const PLAN_DATA_EXTRACT_SCRIPT: &[u8] = include_bytes!("../scripts/plan_data_extract.ps1");
const PLAN_CONFIG_FILE: &str = ".hab-plan-config.toml";

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub(crate) struct PlanContextPath(PathBuf);

impl AsRef<Path> for PlanContextPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub(crate) struct PlanTargetContextPath(PathBuf);

impl AsRef<Path> for PlanTargetContextPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize, Hash)]
pub(crate) struct PlanContextFilePath(PathBuf);

impl AsRef<Path> for PlanContextFilePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub(crate) struct PlanFilePath(PathBuf);

impl PlanFilePath {
    pub fn plan_config_path(&self) -> PathBuf {
        self.0.parent().unwrap().join(PLAN_CONFIG_FILE)
    }
}

impl AsRef<Path> for PlanFilePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct RawPlanData {
    pub origin: PackageOrigin,
    pub name: PackageName,
    pub version: PackageBuildVersion,
    pub source: Option<PackageSource>,
    pub licenses: Vec<String>,
    pub deps: Vec<PackageDepIdent>,
    pub build_deps: Vec<PackageDepIdent>,
    pub scaffolding_dep: Option<PackageDepIdent>,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub(crate) struct PlanContextID(PackageBuildIdent);

impl From<&PackageBuildIdent> for PlanContextID {
    fn from(value: &PackageBuildIdent) -> Self {
        PlanContextID(value.clone())
    }
}

impl AsRef<PackageBuildIdent> for PlanContextID {
    fn as_ref(&self) -> &PackageBuildIdent {
        &self.0
    }
}

impl From<PlanContextID> for PackageBuildIdent {
    fn from(value: PlanContextID) -> Self {
        value.0
    }
}

impl Display for PlanContextID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PlanContext {
    pub id: PlanContextID,
    pub repo_id: RepoContextID,
    pub context_path: PlanContextPath,
    pub target_context_path: PlanTargetContextPath,
    pub target_context_last_modified_at: DateTime<Utc>,
    pub plan_path: PlanFilePath,
    pub source: Option<PackageSource>,
    pub licenses: Vec<String>,
    pub deps: Vec<PackageResolvedDepIdent>,
    pub build_deps: Vec<PackageResolvedDepIdent>,
    pub latest_artifact: Option<PlanContextLatestArtifact>,
    pub files_changed_on_disk: Vec<PlanContextFileChangeOnDisk>,
    pub files_changed_on_git: Vec<PlanContextFileChangeOnGit>,
    pub is_native: bool,
    pub plan_config: Option<PlanContextConfig>,
}

impl PlanContext {
    pub fn config(&self) -> PlanContextConfig {
        let context_rules = PlanContextConfig::default();
        if let Some(rules) = self.plan_config.as_ref() {
            context_rules.merge(rules)
        } else {
            context_rules
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PlanContextLatestArtifact {
    pub created_at: DateTime<Utc>,
    pub ident: PackageIdent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PlanContextFileChangeOnDisk {
    pub last_modified_at: DateTime<Utc>,
    pub real_last_modified_at: DateTime<Utc>,
    pub path: PlanContextFilePath,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PlanContextFileChangeOnGit {
    pub last_modified_at: DateTime<Utc>,
    pub path: PlanContextFilePath,
}

pub(crate) enum PlanContextPathGitSyncStatus {
    Synced(PathBuf, DateTime<Utc>, DateTime<Utc>),
    LocallyModified(PathBuf, DateTime<Utc>),
}

impl PlanContext {
    #[allow(clippy::too_many_arguments)]
    #[cfg(not(target_os = "windows"))]
    pub fn read_from_disk(
        connection: Option<&mut SqliteConnection>,
        modification_index: Option<&ModificationIndex>,
        repo_ctx: &RepoContext,
        artifact_cache: &ArtifactCache,
        plan_ctx_path: &PlanContextPath,
        plan_target_ctx_path: &PlanTargetContextPath,
        plan_path: &PlanFilePath,
        target: PackageTarget,
        change_detection_mode: ChangeDetectionMode,
    ) -> Result<PlanContext> {
        let start = Instant::now();
        let mut child =  Command::new("bash")
            .arg("-s")
            .arg("-")
            .arg(plan_path.as_ref())
            .arg(plan_ctx_path.as_ref())
            .arg(plan_target_ctx_path.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(plan_target_ctx_path.as_ref())
            .spawn()
            .context("Failed to execute bash shell")
            .with_suggestion(|| "Make sure you have bash installed on your system, and that it's location is included in your PATH")?;
        let mut stdin = child
            .stdin
            .take()
            .expect("Failed to acquire stdin to bash process");
        stdin.write_all(PLAN_DATA_EXTRACT_SCRIPT)?;
        stdin.flush()?;
        drop(stdin);
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            let raw_data: RawPlanData = serde_json::from_str(&stdout)
                .with_context(|| {
                    format!(
                        "Failed to read extracted JSON data from plan file at '{}'",
                        plan_path.as_ref().display()
                    )
                })
                .with_section(move || stdout.header("stdout: "))
                .with_section(move || stderr.header("stderr: "))
                .with_suggestion(|| "Ensure your plan file does not generate output outside the standard functions like 'do_begin', 'do_prepare', 'do_build', 'do_check' and 'do_install'")?;
            let id = PlanContextID(PackageBuildIdent {
                origin: raw_data.origin,
                name: raw_data.name,
                version: raw_data.version,
                target: target.to_owned(),
            });
            let plan_config_path = plan_path.plan_config_path();
            let plan_config = if let Ok(mut file) = std::fs::File::open(plan_config_path.as_path())
            {
                let mut data = String::new();
                file.read_to_string(&mut data)?;
                match PlanContextConfig::from_str(data.as_str(), target)
                    .with_section(move || {
                        data.header(format!("{}:", "File Contents".bright_cyan()))
                    })
                    .with_suggestion(|| {
                        "Ensure your .hab-plan-config.toml file contains valid rules"
                    }) {
                    Ok(plan_rules) => Some(plan_rules),
                    Err(err) => {
                        info!(target: "user-ui", "{} Failed to read plan config from {}: {:?}", "error:".bold().red(), plan_config_path.strip_prefix(repo_ctx.path.as_ref()).unwrap().display(), err);
                        None
                    }
                }
            } else {
                None
            };

            let mut plan_ctx = PlanContext {
                id,
                repo_id: repo_ctx.id.clone(),
                is_native: repo_ctx.is_native_plan(plan_ctx_path),
                context_path: plan_ctx_path.clone(),
                target_context_last_modified_at: plan_target_ctx_path.last_modifed_at()?,
                target_context_path: plan_target_ctx_path.clone(),
                plan_path: plan_path.clone(),
                source: raw_data.source,
                licenses: raw_data.licenses,
                deps: raw_data
                    .deps
                    .into_iter()
                    .map(|d| d.to_resolved_dep_ident(target.to_owned()))
                    .collect(),
                build_deps: raw_data
                    .build_deps
                    .into_iter()
                    .chain(raw_data.scaffolding_dep)
                    .map(|d| d.to_resolved_dep_ident(target.to_owned()))
                    .collect(),
                latest_artifact: None,
                files_changed_on_disk: Vec::new(),
                files_changed_on_git: Vec::new(),
                plan_config,
            };
            let latest_artifact = artifact_cache.latest_plan_minimal_artifact(&plan_ctx.id);
            plan_ctx.determine_changes(
                connection,
                modification_index,
                latest_artifact.as_ref(),
                change_detection_mode,
            )?;
            trace!(
                "Read plan context {} from disk in {}s",
                plan_ctx.context_path.as_ref().display(),
                start.elapsed().as_secs_f32()
            );
            Ok(plan_ctx)
        } else {
            Err(eyre!(
                "Failed to extract plan data from {}, bash process exited with code: {}",
                plan_path.as_ref().display(),
                output.status,
            )
            .with_section(move || stdout.header("stdout: "))
            .with_section(move || stderr.header("stderr: ")))
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(target_os = "windows")]
    pub fn read_from_disk(
        connection: Option<&mut SqliteConnection>,
        modification_index: Option<&ModificationIndex>,
        repo_ctx: &RepoContext,
        artifact_cache: &ArtifactCache,
        plan_ctx_path: &PlanContextPath,
        plan_target_ctx_path: &PlanTargetContextPath,
        plan_path: &PlanFilePath,
        target: PackageTarget,
        change_detection_mode: ChangeDetectionMode,
    ) -> Result<PlanContext> {
        let start = Instant::now();
        let temp_dir = std::env::temp_dir();
        // We need to create the extraction script to execute a plan and retrieve the required metadata.
        // We should revisit this issue, as it is generating multiple temporary files.
        // It might be helpful to extract a separate function for Windows or refactor the existing code.
        let mut hasher = Sha256::new();
        hasher.update(plan_path.as_ref().display().to_string().as_bytes());
        let result = hasher.finalize();
        let unique_id = format!("{:x}", result);
        let temp_file_path: PathBuf = temp_dir.join(format!("plan_data_extract_{}.ps1", unique_id));
        {
            let mut temp_file = File::create(&temp_file_path).with_context(|| {
                format!(
                    "Failed to create temporary file at '{}'",
                    temp_file_path.display()
                )
            })?;
            let mut writer = BufWriter::new(&mut temp_file);
            writer
                .write_all(PLAN_DATA_EXTRACT_SCRIPT)
                .with_context(|| {
                    format!(
                        "Failed to write to temporary file at '{}'",
                        temp_file_path.display()
                    )
                })?;
        }

        let child =  Command::new("powershell")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&temp_file_path)
            .arg(plan_path.as_ref().display().to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(plan_target_ctx_path.as_ref().display().to_string())
            .spawn()
            .context("Failed to execute bash shell")
            .with_suggestion(|| "Make sure you have bash installed on your system, and that it's location is included in your PATH")?;
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        fs::remove_file(&temp_file_path).with_context(|| {
            format!(
                "Failed to delete temporary file at '{}'",
                temp_file_path.display()
            )
        })?;

        if output.status.success() {
            let raw_data: RawPlanData = serde_json::from_str(&stdout)
                .with_context(|| {
                    format!(
                        "Failed to read extracted JSON data from plan file at '{}'",
                        plan_path.as_ref().display()
                    )
                })
                .with_section(move || stdout.header("stdout: "))
                .with_section(move || stderr.header("stderr: "))
                .with_suggestion(|| "Ensure your plan file does not generate output outside the standard functions like 'do_begin', 'do_prepare', 'do_build', 'do_check' and 'do_install'")?;
            let id = PlanContextID(PackageBuildIdent {
                origin: raw_data.origin,
                name: raw_data.name,
                version: raw_data.version,
                target: target.to_owned(),
            });
            // For Windows, suppress it for now until we establish some validation rules.
            // let plan_config_path = plan_path.plan_config_path();
            let plan_config = None;

            let mut plan_ctx = PlanContext {
                id,
                repo_id: repo_ctx.id.clone(),
                is_native: repo_ctx.is_native_plan(plan_ctx_path),
                context_path: plan_ctx_path.clone(),
                target_context_last_modified_at: plan_target_ctx_path.last_modifed_at()?,
                target_context_path: plan_target_ctx_path.clone(),
                plan_path: plan_path.clone(),
                source: raw_data.source,
                licenses: raw_data.licenses,
                deps: raw_data
                    .deps
                    .into_iter()
                    .map(|d| d.to_resolved_dep_ident(target.to_owned()))
                    .collect(),
                build_deps: raw_data
                    .build_deps
                    .into_iter()
                    .chain(raw_data.scaffolding_dep)
                    .map(|d| d.to_resolved_dep_ident(target.to_owned()))
                    .collect(),
                latest_artifact: None,
                files_changed_on_disk: Vec::new(),
                files_changed_on_git: Vec::new(),
                plan_config,
            };
            let latest_artifact = artifact_cache.latest_plan_minimal_artifact(&plan_ctx.id);
            plan_ctx.determine_changes(
                connection,
                modification_index,
                latest_artifact.as_ref(),
                change_detection_mode,
            )?;
            trace!(
                "Read plan context {} from disk in {}s",
                plan_ctx.context_path.as_ref().display(),
                start.elapsed().as_secs_f32()
            );
            Ok(plan_ctx)
        } else {
            Err(eyre!(
                "Failed to extract plan data from {}, bash process exited with code: {}",
                plan_path.as_ref().display(),
                output.status,
            )
            .with_section(move || stdout.header("stdout: "))
            .with_section(move || stderr.header("stderr: ")))
        }
    }

    pub fn determine_changes(
        &mut self,
        mut connection: Option<&mut SqliteConnection>,
        modification_index: Option<&ModificationIndex>,
        artifact_ctx: Option<&MinimalArtifactContext>,
        change_detection_mode: ChangeDetectionMode,
    ) -> Result<()> {
        let plan_ctx_walker = WalkBuilder::new(self.context_path.as_ref())
            .standard_filters(false)
            .sort_by_file_path(|a, b| a.cmp(b))
            .build();
        self.files_changed_on_disk = Vec::new();
        self.latest_artifact = artifact_ctx.map(|artifact_ctx| PlanContextLatestArtifact {
            created_at: artifact_ctx.created_at,
            ident: artifact_ctx.id.clone(),
        });
        // Is the plan a top level plan in the same folder as the plan context?
        let is_in_top_level_dir = self.target_context_path.as_ref() == self.context_path.as_ref();

        for entry in plan_ctx_walker {
            match entry {
                Ok(entry) => {
                    // Is this inside the plan's target folder
                    let is_in_target_dir = entry
                        .path()
                        .strip_prefix(self.target_context_path.as_ref())
                        .is_ok();
                    // Is this inside a habitat or platform folder ?
                    let is_in_habitat_dir = entry
                        .path()
                        .strip_prefix(self.context_path.as_ref())
                        .ok()
                        .and_then(|p| p.components().next())
                        .and_then(|p| p.as_os_str().to_str())
                        .map_or(false, |p| p == "habitat" || PackageTarget::parse(p).is_ok());
                    let is_plan_config = if let Some(file_name) = entry.path().file_name() {
                        file_name == PLAN_CONFIG_FILE
                    } else {
                        false
                    };
                    if !is_in_top_level_dir && is_in_habitat_dir && !is_in_target_dir {
                        continue;
                    }
                    if is_in_target_dir && is_plan_config {
                        continue;
                    }

                    match entry.path().last_modifed_at() {
                        Ok(real_last_modified_at) => {
                            if entry.path() == self.target_context_path.as_ref() {
                                self.target_context_last_modified_at = real_last_modified_at;
                            }
                            let alternate_modified_at =
                                if let Some(connection) = connection.as_mut() {
                                    store::file_alternate_modified_at_get(
                                        connection,
                                        &self.context_path,
                                        entry.path(),
                                        real_last_modified_at,
                                    )?
                                } else if let Some(modification_index) = modification_index {
                                    modification_index.file_alternate_modified_at_get(
                                        &self.context_path,
                                        entry.path(),
                                        real_last_modified_at,
                                    )
                                } else {
                                    panic!("No modification source provided")
                                };
                            let modified_at =
                                if let Some(alternate_modified_at) = alternate_modified_at {
                                    alternate_modified_at
                                } else {
                                    real_last_modified_at
                                };
                            let git_modified_at: Option<DateTime<Utc>> = if change_detection_mode
                                == ChangeDetectionMode::Git
                            {
                                let child = std::process::Command::new("git")
                                    .arg("log")
                                    .arg("-1")
                                    .arg("--pretty=%ci")
                                    .arg(entry.path())
                                    .stdin(Stdio::null())
                                    .stdout(Stdio::piped())
                                    .stderr(Stdio::piped())
                                    .current_dir(self.context_path.as_ref())
                                    .spawn()?;
                                let output = child.wait_with_output()?;
                                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                                DateTime::parse_from_str(stdout.trim(), "%Y-%m-%d %H:%M:%S %z")
                                    .ok()
                                    .map(|value| {
                                        DateTime::from_naive_utc_and_offset(value.naive_utc(), Utc)
                                    })
                            } else {
                                None
                            };

                            if let Some(artifact_ctx) = artifact_ctx {
                                if modified_at > artifact_ctx.created_at {
                                    self.files_changed_on_disk
                                        .push(PlanContextFileChangeOnDisk {
                                            last_modified_at: modified_at,
                                            real_last_modified_at,
                                            path: PlanContextFilePath(entry.path().to_path_buf()),
                                        })
                                }
                                if change_detection_mode == ChangeDetectionMode::Git {
                                    if let Some(modified_at) = git_modified_at {
                                        if modified_at > artifact_ctx.created_at {
                                            self.files_changed_on_git.push(
                                                PlanContextFileChangeOnGit {
                                                    last_modified_at: modified_at,
                                                    path: PlanContextFilePath(
                                                        entry.path().to_path_buf(),
                                                    ),
                                                },
                                            )
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            error!(
                                "Failed to read last modified time for entry '{}' in plan context: {}",
                                entry.path().display(),
                                err
                            );
                        }
                    }
                }
                Err(err) => {
                    error!("Failed to read entry in plan context: {}", err);
                }
            }
        }
        Ok(())
    }

    pub fn sync_changes_with_git(
        &mut self,
        is_dry_run: bool,
    ) -> Result<Vec<PlanContextPathGitSyncStatus>> {
        let mut results = Vec::new();
        let plan_ctx_walker = WalkBuilder::new(self.context_path.as_ref())
            .standard_filters(false)
            .sort_by_file_path(|a, b| a.cmp(b))
            .build();
        for entry in plan_ctx_walker {
            match entry {
                Ok(entry) => {
                    let disk_modified_at = entry.path().last_modifed_at()?;
                    let is_locally_modified = {
                        let mut child = std::process::Command::new("git")
                            .arg("diff")
                            .arg("--quiet")
                            .arg("--exit-code")
                            .arg(entry.path())
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .current_dir(self.context_path.as_ref())
                            .spawn()?;
                        let exit_status = child.wait()?;
                        !exit_status.success()
                    };
                    if !is_locally_modified {
                        let git_modified_at: Option<DateTime<Utc>> = {
                            let child = std::process::Command::new("git")
                                .arg("log")
                                .arg("-1")
                                .arg("--pretty=%ci")
                                .arg(entry.path())
                                .stdin(Stdio::null())
                                .stdout(Stdio::piped())
                                .stderr(Stdio::piped())
                                .current_dir(self.context_path.as_ref())
                                .spawn()?;
                            let output = child.wait_with_output()?;
                            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            DateTime::parse_from_str(stdout.trim(), "%Y-%m-%d %H:%M:%S %z")
                                .ok()
                                .map(|value| {
                                    DateTime::from_naive_utc_and_offset(value.naive_utc(), Utc)
                                })
                        };
                        if let Some(git_modified_at) = git_modified_at {
                            if git_modified_at != disk_modified_at {
                                if !is_dry_run {
                                    entry.path().set_last_modifed_at(git_modified_at)?;
                                }
                                results.push(PlanContextPathGitSyncStatus::Synced(
                                    Path::new(".")
                                        .join(entry.path().strip_prefix(&self.context_path)?),
                                    disk_modified_at,
                                    git_modified_at,
                                ));
                            }
                        }
                    } else {
                        results.push(PlanContextPathGitSyncStatus::LocallyModified(
                            Path::new(".").join(entry.path().strip_prefix(&self.context_path)?),
                            disk_modified_at,
                        ));
                    }
                }
                Err(err) => {
                    error!("Failed to read entry in plan context: {}", err);
                }
            }
        }
        Ok(results)
    }
}

pub(crate) struct PlanScanner<'a> {
    repos: &'a HashMap<RepoContextID, RepoContext>,
    modification_index: &'a ModificationIndex,
    artifact_cache: &'a ArtifactCache,
    change_detection_mode: ChangeDetectionMode,
    sender: Sender<PlanContext>,
}

impl<'a> ParallelVisitor for PlanScanner<'a> {
    fn visit(
        &mut self,
        entry: std::result::Result<ignore::DirEntry, ignore::Error>,
    ) -> ignore::WalkState {
        if let Ok(entry) = entry {
            let base_dir = entry.path();
            if !base_dir.is_dir() {
                return WalkState::Continue;
            }
            let mut is_plan_ctx = false;
            for (plan_rel_path, plan_target) in RELATIVE_PLAN_FILE_PATHS.iter() {
                // println!("Plan rel path {:?} and target {:?}", plan_rel_path, plan_target);
                let plan_path = base_dir.join(plan_rel_path);
                if plan_path.is_file() {
                    is_plan_ctx = true;
                    let (_, repo_ctx) = self
                        .repos
                        .iter()
                        .find(|(_, repo_ctx)| plan_path.starts_with(repo_ctx.path.as_ref()))
                        .expect("Plan can only be within a repo folder");
                    let plan_target_ctx_path = PlanTargetContextPath(
                        plan_path
                            .parent()
                            .expect("Failed to determine plan's parent path")
                            .to_path_buf(),
                    );
                    let plan_ctx_path = PlanContextPath(base_dir.into());
                    let plan_path = PlanFilePath(plan_path);
                    debug!(
                        "Plan found at {} in context {}",
                        plan_path.as_ref().display(),
                        plan_ctx_path.as_ref().display()
                    );
                    if repo_ctx.is_ignored_plan(&plan_ctx_path) {
                        continue;
                    }
                    match PlanContext::read_from_disk(
                        None,
                        Some(self.modification_index),
                        repo_ctx,
                        self.artifact_cache,
                        &plan_ctx_path,
                        &plan_target_ctx_path,
                        &plan_path,
                        plan_target.to_owned(),
                        self.change_detection_mode,
                    ) {
                        Ok(plan_ctx) => {
                            self.sender
                                .send(plan_ctx)
                                .expect("Failed to send PlanContext to parent thread");
                        }
                        Err(err) => {
                            info!(target: "user-ui", "{} Failed to extract plan metadata from {}: {:?}", "error:".bold().red(), plan_path.as_ref().strip_prefix(repo_ctx.path.as_ref()).unwrap().display(), err);
                        }
                    };
                }
            }
            if is_plan_ctx {
                WalkState::Skip
            } else {
                WalkState::Continue
            }
        } else {
            WalkState::Continue
        }
    }
}

pub(crate) struct PlanScannerBuilder<'a> {
    repos: &'a HashMap<RepoContextID, RepoContext>,
    modification_index: &'a ModificationIndex,
    artifact_cache: &'a ArtifactCache,
    change_detection_mode: ChangeDetectionMode,
    sender: Sender<PlanContext>,
}

impl<'s, 'a> ParallelVisitorBuilder<'s> for PlanScannerBuilder<'a>
where
    'a: 's,
{
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(PlanScanner {
            repos: self.repos,
            modification_index: self.modification_index,
            artifact_cache: self.artifact_cache,
            change_detection_mode: self.change_detection_mode,
            sender: self.sender.clone(),
        })
    }
}

impl<'a> PlanScannerBuilder<'a> {
    pub fn new(
        repos: &'a HashMap<RepoContextID, RepoContext>,
        modification_index: &'a ModificationIndex,
        artifact_cache: &'a ArtifactCache,
        change_detection_mode: ChangeDetectionMode,
        sender: Sender<PlanContext>,
    ) -> PlanScannerBuilder<'a> {
        PlanScannerBuilder {
            repos,
            modification_index,
            artifact_cache,
            change_detection_mode,
            sender,
        }
    }
}
