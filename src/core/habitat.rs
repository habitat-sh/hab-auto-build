use super::{
    ArtifactCache, ArtifactCachePath, ArtifactContext, BuildStep, FSRootPath, HabitatRootPath,
    HabitatSourceCachePath, PackageIdent, PlanContextID,
};
use crate::{check::PlanContextConfig, core::PackageTarget, store::Store};
use chrono::Utc;
use color_eyre::eyre::{eyre, Context, Result};
use goblin::{
    mach::{Mach, SingleArch},
    Object,
};
use lazy_static::lazy_static;
use std::{
    collections::{BTreeSet, VecDeque},
    env,
    fmt::Write,
    path::{Path, PathBuf},
    process::Stdio,
};
use subprocess::{Exec, NullFile, Redirection};
use tempdir::TempDir;
use thiserror::Error;
use tracing::{debug, error, trace};
use which::which;

lazy_static! {
    pub static ref MACOS_SYSTEM_LIBS: Vec<String> = {
        let sdk_path = PathBuf::from(
            String::from_utf8(
                std::process::Command::new("xcrun")
                    .arg("--show-sdk-path")
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim(),
        );
        let sdk_libs_path = sdk_path.join("usr").join("lib");
        let mut system_libs = Vec::new();
        for entry in std::fs::read_dir(sdk_libs_path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if let Some(extension) = path.extension() {
                if extension == "tbd" {
                    system_libs.push(format!(
                        "/usr/lib/{}.dylib",
                        path.file_stem().unwrap().to_string_lossy()
                    ));
                }
            }
        }
        debug!("Detected System Libraries: {:?}", system_libs);
        system_libs
    };
    pub static ref MACOS_SYSTEM_DIRS: Vec<String> = vec![
        String::from("/System/Library/Frameworks"),
        String::from("/Library/Frameworks"),
        String::from("/Applications/Xcode.app")
    ];
    static ref HAB_BINARY: PathBuf =
        which("hab").expect("Failed to find hab binary in environment");
}

#[allow(dead_code)]
const MACOS_CPU_TYPE: u32 = 16777228;
#[allow(dead_code)]
const MACOS_CPU_SUBTYPE: u32 = 2;
#[allow(dead_code)]
const SANDBOX_DEFAULTS: &str = include_str!("../scripts/sandbox-defaults.sb");

pub(crate) fn install_artifact_offline(package_ident: &PackageIdent) -> Result<()> {
    debug!("Installing habitat package {}", package_ident);
    let exit_status = std::process::Command::new("sudo")
        .arg("-E")
        .arg(HAB_BINARY.as_path())
        .arg("pkg")
        .arg("install")
        .arg(
            ArtifactCachePath::default()
                .artifact_path(package_ident)
                .as_ref(),
        )
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
        let store_archive = store.package_source_store_path(source).archive_data_path();
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

#[cfg(target_os = "windows")]
fn copy_build_success_output(
    store: &Store,
    _build_step: &BuildStep,
    build_log_path: impl AsRef<Path>,
    build_output_path: impl AsRef<Path>,
) -> Result<(PathBuf, PathBuf)> {
    // TODO revisit later
    Ok((PathBuf::new(), PathBuf::new()))
}

#[cfg(not(target_os = "windows"))]
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
            .filter_map(|l| l.strip_prefix("pkg_ident="))
            .next()
            .unwrap()
            .trim()
            .replace('/', "-"),
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
    #[allow(dead_code)]
    Standard(PlanContextID, PathBuf),
    #[error("Failed due to unexpected IO error")]
    IO(#[from] std::io::Error),
    #[error("Failed due to unexpected sub process error")]
    Popen(#[from] subprocess::PopenError),
    #[error("Failed due to an unexpected build error")]
    Unexpected(#[from] color_eyre::eyre::Error),
}

#[cfg(target_os = "linux")]
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

    let build_log_path = tmp_dir.path().join("build.log");
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
    let exit_status;
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
        let deps_to_install = build_step
            .deps_to_install
            .iter()
            .filter_map(|dep| artifact_cache.latest_plan_minimal_artifact(dep))
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
        let container_name = "hab-auto-build-native";
        let container_id_output = std::process::Command::new("docker")
            .args(["ps", "-aqf", &format!("name={}", container_name)])
            .output()?;
        let container_id = String::from_utf8_lossy(&container_id_output.stdout);
        let container_id = container_id.trim();
        if !container_id.is_empty() {
            let exit_status = Exec::cmd("docker").arg("rm").arg(container_name).join()?;
            if !exit_status.success() {
                return Err(BuildError::Unexpected(eyre!(
                    "Failed to remove Docker container '{}'",
                    container_name
                )));
            }
        }

        cmd = Exec::cmd("docker")
            .arg("run")
            .arg("-it")
            .arg("--name")
            .arg(container_name)
            .arg("-v")
            .arg(format!(
                "{}:/src",
                build_step.repo_ctx.path.as_ref().display()
            ));
        if let Some(source) = &build_step.plan_ctx.source {
            let source_cache_folder = HabitatRootPath::default().source_cache();
            let store_archive = store.package_source_store_path(source).archive_data_path();
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
            .arg(format!("{}:/bin/hab", HAB_BINARY.display()))
            .arg("-v")
            .arg(format!("{}:/output", build_output_dir.display()))
            .arg("-v")
            .arg("/hab/cache/artifacts:/hab/cache/artifacts")
            .arg("-v")
            .arg("/hab/cache/keys:/hab/cache/keys")
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
                "HAB_ORIGIN={}",
                build_step.plan_ctx.id.as_ref().origin
            ))
            .arg("-e")
            .arg(format!("BUILD_PKG_TARGET={}", PackageTarget::default()))
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
            .arg(HAB_BINARY.as_path())
            .arg("pkg")
            .arg("build")
            .arg("-N")
            .arg(relative_plan_context)
            .env("HAB_FEAT_NATIVE_PACKAGE_SUPPORT", "1")
            .env("HAB_OUTPUT_PATH", tmp_dir.path())
            .env(
                "HAB_ORIGIN",
                build_step.plan_ctx.id.as_ref().origin.to_string(),
            )
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

#[allow(dead_code)]
fn compute_binary_impurities(binary_path: impl AsRef<Path>) -> Result<BTreeSet<PathBuf>> {
    let mut impure_paths = BTreeSet::new();
    let mut unvisted_paths = VecDeque::new();
    impure_paths.insert(binary_path.as_ref().to_path_buf());
    unvisted_paths.push_back(binary_path.as_ref().to_path_buf());
    while !unvisted_paths.is_empty() {
        if let Some(current_object_path) = unvisted_paths.pop_front() {
            let data = std::fs::read(current_object_path)?;
            let macho = Object::parse(&data)?;
            if let Object::Mach(macho) = macho {
                match macho {
                    Mach::Fat(archs) => {
                        for index in 0..archs.narches {
                            let arch = archs.get(index)?;
                            match arch {
                                SingleArch::MachO(arch) => {
                                    if arch.header.cputype == MACOS_CPU_TYPE
                                        && arch.header.cpusubtype == MACOS_CPU_SUBTYPE
                                    {
                                        for lib in arch.libs {
                                            let library_path = PathBuf::from(lib);
                                            if library_path.is_absolute() {
                                                // if MACOS_SYSTEM_LIBS.contains(&lib) {
                                                //     continue;
                                                // }
                                                impure_paths.insert(library_path.clone());
                                                if !impure_paths.contains(&library_path) {
                                                    unvisted_paths.push_back(library_path.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                                SingleArch::Archive(_) => {}
                            }
                        }
                    }
                    Mach::Binary(arch) => {
                        for lib in arch.libs {
                            let library_path = PathBuf::from(lib);
                            if library_path.is_absolute() {
                                // if MACOS_SYSTEM_LIBS.contains(&lib) {
                                //     continue;
                                // }
                                impure_paths.insert(library_path.clone());
                                if !impure_paths.contains(&library_path) {
                                    unvisted_paths.push_back(library_path.clone());
                                }
                            }
                        }
                    }
                }
            } else {
                return Err(eyre!("Binary is not a valid Mach-O executable"));
            }
        }
    }
    Ok(impure_paths)
}

#[allow(dead_code)]
fn build_sandbox_profile(tmp_dir: impl AsRef<Path>) -> Result<PathBuf> {
    let sandbox_profile_path = tmp_dir.as_ref().join("sandbox-profile.sb");
    let mut sandbox_profile = String::new();
    let mut impure_dirs = BTreeSet::new();
    let runtime_binaries = &["hab", "bash", "env", "basename", "dirname"];
    for runtime_binary in runtime_binaries {
        let binary_path = which(runtime_binary)?;
        impure_dirs.append(&mut compute_binary_impurities(binary_path)?);
    }
    println!("IMPURE DIRS: {:?}", impure_dirs);
    writeln!(&mut sandbox_profile, "(version 1)")?;
    writeln!(&mut sandbox_profile, "(import \"dyld-support.sb\")")?;
    let impure_dirs = impure_dirs
        .into_iter()
        .map(|dir| format!("(literal \"{}\")", dir.display()))
        .collect::<Vec<String>>()
        .join("\n");
    writeln!(
        &mut sandbox_profile,
        "(allow file-read* process-exec {})",
        impure_dirs
    )?;
    // Compute directories in chroot
    write!(&mut sandbox_profile, "{}", SANDBOX_DEFAULTS)?;
    std::fs::write(&sandbox_profile_path, sandbox_profile.as_bytes())?;
    Ok(sandbox_profile_path)
}

#[cfg(target_os = "macos")]
pub(crate) fn native_package_build(
    build_step: &BuildStep,
    _artifact_cache: &ArtifactCache,
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

    let build_log_path = tmp_dir.path().join("build.log");
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

    cmd = Exec::cmd("sudo").arg("-E");

    if let Some(PlanContextConfig {
        sandbox: Some(true),
        ..
    }) = &build_step.plan_ctx.plan_config
    {
        let sandbox_profile = build_sandbox_profile(tmp_path.as_ref())?;
        cmd = cmd
            .arg("sandbox-exec")
            .arg("-f")
            .arg(sandbox_profile)
            .arg("-D")
            .arg(format!("BUILD_DIR={}", "/hab/cache/"))
            .arg("-D")
            .arg(format!("ALLOW_NETWORKING={}", "1"));
    }

    cmd = cmd
        .arg("env")
        .arg(format!("PATH={}", env::var("PATH").unwrap_or_default()))
        .arg("hab")
        .arg("pkg")
        .arg("build")
        .arg("-N")
        .arg(relative_plan_context)
        .env("HAB_LICENSE", "accept-no-persist")
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
    let exit_status = cmd.join()?;

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

#[cfg(target_os = "windows")]
pub(crate) fn native_package_build(
    _build_step: &BuildStep,
    _artifact_cache: &ArtifactCache,
    _store: &Store,
) -> Result<BuildOutput, BuildError> {
    // This should never be called on Windows
    Err(BuildError::Unexpected(eyre!(
        "The function 'native_package_build' should not be called on Windows."
    )))
}

#[cfg(target_os = "linux")]
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
    let build_log_path = tmp_dir.path().join("build.log");
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
        .filter_map(|dep| artifact_cache.latest_plan_minimal_artifact(dep))
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

    install_artifact_offline(
        &artifact_cache
            .latest_minimal_artifact(
                &build_step
                    .studio_package
                    .unwrap()
                    .to_resolved_dep_ident(PackageTarget::default()),
            )
            .unwrap()
            .id,
    )?;

    let exit_status = Exec::cmd("sudo")
        .arg("-E")
        .arg(HAB_BINARY.as_path())
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
        .arg(HAB_BINARY.as_path())
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
        .env(
            "HAB_ORIGIN",
            build_step.plan_ctx.id.as_ref().origin.to_string(),
        )
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

#[cfg(target_os = "macos")]
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
    let build_log_path = tmp_dir.path().join("build.log");
    let _build_log = std::fs::File::create(&build_log_path).with_context(|| {
        format!(
            "Failed to create build log at '{}'",
            build_log_path.display()
        )
    })?;
    let studio_root = HabitatRootPath::new(FSRootPath::default())
        .studio_root(format!("hab-auto-build-{}", id).as_str());

    let build_output_dir = tmp_dir.path();
    let deps_to_install = build_step
        .deps_to_install
        .iter()
        .filter_map(|dep| artifact_cache.latest_plan_minimal_artifact(dep))
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

    install_artifact_offline(
        &artifact_cache
            .latest_minimal_artifact(
                &build_step
                    .studio_package
                    .unwrap()
                    .to_resolved_dep_ident(PackageTarget::default()),
            )
            .unwrap()
            .id,
    )?;

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
        &HabitatRootPath::new(FSRootPath::default()).source_cache(),
    )?;

    let mut cmd = Exec::cmd("sudo")
        .arg("-E")
        .arg(HAB_BINARY.as_path())
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
        .arg(relative_plan_context)
        .env("CERT_PATH", "/hab/cache/ssl")
        .env(
            "ARTIFACT_PATH",
            ArtifactCachePath::new(HabitatRootPath::default()).as_ref(),
        )
        .env("HAB_ORIGIN_KEYS", origin_keys)
        .env("HAB_OUTPUT_PATH", build_output_dir)
        .env("HAB_LICENSE", "accept-no-persist")
        .env("INSTALL_PKGS", deps_to_install)
        .env("NO_INSTALL_DEPS", "1")
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
        Err(BuildError::Bootstrap(
            build_step.plan_ctx.id.clone(),
            build_log_path,
        ))
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn bootstrap_package_build(
    build_step: &BuildStep,
    artifact_cache: &ArtifactCache,
    store: &Store,
    id: u64,
) -> Result<BuildOutput, BuildError> {
    // This should never be called on Windows
    Err(BuildError::Unexpected(eyre!(
        "The function 'bootstrap_package_build' should not be called on Windows."
    )))
}

#[cfg(target_os = "linux")]
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
    let build_log_path = tmp_dir.path().join("build.log");
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
        .filter_map(|dep| artifact_cache.latest_plan_minimal_artifact(dep))
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

    install_artifact_offline(
        &artifact_cache
            .latest_minimal_artifact(
                &build_step
                    .studio_package
                    .unwrap()
                    .to_resolved_dep_ident(PackageTarget::default()),
            )
            .unwrap()
            .id,
    )?;

    let cmd = Exec::cmd("sudo")
        .arg("-E")
        .arg(HAB_BINARY.as_path())
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
        .arg(HAB_BINARY.as_path())
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
        .env(
            "HAB_ORIGIN",
            build_step.plan_ctx.id.as_ref().origin.to_string(),
        )
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
        Err(BuildError::Standard(
            build_step.plan_ctx.id.clone(),
            build_log_path,
        ))
    }
}

#[cfg(target_os = "macos")]
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
    let build_log_path = tmp_dir.path().join("build.log");
    let _build_log = std::fs::File::create(&build_log_path).with_context(|| {
        format!(
            "Failed to create build log at '{}'",
            build_log_path.display()
        )
    })?;
    let studio_root = HabitatRootPath::new(FSRootPath::default())
        .studio_root(format!("hab-auto-build-{}", id).as_str());

    let build_output_dir = tmp_dir.path();
    let deps_to_install = build_step
        .deps_to_install
        .iter()
        .filter_map(|dep| artifact_cache.latest_plan_minimal_artifact(dep))
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

    install_artifact_offline(
        &artifact_cache
            .latest_minimal_artifact(
                &build_step
                    .studio_package
                    .unwrap()
                    .to_resolved_dep_ident(PackageTarget::default()),
            )
            .unwrap()
            .id,
    )?;

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
        &HabitatRootPath::new(FSRootPath::default()).source_cache(),
    )?;

    let mut cmd = Exec::cmd("sudo")
        .arg("-E")
        .arg(HAB_BINARY.as_path())
        .arg("pkg")
        .arg("exec")
        .arg(build_step.studio_package.unwrap().to_string())
        .arg("hab-studio")
        .arg("--")
        .arg("-r")
        .arg(studio_root.as_ref())
        .arg("build")
        .arg(relative_plan_context)
        .env("CERT_PATH", "/hab/cache/ssl")
        .env(
            "ARTIFACT_PATH",
            ArtifactCachePath::new(HabitatRootPath::default()).as_ref(),
        )
        .env("HAB_ORIGIN_KEYS", origin_keys)
        .env("HAB_OUTPUT_PATH", build_output_dir)
        .env("HAB_LICENSE", "accept-no-persist")
        .env("INSTALL_PKGS", deps_to_install)
        .env("NO_INSTALL_DEPS", "1")
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
        Err(BuildError::Bootstrap(
            build_step.plan_ctx.id.clone(),
            build_log_path,
        ))
    }
}

#[cfg(target_os = "windows")]
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
    let build_log_path = tmp_dir.path().join("build.log");
    let _build_log = std::fs::File::create(&build_log_path).with_context(|| {
        format!(
            "Failed to create build log at '{}'",
            build_log_path.display()
        )
    })?;
    // let studio_root = HabitatRootPath::new(FSRootPath::default())
    //     .studio_root(format!("hab-auto-build-{}", id).as_str());

    let build_output_dir = tmp_dir.path();
    let deps_to_install = build_step
        .deps_to_install
        .iter()
        .filter_map(|dep| artifact_cache.latest_plan_minimal_artifact(dep))
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
    // let origin_keys = build_step
    //     .origins
    //     .iter()
    //     .map(|origin| origin.to_string())
    //     .collect::<Vec<String>>()
    //     .join(",");
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

    // install_artifact_offline(
    //     &artifact_cache
    //         .latest_minimal_artifact(
    //             &build_step
    //                 .studio_package
    //                 .unwrap()
    //                 .to_resolved_dep_ident(PackageTarget::default()),
    //         )
    //         .unwrap()
    //         .id,
    // )?;

    let build_log = std::fs::File::options()
        .append(true)
        .open(&build_log_path)
        .with_context(|| {
            format!(
                "Failed to append to build log at '{}'",
                build_log_path.display()
            )
        })?;

    // copy_source_to_cache(
    //     build_step,
    //     store,
    //     &HabitatRootPath::new(FSRootPath::default()).source_cache(),
    // )?;

    let mut cmd = Exec::cmd("hab")
        .arg("pkg")
        .arg("build")
        .arg("--docker")
        .arg(relative_plan_context)
        .env("CERT_PATH", "c:/hab/cache/ssl")
        .env(
            "ARTIFACT_PATH",
            ArtifactCachePath::new(HabitatRootPath::default()).as_ref(),
        )
        .env(
            "HAB_ORIGIN",
            build_step.plan_ctx.id.as_ref().origin.to_string(),
        )
        // .env("HAB_ORIGIN_KEYS", origin_keys)
        .env("HAB_LICENSE", "accept-no-persist")
        .env("INSTALL_PKGS", deps_to_install)
        .env("NO_INSTALL_DEPS", "1")
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
        Err(BuildError::Bootstrap(
            build_step.plan_ctx.id.clone(),
            build_log_path,
        ))
    }
}
