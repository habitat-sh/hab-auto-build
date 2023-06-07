use crate::{check::PlanContextConfig, core::PackageTarget, store::Store};
use chrono::{DateTime, Utc};
use color_eyre::eyre::{eyre, Context, Result};
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::SystemTime,
};
use subprocess::{Exec, ExitStatus, NullFile, Redirection};
use tempdir::TempDir;
use thiserror::Error;
use tracing::{debug, error, trace};
use which::which;

use super::{
    ArtifactCache, ArtifactCachePath, ArtifactContext, ArtifactPath, BuildStep, FSRootPath,
    HabitatRootPath, HabitatSourceCachePath, PlanContextID,
};

pub(crate) fn install_artifact(artifact_path: &ArtifactPath) -> Result<()> {
    let exit_status = std::process::Command::new("sudo")
        .arg("-E")
        .arg("hab")
        .arg("pkg")
        .arg("install")
        .arg(artifact_path.as_ref())
        .env("HAB_LICENSE", "accept-no-persist")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to invoke hab pkg install command")
        .wait()?;
    if exit_status.success() {
        Ok(())
    } else if let Some(1) = exit_status.code() {
        Ok(())
    } else {
        Err(eyre!(
            "Failed to install package, exit code: {:?}",
            exit_status.code()
        ))
    }
}

fn copy_source_to_cache(
    build_step: &BuildStep,
    store: &Store,
    source_cache_folder: &HabitatSourceCachePath,
) -> Result<()> {
    if let Some(source) = &build_step.plan_ctx.source {
        std::fs::create_dir_all(source_cache_folder.as_ref()).with_context(|| {
            format!(
                "Failed to create source cache folder at '{}'",
                source_cache_folder.as_ref().display()
            )
        })?;
        let store_archive = store.package_source_store_path(&source).archive_data_path();
        let source_cache_path = source_cache_folder.as_ref().join(source.url.filename()?);
        if !source_cache_path.exists() {
            trace!(
                "Copying downloaded source from {} to {} for build",
                store_archive.as_ref().display(),
                source_cache_path.display()
            );
            std::fs::copy(store_archive.as_ref(), source_cache_path.as_path()).with_context(
                || {
                    format!(
                        "Failed to copy source from {} to {} for build",
                        store_archive.as_ref().display(),
                        source_cache_path.display()
                    )
                },
            )?;
            debug!(
                "Copied downloaded source from {} to {} for build",
                store_archive.as_ref().display(),
                source_cache_path.display()
            );
        } else {
            debug!(
                "Downloaded source already present at {} for build",
                source_cache_path.display()
            );
        }
    }
    Ok(())
}

fn copy_build_success_output(
    store: &Store,
    _build_step: &BuildStep,
    build_log_path: impl AsRef<Path>,
    build_output_path: impl AsRef<Path>,
) -> Result<(PathBuf, PathBuf)> {
    let last_build_path = build_output_path.as_ref().join("last_build.env");
    let last_build = std::fs::read_to_string(&last_build_path).with_context(|| {
        format!(
            "Failed to read last build file at '{}'",
            last_build_path.display()
        )
    })?;
    let artifact_name = last_build
        .lines()
        .into_iter()
        .filter_map(|l| l.strip_prefix("pkg_artifact="))
        .next()
        .unwrap()
        .trim();
    let final_build_artifacts_dir_path = store.package_build_artifacts_path();
    std::fs::create_dir_all(final_build_artifacts_dir_path.as_ref()).with_context(|| {
        format!(
            "Failed to create build artifact directory at '{}'",
            final_build_artifacts_dir_path.as_ref().display()
        )
    })?;
    let artifact_path = build_output_path.as_ref().join(artifact_name);
    let final_artifact_path = final_build_artifacts_dir_path.as_ref().join(artifact_name);

    let final_build_log_dir_path = store.package_build_success_logs_path();
    std::fs::create_dir_all(final_build_log_dir_path.as_ref()).with_context(|| {
        format!(
            "Failed to create build log directory at '{}'",
            final_build_log_dir_path.as_ref().display()
        )
    })?;
    let final_build_log_path = final_build_log_dir_path.as_ref().join(format!(
        "{}.log",
        artifact_name.strip_suffix(".hart").unwrap()
    ));
    debug!(
        "Moving build log from {} to {}",
        build_log_path.as_ref().display(),
        final_build_log_path.display()
    );
    std::fs::rename(build_log_path.as_ref(), final_build_log_path.as_path()).with_context(
        || {
            format!(
                "Failed to move build log from {} to {}",
                build_log_path.as_ref().display(),
                final_build_log_path.display()
            )
        },
    )?;
    debug!(
        "Moving build artifact from {} to {}",
        artifact_path.display(),
        final_artifact_path.display()
    );
    std::fs::rename(artifact_path.as_path(), final_artifact_path.as_path()).with_context(|| {
        format!(
            "Failed to move build artifact from {} to {}",
            artifact_path.display(),
            final_artifact_path.display()
        )
    })?;
    Ok((final_artifact_path, final_build_log_path))
}

fn copy_build_failure_output(
    store: &Store,
    build_step: &BuildStep,
    build_log_path: impl AsRef<Path>,
    build_output_path: impl AsRef<Path>,
) -> Result<PathBuf> {
    let pre_build_path = build_output_path.as_ref().join("pre_build.env");
    let pkg_ident = match std::fs::read_to_string(&pre_build_path).with_context(|| {
        format!(
            "Failed to read pre build file at '{}'",
            pre_build_path.display()
        )
    }) {
        Ok(pre_build) => pre_build
            .lines()
            .into_iter()
            .filter_map(|l| l.strip_prefix("pkg_ident="))
            .next()
            .unwrap()
            .trim()
            .replace("/", "-"),
        Err(err) => {
            debug!("Failed to find pre_build file: {:#}", err);
            let build_id = build_step.plan_ctx.id.as_ref();
            format!(
                "{}-{}-{}-{}",
                build_id.origin,
                build_id.name,
                build_id.version,
                Utc::now().format("%Y%m%d%H%M%S")
            )
        }
    };
    let final_build_log_dir_path = store.package_build_failure_logs_path();
    std::fs::create_dir_all(final_build_log_dir_path.as_ref()).with_context(|| {
        format!(
            "Failed to create build log directory at '{}'",
            final_build_log_dir_path.as_ref().display()
        )
    })?;
    let final_build_log_path = final_build_log_dir_path.as_ref().join(format!(
        "{}-{}.log",
        pkg_ident,
        build_step.plan_ctx.id.as_ref().target
    ));
    debug!(
        "Moving build log from {} to {}",
        build_log_path.as_ref().display(),
        final_build_log_path.display()
    );
    std::fs::rename(build_log_path.as_ref(), final_build_log_path.as_path()).with_context(
        || {
            format!(
                "Failed to move build log from {} to {}",
                build_log_path.as_ref().display(),
                final_build_log_path.display()
            )
        },
    )?;
    Ok(final_build_log_path)
}

pub(crate) struct BuildOutput {
    pub artifact: ArtifactContext,
    pub build_log: PathBuf,
}

#[derive(Debug, Error)]
pub(crate) enum BuildError {
    #[error("Failed to build native package {0}, you can find the build log at {1}")]
    Native(PlanContextID, PathBuf),
    #[error("Failed to build bootstrap package {0}, you can find the build log at {1}")]
    Bootstrap(PlanContextID, PathBuf),
    #[error("Failed to build standard package {0}, you can find the build log at {1}")]
    Standard(PlanContextID, PathBuf),
    #[error("Failed due to unexpected IO error")]
    IO(#[from] std::io::Error),
    #[error("Failed due to unexpected sub process error")]
    Popen(#[from] subprocess::PopenError),
    #[error("Failed due to an unexpected build error")]
    Unexpected(#[from] color_eyre::eyre::Error),
}

pub(crate) fn native_package_build(
    build_step: &BuildStep,
    artifact_cache: &ArtifactCache,
    store: &Store,
) -> Result<BuildOutput, BuildError> {
    let tmp_path = store.temp_dir_path();
    std::fs::create_dir_all(tmp_path.as_ref())?;
    let tmp_dir = TempDir::new_in(tmp_path.as_ref(), "native-build").with_context(|| {
        format!(
            "Failed to create temporary directory in hab-auto-build store at '{}'",
            tmp_path.as_ref().display()
        )
    })?;

    let build_log_path = tmp_dir.path().join(format!("build.log"));
    let build_log = std::fs::File::create(&build_log_path).with_context(|| {
        format!(
            "Failed to create build log at '{}'",
            build_log_path.display()
        )
    })?;
    let build_output_dir = tmp_dir.path();
    let relative_plan_context = build_step
        .plan_ctx
        .context_path
        .as_ref()
        .strip_prefix(&build_step.repo_ctx.path)
        .unwrap();

    let mut cmd;
    let mut exit_status;
    if let Some(PlanContextConfig {
        docker_image: Some(docker_image),
        ..
    }) = &build_step.plan_ctx.plan_config
    {
        debug!(
            "Starting build of native package {} with image {}, logging output to {}",
            relative_plan_context.display(),
            docker_image,
            build_log_path.display()
        );
        let hab_binary = which("hab").with_context(|| {
            format!(
                "Failed to find hab binary to build native package {}",
                build_step.plan_ctx.id
            )
        })?;
        let deps_to_install = build_step
            .deps_to_install
            .iter()
            .filter_map(|dep| artifact_cache.latest_plan_artifact(dep))
            .map(|artifact| {
                format!(
                    "{}",
                    ArtifactCachePath::new(HabitatRootPath::default())
                        .as_ref()
                        .join(artifact.id.artifact_name())
                        .display(),
                )
            })
            .collect::<Vec<String>>()
            .join(":");
        let container_name = tmp_dir.path().file_name().unwrap();
        cmd = Exec::cmd("docker")
            .arg("run")
            .arg("-it")
            .arg("--name")
            .arg(container_name)
            .arg("--rm")
            .arg("-v")
            .arg(format!(
                "{}:/src",
                build_step.repo_ctx.path.as_ref().display()
            ));
        if let Some(source) = &build_step.plan_ctx.source {
            let source_cache_folder = HabitatRootPath::default().source_cache();
            let store_archive = store.package_source_store_path(&source).archive_data_path();
            let source_cache_path = source_cache_folder.as_ref().join(source.url.filename()?);
            cmd = cmd.arg("-v").arg(format!(
                "{}:{}",
                store_archive.as_ref().display(),
                source_cache_path.display()
            ));
        }
        if !build_step.allow_remote {
            cmd = cmd.arg("-e").arg("HAB_BLDR_URL=https://non-existent");
        }
        cmd = cmd
            .arg("-v")
            .arg(format!("{}:/bin/hab", hab_binary.display()))
            .arg("-v")
            .arg(format!("{}:/output", build_output_dir.display()))
            .arg("-v")
            .arg(format!("/hab/cache/artifacts:/hab/cache/artifacts"))
            .arg("-v")
            .arg(format!("/hab/cache/keys:/hab/cache/keys"))
            .arg("--workdir")
            .arg("/src")
            .arg("-e")
            .arg(format!("HAB_STUDIO_INSTALL_PKGS={}", deps_to_install))
            .arg("-e")
            .arg("NO_INSTALL_DEPS=1")
            .arg("-e")
            .arg("HAB_LICENSE=accept")
            .arg("-e")
            .arg("HAB_FEAT_NATIVE_PACKAGE_SUPPORT=1")
            .arg("-e")
            .arg("HAB_OUTPUT_PATH=/output")
            .arg("-e")
            .arg(format!(
                "BUILD_PKG_TARGET={}",
                PackageTarget::default().to_string()
            ))
            .arg(docker_image)
            .arg("build")
            .arg(relative_plan_context)
            .cwd(build_step.repo_ctx.path.as_ref())
            .stdin(Redirection::None)
            .stdout(Redirection::File(build_log))
            .stderr(Redirection::Merge);
        trace!("Executing command: {:?}", cmd);
        exit_status = cmd.join()?;
    } else {
        debug!(
            "Starting build of native package {}, logging output to {}",
            relative_plan_context.display(),
            build_log_path.display()
        );
        copy_source_to_cache(
            build_step,
            store,
            &HabitatRootPath::default().source_cache(),
        )?;
        cmd = Exec::cmd("sudo")
            .arg("-E")
            .arg("env")
            .arg(format!("PATH={}", env::var("PATH").unwrap_or_default()))
            .arg("hab")
            .arg("pkg")
            .arg("build")
            .arg("-N")
            .arg(relative_plan_context)
            .env("HAB_FEAT_NATIVE_PACKAGE_SUPPORT", "1")
            .env("HAB_OUTPUT_PATH", tmp_dir.path())
            .env("BUILD_PKG_TARGET", PackageTarget::default().to_string())
            .cwd(build_step.repo_ctx.path.as_ref())
            .stdin(NullFile)
            .stdout(Redirection::File(build_log))
            .stderr(Redirection::Merge);
        if !build_step.allow_remote {
            cmd = cmd.env("HAB_BLDR_URL", "https://non-existent");
        }
        trace!("Executing command: {:?}", cmd);
        exit_status = cmd.join()?;
    }

    if exit_status.success() {
        let (artifact_path, build_log_path) =
            copy_build_success_output(store, build_step, &build_log_path, build_output_dir)?;
        Ok(BuildOutput {
            artifact: ArtifactContext::read_from_disk(artifact_path.as_path(), None).with_context(
                || {
                    format!(
                        "Failed to index built artifact: {}",
                        artifact_path.display()
                    )
                },
            )?,
            build_log: build_log_path,
        })
    } else {
        let build_log_path =
            copy_build_failure_output(store, build_step, &build_log_path, build_output_dir)?;
        Err(BuildError::Native(
            build_step.plan_ctx.id.clone(),
            build_log_path,
        ))
    }
}

pub(crate) fn bootstrap_package_build(
    build_step: &BuildStep,
    artifact_cache: &ArtifactCache,
    store: &Store,
    id: u64,
) -> Result<BuildOutput, BuildError> {
    let tmp_path = store.temp_dir_path();
    std::fs::create_dir_all(tmp_path.as_ref())?;
    let tmp_dir = TempDir::new_in(tmp_path.as_ref(), "bootstrap-build").with_context(|| {
        format!(
            "Failed to create temporary directory in hab-auto-build store at '{}'",
            tmp_path.as_ref().display()
        )
    })?;
    let build_log_path = tmp_dir.path().join(format!("build.log"));
    let build_log = std::fs::File::create(&build_log_path).with_context(|| {
        format!(
            "Failed to create build log at '{}'",
            build_log_path.display()
        )
    })?;
    let studio_root = HabitatRootPath::new(FSRootPath::default())
        .studio_root(format!("hab-auto-build-{}", id).as_str());
    let build_output_dir = studio_root.as_ref().join("output");
    let deps_to_install = build_step
        .deps_to_install
        .iter()
        .filter_map(|dep| artifact_cache.latest_plan_artifact(dep))
        .map(|artifact| {
            format!(
                "{}",
                ArtifactCachePath::new(HabitatRootPath::default())
                    .as_ref()
                    .join(artifact.id.artifact_name())
                    .display(),
            )
        })
        .collect::<Vec<String>>()
        .join(":");
    let origin_keys = build_step
        .origins
        .iter()
        .map(|origin| origin.to_string())
        .collect::<Vec<String>>()
        .join(",");
    let relative_plan_context =
        if build_step.plan_ctx.context_path.as_ref() == build_step.repo_ctx.path.as_ref() {
            PathBuf::from(".")
        } else {
            build_step
                .plan_ctx
                .context_path
                .as_ref()
                .strip_prefix(&build_step.repo_ctx.path)
                .unwrap()
                .to_path_buf()
        };

    debug!(
        "Starting build of bootstrap package {} with studio package {}, logging output to {}",
        relative_plan_context.display(),
        build_step.studio_package.unwrap(),
        build_log_path.display()
    );

    let exit_status = Exec::cmd("sudo")
        .arg("-E")
        .arg("hab")
        .arg("pkg")
        .arg("exec")
        .arg(build_step.studio_package.unwrap().to_string())
        .arg("hab-studio")
        .arg("--")
        .arg("-r")
        .arg(studio_root.as_ref())
        .arg("rm")
        .env("HAB_LICENSE", "accept-no-persist")
        .cwd(build_step.repo_ctx.path.as_ref())
        .stdin(NullFile)
        .stdout(Redirection::File(build_log))
        .stderr(Redirection::Merge)
        .join()?;
    if !exit_status.success() {
        let build_log_path =
            copy_build_failure_output(store, build_step, &build_log_path, &build_output_dir)?;
        return Err(eyre!(
            "Failed to cleanup bootstrap studio at '{}', you can find the build log at {}",
            studio_root.as_ref().display(),
            build_log_path.display()
        )
        .into());
    }

    let build_log = std::fs::File::options()
        .append(true)
        .open(&build_log_path)
        .with_context(|| {
            format!(
                "Failed to append to build log at '{}'",
                build_log_path.display()
            )
        })?;

    copy_source_to_cache(
        build_step,
        store,
        &HabitatRootPath::new(FSRootPath::from(studio_root.clone())).source_cache(),
    )?;

    let mut cmd = Exec::cmd("sudo")
        .arg("-E")
        .arg("hab")
        .arg("pkg")
        .arg("exec")
        .arg(build_step.studio_package.unwrap().to_string())
        .arg("hab-studio")
        .arg("--")
        .arg("-t")
        .arg("bootstrap")
        .arg("-r")
        .arg(studio_root.as_ref())
        .arg("build")
        .arg("-R")
        .arg(relative_plan_context)
        .env("HAB_ORIGIN_KEYS", origin_keys)
        .env("HAB_LICENSE", "accept-no-persist")
        .env("HAB_STUDIO_SUP", "false")
        .env("HAB_STUDIO_INSTALL_PKGS", deps_to_install)
        .env("HAB_STUDIO_SECRET_STUDIO_ENTER", "1")
        .env("HAB_STUDIO_SECRET_HAB_OUTPUT_PATH", "/output")
        .env("HAB_STUDIO_SECRET_NO_INSTALL_DEPS", "1")
        .cwd(build_step.repo_ctx.path.as_ref())
        .stdin(NullFile)
        .stdout(Redirection::File(build_log))
        .stderr(Redirection::Merge);
    if !build_step.allow_remote {
        cmd = cmd.env("HAB_BLDR_URL", "https://non-existent");
    }
    trace!("Executing command: {:?}", cmd);
    let exit_status = cmd.join()?;
    if exit_status.success() {
        let (artifact_path, build_log_path) =
            copy_build_success_output(store, build_step, &build_log_path, &build_output_dir)?;
        Ok(BuildOutput {
            artifact: ArtifactContext::read_from_disk(artifact_path.as_path(), None).with_context(
                || {
                    format!(
                        "Failed to index built artifact: {}",
                        artifact_path.display()
                    )
                },
            )?,
            build_log: build_log_path,
        })
    } else {
        let build_log_path =
            copy_build_failure_output(store, build_step, &build_log_path, &build_output_dir)?;
        Err(BuildError::Bootstrap(
            build_step.plan_ctx.id.clone(),
            build_log_path,
        ))
    }
}

pub(crate) fn standard_package_build(
    build_step: &BuildStep,
    artifact_cache: &ArtifactCache,
    store: &Store,
    id: u64,
) -> Result<BuildOutput, BuildError> {
    let tmp_path = store.temp_dir_path();
    std::fs::create_dir_all(tmp_path.as_ref())?;
    let tmp_dir = TempDir::new_in(tmp_path.as_ref(), "standard-build").with_context(|| {
        format!(
            "Failed to create temporary directory in hab-auto-build store at '{}'",
            tmp_path.as_ref().display()
        )
    })?;
    let build_log_path = tmp_dir.path().join(format!("build.log"));
    let build_log = std::fs::File::create(&build_log_path).with_context(|| {
        format!(
            "Failed to create build log at '{}'",
            build_log_path.display()
        )
    })?;
    let studio_root = HabitatRootPath::new(FSRootPath::default())
        .studio_root(format!("hab-auto-build-{}", id).as_str());
    let build_output_dir = studio_root.as_ref().join("output");
    let deps_to_install = build_step
        .deps_to_install
        .iter()
        .filter_map(|dep| artifact_cache.latest_plan_artifact(dep))
        .map(|artifact| {
            format!(
                "{}",
                ArtifactCachePath::new(HabitatRootPath::default())
                    .as_ref()
                    .join(artifact.id.artifact_name())
                    .display(),
            )
        })
        .collect::<Vec<String>>()
        .join(":");
    let origin_keys = build_step
        .origins
        .iter()
        .map(|origin| origin.to_string())
        .collect::<Vec<String>>()
        .join(",");
    let relative_plan_context =
        if build_step.plan_ctx.context_path.as_ref() == build_step.repo_ctx.path.as_ref() {
            PathBuf::from(".")
        } else {
            build_step
                .plan_ctx
                .context_path
                .as_ref()
                .strip_prefix(&build_step.repo_ctx.path)
                .unwrap()
                .to_path_buf()
        };

    debug!(
        "Starting build of standard package {} with studio package {}, logging output to {}",
        relative_plan_context.display(),
        build_step.studio_package.unwrap(),
        build_log_path.display()
    );

    let cmd = Exec::cmd("sudo")
        .arg("-E")
        .arg("hab")
        .arg("pkg")
        .arg("exec")
        .arg(build_step.studio_package.unwrap().to_string())
        .arg("hab-studio")
        .arg("--")
        .arg("-r")
        .arg(studio_root.as_ref())
        .arg("rm")
        .env("HAB_LICENSE", "accept-no-persist")
        .cwd(build_step.repo_ctx.path.as_ref())
        .stdin(NullFile)
        .stdout(Redirection::File(build_log))
        .stderr(Redirection::Merge);
    let exit_status = cmd.join()?;

    if !exit_status.success() {
        let build_log_path =
            copy_build_failure_output(store, build_step, &build_log_path, &build_output_dir)?;
        return Err(eyre!(
            "Failed to cleanup standard studio at '{}', you can find the build log at {}",
            studio_root.as_ref().display(),
            build_log_path.display()
        )
        .into());
    }

    let build_log = std::fs::File::options()
        .append(true)
        .open(&build_log_path)
        .with_context(|| {
            format!(
                "Failed to append to build log at '{}'",
                build_log_path.display()
            )
        })?;

    copy_source_to_cache(
        build_step,
        store,
        &HabitatRootPath::new(FSRootPath::from(studio_root.clone())).source_cache(),
    )?;
    let mut cmd = Exec::cmd("sudo")
        .arg("-E")
        .arg("hab")
        .arg("pkg")
        .arg("exec")
        .arg(build_step.studio_package.unwrap().to_string())
        .arg("hab-studio")
        .arg("--")
        .arg("-r")
        .arg(studio_root.as_ref())
        .arg("build")
        .arg("-R")
        .arg(relative_plan_context)
        .env("HAB_ORIGIN_KEYS", origin_keys)
        .env("HAB_LICENSE", "accept-no-persist")
        .env("HAB_STUDIO_INSTALL_PKGS", deps_to_install)
        .env("HAB_STUDIO_SUP", "false")
        .env("HAB_STUDIO_SECRET_STUDIO_ENTER", "1")
        .env("HAB_STUDIO_SECRET_HAB_OUTPUT_PATH", "/output")
        .env("HAB_STUDIO_SECRET_NO_INSTALL_DEPS", "1")
        .cwd(build_step.repo_ctx.path.as_ref())
        .stdin(NullFile)
        .stdout(Redirection::File(build_log))
        .stderr(Redirection::Merge);
    if !build_step.allow_remote {
        cmd = cmd.env("HAB_BLDR_URL", "https://non-existent");
    }
    trace!("Executing command: {:?}", cmd);
    let exit_status = cmd.join()?;

    if exit_status.success() {
        let (artifact_path, build_log_path) =
            copy_build_success_output(store, build_step, &build_log_path, &build_output_dir)?;
        Ok(BuildOutput {
            artifact: ArtifactContext::read_from_disk(artifact_path.as_path(), None).with_context(
                || {
                    format!(
                        "Failed to index built artifact: {}",
                        artifact_path.display()
                    )
                },
            )?,
            build_log: build_log_path,
        })
    } else {
        let build_log_path =
            copy_build_failure_output(store, build_step, &build_log_path, &build_output_dir)?;
        Err(BuildError::Bootstrap(
            build_step.plan_ctx.id.clone(),
            build_log_path,
        ))
    }
}
