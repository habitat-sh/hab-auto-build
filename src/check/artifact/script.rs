use std::{collections::HashSet, fmt::Display, path::PathBuf};

use owo_colors::OwoColorize;
use path_absolutize::Absolutize;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::{
    check::{
        ArtifactCheck, ArtifactCheckViolation, ArtifactRuleOptions, CheckerContext,
        LeveledArtifactCheckViolation, PlanContextConfig, ViolationLevel,
    },
    core::{ArtifactCache, ArtifactContext, GlobSetExpression, PackageIdent, PackagePath},
    store::Store,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum ScriptRule {
    #[serde(rename = "host-script-interpreter")]
    HostScriptInterpreter(HostScriptInterpreter),
    #[serde(rename = "missing-env-script-interpreter")]
    MissingEnvScriptInterpreter(MissingEnvScriptInterpreter),
    #[serde(rename = "env-script-interpreter-not-found")]
    EnvScriptInterpreterNotFound(EnvScriptInterpreterNotFound),
    #[serde(rename = "script-interpreter-not-found")]
    ScriptInterpreterNotFound(ScriptInterpreterNotFound),
    #[serde(rename = "unlisted-script-interpreter")]
    UnlistedScriptInterpreter(UnlistedScriptInterpreter),
    #[serde(rename = "missing-script-interpreter-dependency")]
    MissingScriptInterpreterDependency(MissingScriptInterpreterDependency),
}

impl Display for ScriptRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptRule::HostScriptInterpreter(rule) => write!(f, "{}", rule),
            ScriptRule::MissingEnvScriptInterpreter(rule) => write!(f, "{}", rule),
            ScriptRule::EnvScriptInterpreterNotFound(rule) => write!(f, "{}", rule),
            ScriptRule::ScriptInterpreterNotFound(rule) => write!(f, "{}", rule),
            ScriptRule::UnlistedScriptInterpreter(rule) => write!(f, "{}", rule),
            ScriptRule::MissingScriptInterpreterDependency(rule) => write!(f, "{}", rule),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "id", content = "options")]
pub(crate) enum ScriptRuleOptions {
    #[serde(rename = "host-script-interpreter")]
    HostScriptInterpreter(HostScriptInterpreterOptions),
    #[serde(rename = "missing-env-script-interpreter")]
    MissingEnvScriptInterpreter(MissingEnvScriptInterpreterOptions),
    #[serde(rename = "env-script-interpreter-not-found")]
    EnvScriptInterpreterNotFound(EnvScriptInterpreterNotFoundOptions),
    #[serde(rename = "script-interpreter-not-found")]
    ScriptInterpreterNotFound(ScriptInterpreterNotFoundOptions),
    #[serde(rename = "unlisted-script-interpreter")]
    UnlistedScriptInterpreter(UnlistedScriptInterpreterOptions),
    #[serde(rename = "missing-script-interpreter-dependency")]
    MissingScriptInterpreterDependency(MissingScriptInterpreterDependencyOptions),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HostScriptInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

impl Display for HostScriptInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The interpreter {} does not belong to a habitat package",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.interpreter.display().yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct HostScriptInterpreterOptions {
    #[serde(default = "HostScriptInterpreterOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl HostScriptInterpreterOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for HostScriptInterpreterOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingEnvScriptInterpreter {
    pub source: PathBuf,
    pub raw_interpreter: String,
}

impl Display for MissingEnvScriptInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The 'env' command must have atleast 1 argument, found '{}'",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.raw_interpreter.yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingEnvScriptInterpreterOptions {
    #[serde(default = "MissingEnvScriptInterpreterOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}
impl MissingEnvScriptInterpreterOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}
impl Default for MissingEnvScriptInterpreterOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EnvScriptInterpreterNotFound {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

impl Display for EnvScriptInterpreterNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The interpreter command '{}' could not be found in the runtime environment",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.interpreter.display().yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct EnvScriptInterpreterNotFoundOptions {
    #[serde(default = "EnvScriptInterpreterNotFoundOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl EnvScriptInterpreterNotFoundOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for EnvScriptInterpreterNotFoundOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScriptInterpreterNotFound {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

impl Display for ScriptInterpreterNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The interpreter command '{}' could not be found in {}",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.interpreter.display().yellow(),
            self.interpreter_dependency.yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ScriptInterpreterNotFoundOptions {
    #[serde(default = "ScriptInterpreterNotFoundOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl ScriptInterpreterNotFoundOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for ScriptInterpreterNotFoundOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingScriptInterpreterDependency {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

impl Display for MissingScriptInterpreterDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The interpreter command '{}' belongs to {} which is not a runtime dependency of this package",
            self.source.relative_package_path().unwrap().display().white(),
            self.interpreter.display().yellow(),
            self.interpreter_dependency.yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingScriptInterpreterDependencyOptions {
    #[serde(default = "MissingScriptInterpreterDependencyOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl MissingScriptInterpreterDependencyOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for MissingScriptInterpreterDependencyOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct UnlistedScriptInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
    pub listed_interpreters: Vec<PathBuf>,
}

impl Display for UnlistedScriptInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.listed_interpreters.is_empty() {
            write!(
                f,
                "{}: The interpreter command '{}' is not listed as an interpreter in {}",
                self.source
                    .relative_package_path()
                    .unwrap()
                    .display()
                    .white(),
                self.interpreter.display().yellow(),
                self.interpreter_dependency.yellow()
            )
        } else {
            write!(
                f,
                "{}: The interpreter command '{}' is not listed as an interpreter in {}, available interpreters are: {:?}",
                self.source.relative_package_path().unwrap().display().white(),
                self.interpreter.display().yellow(),
                self.interpreter_dependency.yellow(),
                self.listed_interpreters.blue()
            )
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct UnlistedScriptInterpreterOptions {
    #[serde(default = "UnlistedScriptInterpreterOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl UnlistedScriptInterpreterOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Warn
    }
}

impl Default for UnlistedScriptInterpreterOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ScriptCheck {
    env_interpreters: Vec<String>,
    platform_interpreter_paths: Vec<PathBuf>,
}

impl Default for ScriptCheck {
    fn default() -> Self {
        Self {
            env_interpreters: vec![String::from("env")],
            #[cfg(target_os = "linux")]
            platform_interpreter_paths: vec![PathBuf::from("/bin/sh"), PathBuf::from("/bin/false")],
            #[cfg(target_os = "macos")]
            platform_interpreter_paths: vec![
                PathBuf::from("/bin/sh"),
                PathBuf::from("/bin/false"),
                PathBuf::from("/usr/bin/env"),
            ],
        }
    }
}

impl ArtifactCheck for ScriptCheck {
    fn artifact_context_check(
        &self,
        _store: &Store,
        rules: &PlanContextConfig,
        checker_context: &mut CheckerContext,
        _artifact_cache: &mut ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        let mut violations = vec![];
        let mut used_deps = HashSet::new();
        let host_script_interpreter_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Script(ScriptRuleOptions::HostScriptInterpreter(
                    options,
                )) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let missing_env_script_interpreter_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Script(
                    ScriptRuleOptions::MissingEnvScriptInterpreter(options),
                ) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let env_script_interpreter_not_found_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Script(
                    ScriptRuleOptions::EnvScriptInterpreterNotFound(options),
                ) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let script_interpreter_not_found_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Script(ScriptRuleOptions::ScriptInterpreterNotFound(
                    options,
                )) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let unlisted_script_interpreter_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Script(ScriptRuleOptions::UnlistedScriptInterpreter(
                    options,
                )) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let missing_script_interpreter_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Script(
                    ScriptRuleOptions::MissingScriptInterpreterDependency(options),
                ) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let tdep_artifacts = checker_context
            .tdeps
            .as_ref()
            .expect("Check context missing transitive dep artifacts");

        for (path, metadata) in artifact_context.scripts.iter() {
            let command = if metadata.interpreter.command.is_absolute() {
                metadata.interpreter.command.clone()
            } else if let Some(value) = path.parent().map(|p| {
                p.join(metadata.interpreter.command.as_path())
                    .absolutize()
                    .unwrap()
                    .to_path_buf()
            }) {
                value
            } else {
                error!(target: "user-ui", "Could not determine interpreter for {} from header: {}", path.display(), metadata.interpreter.raw);
                continue;
            };
            // Resolves the path if it is a symlink
            debug!(
                "In {}, following interpreter command: {} -> {}",
                path.display(),
                command.display(),
                artifact_context
                    .resolve_path(tdep_artifacts, command.as_path())
                    .display()
            );
            let (command, intermediates) =
                artifact_context.resolve_path_and_intermediates(tdep_artifacts, command);

            if let Some(interpreter_dep) = command.as_path().package_ident(artifact_context.target)
            {
                let is_env_interpreter =
                    if let Some(file_name) = command.file_name().and_then(|x| x.to_str()) {
                        self.env_interpreters.iter().any(|x| x == file_name)
                    } else {
                        false
                    };
                if is_env_interpreter {
                    used_deps.insert(interpreter_dep.clone());
                    // TODO: Handle case where command is symlinked
                    if let Some(command) = metadata.interpreter.args.first() {
                        let mut found = false;
                        for runtime_artifact_ctx in
                            checker_context.runtime_artifacts.as_ref().unwrap().iter()
                        {
                            if let Some(metadata) = runtime_artifact_ctx
                                .search_runtime_executable(tdep_artifacts, command)
                            {
                                found = metadata.is_executable();
                                used_deps.insert(runtime_artifact_ctx.id.clone());
                                break;
                            }
                        }
                        if !found
                            && !env_script_interpreter_not_found_options
                                .ignored_files
                                .is_match(path.relative_package_path().unwrap())
                        {
                            violations.push(LeveledArtifactCheckViolation {
                                level: env_script_interpreter_not_found_options.level,
                                violation: ArtifactCheckViolation::Script(
                                    ScriptRule::EnvScriptInterpreterNotFound(
                                        EnvScriptInterpreterNotFound {
                                            source: path.clone(),
                                            interpreter: PathBuf::from(command),
                                        },
                                    ),
                                ),
                            });
                        }
                    } else if !missing_env_script_interpreter_options
                        .ignored_files
                        .is_match(path.relative_package_path().unwrap())
                    {
                        violations.push(LeveledArtifactCheckViolation {
                            level: missing_env_script_interpreter_options.level,
                            violation: ArtifactCheckViolation::Script(
                                ScriptRule::MissingEnvScriptInterpreter(
                                    MissingEnvScriptInterpreter {
                                        source: path.clone(),
                                        raw_interpreter: metadata.interpreter.raw.clone(),
                                    },
                                ),
                            ),
                        });
                    }
                } else if let Some(interpreter_artifact_ctx) = tdep_artifacts.get(&interpreter_dep)
                {
                    used_deps.insert(interpreter_artifact_ctx.id.clone());
                    if interpreter_artifact_ctx
                        .elfs
                        .contains_key(command.as_path())
                        || interpreter_artifact_ctx
                            .scripts
                            .contains_key(command.as_path())
                        || interpreter_artifact_ctx
                            .links
                            .contains_key(command.as_path())
                        || interpreter_artifact_ctx
                            .machos
                            .contains_key(command.as_path())
                    {
                        let mut interpreter_listed = false;
                        for intermediate in intermediates.iter() {
                            if interpreter_artifact_ctx.interpreters.contains(intermediate) {
                                interpreter_listed = true;
                            }
                        }
                        if !interpreter_listed
                            && !unlisted_script_interpreter_options
                                .ignored_files
                                .is_match(path.relative_package_path().unwrap())
                        {
                            violations.push(LeveledArtifactCheckViolation {
                                level: unlisted_script_interpreter_options.level,
                                violation: ArtifactCheckViolation::Script(
                                    ScriptRule::UnlistedScriptInterpreter(
                                        UnlistedScriptInterpreter {
                                            source: path.clone(),
                                            interpreter: command,
                                            interpreter_dependency: interpreter_dep,
                                            listed_interpreters: interpreter_artifact_ctx
                                                .interpreters
                                                .clone(),
                                        },
                                    ),
                                ),
                            });
                        }
                    } else if !script_interpreter_not_found_options
                        .ignored_files
                        .is_match(path.relative_package_path().unwrap())
                    {
                        violations.push(LeveledArtifactCheckViolation {
                            level: script_interpreter_not_found_options.level,
                            violation: ArtifactCheckViolation::Script(
                                ScriptRule::ScriptInterpreterNotFound(ScriptInterpreterNotFound {
                                    source: path.clone(),
                                    interpreter: command,
                                    interpreter_dependency: interpreter_dep,
                                }),
                            ),
                        });
                    }
                } else if !missing_env_script_interpreter_options
                    .ignored_files
                    .is_match(path.relative_package_path().unwrap())
                {
                    violations.push(LeveledArtifactCheckViolation {
                        level: missing_script_interpreter_dependency_options.level,
                        violation: ArtifactCheckViolation::Script(
                            ScriptRule::MissingScriptInterpreterDependency(
                                MissingScriptInterpreterDependency {
                                    source: path.clone(),
                                    interpreter: command,
                                    interpreter_dependency: interpreter_dep,
                                },
                            ),
                        ),
                    });
                }
            } else if !self.platform_interpreter_paths.contains(&command)
                && !host_script_interpreter_options
                    .ignored_files
                    .is_match(path.relative_package_path().unwrap())
            {
                violations.push(LeveledArtifactCheckViolation {
                    level: host_script_interpreter_options.level,
                    violation: ArtifactCheckViolation::Script(ScriptRule::HostScriptInterpreter(
                        HostScriptInterpreter {
                            source: path.clone(),
                            interpreter: command.clone(),
                        },
                    )),
                });
            }
        }
        for used_dep in used_deps {
            checker_context.mark_used(&used_dep);
        }

        violations.into_iter().collect()
    }
}
