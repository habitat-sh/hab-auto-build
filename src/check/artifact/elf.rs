use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    check::{
        ArtifactCheck, ArtifactCheckViolation, ArtifactRuleOptions, CheckerContext, ContextRules,
        LeveledArtifactCheckViolation, ViolationLevel,
    },
    core::{ArtifactCache, ArtifactContext, ElfType, PackageIdent, PackagePath},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum ElfRule {
    #[serde(rename = "missing-rpath-entry-dependency")]
    MissingRPathEntryDependency(MissingRPathEntryDependency),
    #[serde(rename = "bad-rpath-entry")]
    BadRPathEntry(BadRPathEntry),
    #[serde(rename = "missing-runpath-entry-dependency")]
    MissingRunPathEntryDependency(MissingRunPathEntryDependency),
    #[serde(rename = "bad-runpath-entry")]
    BadRunPathEntry(BadRunPathEntry),
    #[serde(rename = "library-dependency-not-found")]
    LibraryDependencyNotFound(LibraryDependencyNotFound),
    #[serde(rename = "bad-library-dependency")]
    BadLibraryDependency(BadLibraryDependency),
    #[serde(rename = "bad-elf-interpreter")]
    BadELFInterpreter(BadELFInterpreter),
    #[serde(rename = "host-elf-interpreter")]
    HostELFInterpreter(HostELFInterpreter),
    #[serde(rename = "elf-interpreter-not-found")]
    ELFInterpreterNotFound(ELFInterpreterNotFound),
    #[serde(rename = "missing-elf-interpreter-dependency")]
    MissingELFInterpreterDependency(MissingELFInterpreterDependency),
    #[serde(rename = "unexpected-elf-interpreter")]
    UnexpectedELFInterpreter(UnexpectedELFInterpreter),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "id", content = "options")]
pub(crate) enum ElfRuleOptions {
    #[serde(rename = "missing-rpath-entry-dependency")]
    MissingRPathEntryDependency(MissingRPathEntryDependencyOptions),
    #[serde(rename = "bad-rpath-entry")]
    BadRPathEntry(BadRPathEntryOptions),
    #[serde(rename = "missing-runpath-entry-dependency")]
    MissingRunPathEntryDependency(MissingRunPathEntryDependencyOptions),
    #[serde(rename = "bad-runpath-entry")]
    BadRunPathEntry(BadRunPathEntryOptions),
    #[serde(rename = "library-dependency-not-found")]
    LibraryDependencyNotFound(LibraryDependencyNotFoundOptions),
    #[serde(rename = "bad-library-dependency")]
    BadLibraryDependency(BadLibraryDependencyOptions),
    #[serde(rename = "bad-elf-interpreter")]
    BadELFInterpreter(BadELFInterpreterOptions),
    #[serde(rename = "host-elf-interpreter")]
    HostELFInterpreter(HostELFInterpreterOptions),
    #[serde(rename = "elf-interpreter-not-found")]
    ELFInterpreterNotFound(ELFInterpreterNotFoundOptions),
    #[serde(rename = "missing-elf-interpreter-dependency")]
    MissingELFInterpreterDependency(MissingELFInterpreterDependencyOptions),
    #[serde(rename = "unexpected-elf-interpreter")]
    UnexpectedELFInterpreter(UnexpectedELFInterpreterOptions),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingRPathEntryDependency {
    pub source: PathBuf,
    pub entry: PathBuf,
    pub dep_ident: PackageIdent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingRPathEntryDependencyOptions {
    pub level: ViolationLevel,
}

impl Default for MissingRPathEntryDependencyOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadRPathEntry {
    pub source: PathBuf,
    pub entry: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BadRPathEntryOptions {
    pub level: ViolationLevel,
}

impl Default for BadRPathEntryOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingRunPathEntryDependency {
    pub source: PathBuf,
    pub entry: PathBuf,
    pub dep_ident: PackageIdent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingRunPathEntryDependencyOptions {
    pub level: ViolationLevel,
}

impl Default for MissingRunPathEntryDependencyOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadRunPathEntry {
    pub source: PathBuf,
    pub entry: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BadRunPathEntryOptions {
    pub level: ViolationLevel,
}

impl Default for BadRunPathEntryOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LibraryDependencyNotFound {
    pub source: PathBuf,
    pub library: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LibraryDependencyNotFoundOptions {
    pub level: ViolationLevel,
}

impl Default for LibraryDependencyNotFoundOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadLibraryDependency {
    pub source: PathBuf,
    pub library: String,
    pub library_path: PathBuf,
    pub elf_type: ElfType,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BadLibraryDependencyOptions {
    pub level: ViolationLevel,
}

impl Default for BadLibraryDependencyOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingELFInterpreter {
    pub source: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingELFInterpreterOptions {
    pub level: ViolationLevel,
}

impl Default for MissingELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadELFInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BadELFInterpreterOptions {
    pub level: ViolationLevel,
}

impl Default for BadELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HostELFInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct HostELFInterpreterOptions {
    pub level: ViolationLevel,
}

impl Default for HostELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ELFInterpreterNotFound {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ELFInterpreterNotFoundOptions {
    pub level: ViolationLevel,
}

impl Default for ELFInterpreterNotFoundOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingELFInterpreterDependency {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingELFInterpreterDependencyOptions {
    pub level: ViolationLevel,
}

impl Default for MissingELFInterpreterDependencyOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct UnexpectedELFInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct UnexpectedELFInterpreterOptions {
    pub level: ViolationLevel,
}

impl Default for UnexpectedELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ElfCheck {}

impl ArtifactCheck for ElfCheck {
    fn artifact_context_check(
        &self,
        rules: &ContextRules,
        checker_context: &mut CheckerContext,
        artifact_cache: &ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        let mut violations = vec![];
        let tdep_artifacts = checker_context
            .tdeps
            .as_ref()
            .expect("Check context missing transitive dep artifacts");

        let missing_rpath_entry_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::MissingRPathEntryDependency(
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
        let bad_rpath_entry_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::BadRPathEntry(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let missing_runpath_entry_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::MissingRunPathEntryDependency(
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

        let bad_runpath_entry_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::BadRunPathEntry(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let library_dependency_not_found_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::LibraryDependencyNotFound(
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

        let bad_library_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::BadLibraryDependency(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let bad_elf_interpreter_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::BadELFInterpreter(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let host_elf_interpreter_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::HostELFInterpreter(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let elf_interpreter_not_found_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::ELFInterpreterNotFound(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let missing_elf_interpreter_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::MissingELFInterpreterDependency(
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

        let unexpected_elf_interpreter_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::UnexpectedELFInterpreter(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        for (path, metadata) in artifact_context.elfs.iter() {
            // Check the interpreter
            let mut interpreter_name = None;

            match metadata.elf_type {
                ElfType::Executable | ElfType::PieExecutable => {
                    if let Some(interpreter_path) = metadata.interpreter.as_ref() {
                        if let Some(file_name) =
                            interpreter_path.file_name().and_then(|x| x.to_str())
                        {
                            interpreter_name = Some(file_name.to_string());
                            if let Some(interpreter_dep) =
                                interpreter_path.package_ident(artifact_context.target)
                            {
                                if let Some(interpreter_artifact_ctx) =
                                    tdep_artifacts.get(&interpreter_dep)
                                {
                                    if interpreter_artifact_ctx
                                        .elfs
                                        .get(interpreter_path.as_path())
                                        .is_none()
                                    {
                                        let resolved_interpreter_path = interpreter_artifact_ctx
                                            .resolve_path(
                                                tdep_artifacts,
                                                interpreter_path.as_path(),
                                            );
                                        if resolved_interpreter_path != *interpreter_path {
                                            debug!(
                                                "In {}, following elf interpreter path: {} -> {}",
                                                path.display(),
                                                interpreter_path.display(),
                                                resolved_interpreter_path.display()
                                            );
                                            if resolved_interpreter_path
                                                .package_ident(interpreter_artifact_ctx.target)
                                                .and_then(|p| tdep_artifacts.get(&p))
                                                .and_then(|a| {
                                                    a.elfs.get(&resolved_interpreter_path)
                                                })
                                                .is_none()
                                            {
                                                violations.push(LeveledArtifactCheckViolation {
                                                    level: elf_interpreter_not_found_options.level,
                                                    violation: ArtifactCheckViolation::Elf(
                                                        ElfRule::ELFInterpreterNotFound(
                                                            ELFInterpreterNotFound {
                                                                source: path.clone(),
                                                                interpreter: interpreter_path
                                                                    .clone(),
                                                                interpreter_dependency:
                                                                    interpreter_dep,
                                                            },
                                                        ),
                                                    ),
                                                });
                                            }
                                        } else {
                                            violations.push(LeveledArtifactCheckViolation {
                                                level: elf_interpreter_not_found_options.level,
                                                violation: ArtifactCheckViolation::Elf(
                                                    ElfRule::ELFInterpreterNotFound(
                                                        ELFInterpreterNotFound {
                                                            source: path.clone(),
                                                            interpreter: interpreter_path.clone(),
                                                            interpreter_dependency: interpreter_dep,
                                                        },
                                                    ),
                                                ),
                                            });
                                        }
                                    }
                                } else {
                                    violations.push(LeveledArtifactCheckViolation {
                                        level: missing_elf_interpreter_dependency_options.level,
                                        violation: ArtifactCheckViolation::Elf(
                                            ElfRule::MissingELFInterpreterDependency(
                                                MissingELFInterpreterDependency {
                                                    source: path.clone(),
                                                    interpreter: interpreter_path.clone(),
                                                    interpreter_dependency: interpreter_dep,
                                                },
                                            ),
                                        ),
                                    });
                                }
                            } else {
                                violations.push(LeveledArtifactCheckViolation {
                                    level: host_elf_interpreter_options.level,
                                    violation: ArtifactCheckViolation::Elf(
                                        ElfRule::HostELFInterpreter(HostELFInterpreter {
                                            source: path.clone(),
                                            interpreter: interpreter_path.clone(),
                                        }),
                                    ),
                                });
                            }
                        } else {
                            violations.push(LeveledArtifactCheckViolation {
                                level: bad_elf_interpreter_options.level,
                                violation: ArtifactCheckViolation::Elf(ElfRule::BadELFInterpreter(
                                    BadELFInterpreter {
                                        source: path.clone(),
                                        interpreter: interpreter_path.to_path_buf(),
                                    },
                                )),
                            });
                        }
                    }
                }
                ElfType::SharedLibrary | ElfType::Relocatable | ElfType::Other => {
                    if let Some(interpreter_path) = metadata.interpreter.as_ref() {
                        violations.push(LeveledArtifactCheckViolation {
                            level: unexpected_elf_interpreter_options.level,
                            violation: ArtifactCheckViolation::Elf(
                                ElfRule::UnexpectedELFInterpreter(UnexpectedELFInterpreter {
                                    source: path.clone(),
                                    interpreter: interpreter_path.to_path_buf(),
                                }),
                            ),
                        });
                    }
                }
            }

            for library in metadata.required_libraries.iter() {
                let mut found = false;
                // If the library is the interpreter skip it
                if let Some(interpreter_name) = interpreter_name.as_ref() {
                    if interpreter_name == library.as_str() {
                        continue;
                    }
                }
                for search_path in metadata.rpath.iter() {
                    if let Some(dep_ident) = search_path.package_ident(artifact_context.target) {
                        if let Some(artifact) = tdep_artifacts.get(&dep_ident) {
                            let library_path = search_path.join(library);
                            let metadata = if let Some(metadata) = artifact.elfs.get(&library_path)
                            {
                                Some(metadata)
                            } else {
                                let resolved_path =
                                    artifact.resolve_path(tdep_artifacts, library_path.as_path());
                                if resolved_path != library_path {
                                    debug!(
                                        "In {}, following shared library path: {} -> {}",
                                        path.display(),
                                        library_path.display(),
                                        resolved_path.display()
                                    );
                                    resolved_path
                                        .package_ident(artifact.target)
                                        .and_then(|p| tdep_artifacts.get(&p))
                                        .and_then(|a| a.elfs.get(&resolved_path))
                                } else {
                                    None
                                }
                            };
                            if let Some(metadata) = metadata {
                                match metadata.elf_type {
                                    ElfType::SharedLibrary | ElfType::Relocatable => {
                                        found = true;
                                        break;
                                    }
                                    ElfType::Executable
                                    | ElfType::PieExecutable
                                    | ElfType::Other => {
                                        found = true;
                                        violations.push(LeveledArtifactCheckViolation {
                                            level: bad_library_dependency_options.level,
                                            violation: ArtifactCheckViolation::Elf(
                                                ElfRule::BadLibraryDependency(
                                                    BadLibraryDependency {
                                                        source: path.clone(),
                                                        library: library.clone(),
                                                        library_path,
                                                        elf_type: metadata.elf_type,
                                                    },
                                                ),
                                            ),
                                        });
                                        break;
                                    }
                                }
                            }
                        } else {
                            violations.push(LeveledArtifactCheckViolation {
                                level: missing_rpath_entry_dependency_options.level,
                                violation: ArtifactCheckViolation::Elf(
                                    ElfRule::MissingRPathEntryDependency(
                                        MissingRPathEntryDependency {
                                            source: path.clone(),
                                            entry: search_path.clone(),
                                            dep_ident: dep_ident.clone(),
                                        },
                                    ),
                                ),
                            });
                        }
                    } else {
                        violations.push(LeveledArtifactCheckViolation {
                            level: bad_rpath_entry_options.level,
                            violation: ArtifactCheckViolation::Elf(ElfRule::BadRPathEntry(
                                BadRPathEntry {
                                    source: path.clone(),
                                    entry: search_path.clone(),
                                },
                            )),
                        });
                    }
                }

                for search_path in metadata.runpath.iter() {
                    if let Some(dep_ident) = search_path.package_ident(artifact_context.target) {
                        if let Some(artifact) = tdep_artifacts.get(&dep_ident) {
                            let library_path = search_path.join(library);
                            let metadata = if let Some(metadata) = artifact.elfs.get(&library_path)
                            {
                                Some(metadata)
                            } else {
                                let resolved_path =
                                    artifact.resolve_path(tdep_artifacts, library_path.as_path());
                                if resolved_path != library_path {
                                    debug!(
                                        "In {}, following shared library path: {} -> {}",
                                        path.display(),
                                        library_path.display(),
                                        resolved_path.display()
                                    );
                                    resolved_path
                                        .package_ident(artifact.target)
                                        .and_then(|p| tdep_artifacts.get(&p))
                                        .and_then(|a| a.elfs.get(&resolved_path))
                                } else {
                                    None
                                }
                            };
                            if let Some(metadata) = metadata {
                                match metadata.elf_type {
                                    ElfType::SharedLibrary | ElfType::Relocatable => {
                                        found = true;
                                        break;
                                    }
                                    ElfType::Executable
                                    | ElfType::PieExecutable
                                    | ElfType::Other => {
                                        found = true;
                                        violations.push(LeveledArtifactCheckViolation {
                                            level: bad_library_dependency_options.level,
                                            violation: ArtifactCheckViolation::Elf(
                                                ElfRule::BadLibraryDependency(
                                                    BadLibraryDependency {
                                                        source: path.clone(),
                                                        library: library.clone(),
                                                        library_path,
                                                        elf_type: metadata.elf_type,
                                                    },
                                                ),
                                            ),
                                        });
                                        break;
                                    }
                                }
                            }
                        } else {
                            violations.push(LeveledArtifactCheckViolation {
                                level: missing_runpath_entry_dependency_options.level,
                                violation: ArtifactCheckViolation::Elf(
                                    ElfRule::MissingRunPathEntryDependency(
                                        MissingRunPathEntryDependency {
                                            source: path.clone(),
                                            entry: search_path.clone(),
                                            dep_ident: dep_ident.clone(),
                                        },
                                    ),
                                ),
                            });
                        }
                    } else {
                        violations.push(LeveledArtifactCheckViolation {
                            level: bad_runpath_entry_options.level,
                            violation: ArtifactCheckViolation::Elf(ElfRule::BadRunPathEntry(
                                BadRunPathEntry {
                                    source: path.clone(),
                                    entry: search_path.clone(),
                                },
                            )),
                        });
                    }
                }

                if !found {
                    violations.push(LeveledArtifactCheckViolation {
                        level: library_dependency_not_found_options.level,
                        violation: ArtifactCheckViolation::Elf(ElfRule::LibraryDependencyNotFound(
                            LibraryDependencyNotFound {
                                source: path.clone(),
                                library: library.clone(),
                            },
                        )),
                    });
                }
            }
        }
        violations
            .into_iter()
            .filter(|v| v.level != ViolationLevel::Off)
            .collect()
    }
}
