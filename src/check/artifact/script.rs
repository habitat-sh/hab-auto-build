use std::path::PathBuf;

use path_absolutize::Absolutize;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    check::{
        ArtifactCheck, ArtifactCheckViolation, ArtifactRuleOptions, CheckerContext, ContextRules,
        LeveledArtifactCheckViolation, ViolationLevel,
    },
    core::{ArtifactCache, ArtifactContext, PackageIdent, PackagePath},
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct HostScriptInterpreterOptions {
    pub level: ViolationLevel,
}

impl Default for HostScriptInterpreterOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingEnvScriptInterpreter {
    pub source: PathBuf,
    pub raw_interpreter: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingEnvScriptInterpreterOptions {
    pub level: ViolationLevel,
}

impl Default for MissingEnvScriptInterpreterOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EnvScriptInterpreterNotFound {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct EnvScriptInterpreterNotFoundOptions {
    pub level: ViolationLevel,
}

impl Default for EnvScriptInterpreterNotFoundOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScriptInterpreterNotFound {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ScriptInterpreterNotFoundOptions {
    pub level: ViolationLevel,
}

impl Default for ScriptInterpreterNotFoundOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingScriptInterpreterDependency {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingScriptInterpreterDependencyOptions {
    pub level: ViolationLevel,
}

impl Default for MissingScriptInterpreterDependencyOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct UnlistedScriptInterpreterOptions {
    pub level: ViolationLevel,
}

impl Default for UnlistedScriptInterpreterOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Warn,
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
            platform_interpreter_paths: vec![PathBuf::from("/bin/sh")],
        }
    }
}

impl ArtifactCheck for ScriptCheck {
    fn artifact_context_check(
        &self,
        rules: &ContextRules,
        checker_context: &mut CheckerContext,
        artifact_cache: &ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        let mut violations = vec![];

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
            } else {
                path.join(metadata.interpreter.command.as_path())
                    .absolutize()
                    .unwrap()
                    .to_path_buf()
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
            let command = artifact_context.resolve_path(tdep_artifacts, command);

            if let Some(interpreter_dep) = command.as_path().package_ident(artifact_context.target)
            {
                let is_env_interpreter =
                    if let Some(file_name) = command.file_name().and_then(|x| x.to_str()) {
                        self.env_interpreters.iter().any(|x| x == file_name)
                    } else {
                        false
                    };
                if is_env_interpreter {
                    // TODO: Handle case where command is symlinked
                    if let Some(command) = metadata.interpreter.args.first() {
                        let mut found = false;
                        for runtime_artifact_ctx in
                            checker_context.runtime_artifacts.as_ref().unwrap().iter()
                        {
                            if runtime_artifact_ctx
                                .search_runtime_executable(command)
                                .is_some()
                            {
                                found = true;
                                break;
                            }
                        }
                        if !found {
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
                    } else {
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
                    if interpreter_artifact_ctx
                        .elfs
                        .get(command.as_path())
                        .is_some()
                        || interpreter_artifact_ctx
                            .scripts
                            .get(command.as_path())
                            .is_some()
                    {
                        if !interpreter_artifact_ctx.interpreters.contains(&command) {
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
                    } else {
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
                } else {
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
            } else if !self.platform_interpreter_paths.contains(&command) {
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

        violations
            .into_iter()
            .filter(|v| v.level != ViolationLevel::Off)
            .collect()
    }
}
