use std::{collections::BTreeSet, fmt::Display, path::PathBuf};

#[cfg(not(target_os = "windows"))]
use std::{
    collections::{hash_map::Entry, HashMap},
    ffi::OsString,
};

use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

use crate::{
    check::{
        ArtifactCheck, CheckerContext, LeveledArtifactCheckViolation, PlanContextConfig,
        ViolationLevel,
    },
    core::{ArtifactCache, ArtifactContext, PackageDepGlob, PackageIdent, PackagePath},
    store::Store,
};

#[cfg(not(target_os = "windows"))]
use crate::check::{ArtifactCheckViolation, ArtifactRuleOptions};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum PackageRule {
    #[serde(rename = "bad-runtime-path-entry")]
    BadRuntimePathEntry(BadRuntimePathEntry),
    #[serde(rename = "missing-runtime-path-entry-dependency")]
    MissingRuntimePathEntryDependency(MissingRuntimePathEntryDependency),
    #[serde(rename = "missing-dependency-artifact")]
    MissingDependencyArtifact(MissingDependencyArtifact),
    #[serde(rename = "duplicate-dependency")]
    DuplicateDependency(DuplicateDependency),
    #[serde(rename = "empty-top-level-directory")]
    EmptyTopLevelDirectory(EmptyTopLevelDirectory),
    #[serde(rename = "broken-link")]
    BrokenLink(BrokenLink),
    #[serde(rename = "unused-dependency")]
    UnusedDependency(UnusedDependency),
    #[serde(rename = "duplicate-runtime-binary")]
    DuplicateRuntimeBinary(DuplicateRuntimeBinary),
}

impl Display for PackageRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageRule::BadRuntimePathEntry(rule) => write!(f, "{}", rule),
            PackageRule::MissingRuntimePathEntryDependency(rule) => write!(f, "{}", rule),
            PackageRule::MissingDependencyArtifact(rule) => write!(f, "{}", rule),
            PackageRule::DuplicateDependency(rule) => write!(f, "{}", rule),
            PackageRule::EmptyTopLevelDirectory(rule) => write!(f, "{}", rule),
            PackageRule::BrokenLink(rule) => write!(f, "{}", rule),
            PackageRule::UnusedDependency(rule) => write!(f, "{}", rule),
            PackageRule::DuplicateRuntimeBinary(rule) => write!(f, "{}", rule),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "id", content = "options")]
pub(crate) enum PackageRuleOptions {
    #[serde(rename = "bad-runtime-path-entry")]
    BadRuntimePathEntry(BadRuntimePathEntryOptions),
    #[serde(rename = "missing-runtime-path-entry-dependency")]
    MissingRuntimePathEntryDependency(MissingRuntimePathEntryDependencyOptions),
    #[serde(rename = "missing-dependency-artifact")]
    MissingDependencyArtifact(MissingDependencyArtifactOptions),
    #[serde(rename = "duplicate-dependency")]
    DuplicateDependency(DuplicateDependencyOptions),
    #[serde(rename = "empty-top-level-directory")]
    EmptyTopLevelDirectory(EmptyTopLevelDirectoryOptions),
    #[serde(rename = "broken-link")]
    BrokenLink(BrokenLinkOptions),
    #[serde(rename = "unused-dependency")]
    UnusedDependency(UnusedDependencyOptions),
    #[serde(rename = "duplicate-runtime-binary")]
    DuplicateRuntimeBinary(DuplicateRuntimeBinaryOptions),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadRuntimePathEntry {
    pub entry: PathBuf,
}

impl Display for BadRuntimePathEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The runtime path entry {} does not belong to a habitat package",
            self.entry.display().yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BadRuntimePathEntryOptions {
    pub level: ViolationLevel,
}

impl Default for BadRuntimePathEntryOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingRuntimePathEntryDependency {
    pub entry: PathBuf,
    pub dep_ident: PackageIdent,
}

impl Display for MissingRuntimePathEntryDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The runtime path entry {} belongs to {} which is not a runtime dependency of this package", self.entry.display().yellow(), self.dep_ident.yellow())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingRuntimePathEntryDependencyOptions {
    pub level: ViolationLevel,
}

impl Default for MissingRuntimePathEntryDependencyOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingDependencyArtifact {
    pub dep_ident: PackageIdent,
}

impl Display for MissingDependencyArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Could not find an artifact for {} required by this package",
            self.dep_ident.yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingDependencyArtifactOptions {
    pub level: ViolationLevel,
}

impl Default for MissingDependencyArtifactOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DuplicateDependency {
    pub dep_ident: PackageIdent,
}

impl Display for DuplicateDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The package {} is specified as both a 'dep' and 'build_dep' for this package",
            self.dep_ident.yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct DuplicateDependencyOptions {
    pub level: ViolationLevel,
}

impl Default for DuplicateDependencyOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EmptyTopLevelDirectory {
    pub directory: PathBuf,
}

impl Display for EmptyTopLevelDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The top level directory {} does not contain any files",
            self.directory.display().yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct EmptyTopLevelDirectoryOptions {
    pub level: ViolationLevel,
}

impl Default for EmptyTopLevelDirectoryOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Warn,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BrokenLink {
    pub entry: PathBuf,
    pub link: PathBuf,
}

impl Display for BrokenLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The symlink points to {} which does not exist",
            self.entry
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.link.display().yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BrokenLinkOptions {
    pub level: ViolationLevel,
}

impl Default for BrokenLinkOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct UnusedDependency {
    pub dep_ident: PackageIdent,
}

impl Display for UnusedDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The package {} does not seem to be used at runtime",
            self.dep_ident.yellow(),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct UnusedDependencyOptions {
    #[serde(default = "UnusedDependencyOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_packages: BTreeSet<PackageDepGlob>,
}

impl UnusedDependencyOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Warn
    }
}

impl Default for UnusedDependencyOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_packages: BTreeSet::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DuplicateRuntimeBinary {
    pub primary_binary: PathBuf,
    pub duplicate_binary: PathBuf,
}

impl Display for DuplicateRuntimeBinary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Duplicate binary {} available at {}, it was first found at {}",
            self.primary_binary
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .yellow(),
            self.duplicate_binary.display().blue(),
            self.primary_binary.display().blue(),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct DuplicateRuntimeBinaryOptions {
    #[serde(default = "DuplicateRuntimeBinaryOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub primary_packages: BTreeSet<PackageDepGlob>,
}

impl DuplicateRuntimeBinaryOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Warn
    }
}

impl Default for DuplicateRuntimeBinaryOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Warn,
            primary_packages: BTreeSet::default(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct PackageBeforeCheck {}

impl ArtifactCheck for PackageBeforeCheck {
    #[cfg(target_os = "windows")]
    fn artifact_context_check(
        &self,
        _store: &Store,
        _rules: &PlanContextConfig,
        _checker_context: &mut CheckerContext,
        _artifact_cache: &mut ArtifactCache,
        _artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        // Currently, we do not know what the violations are for Windows; we will revisit this later.
        vec![].into_iter().collect()
    }

    #[cfg(not(target_os = "windows"))]
    fn artifact_context_check(
        &self,
        _store: &Store,
        rules: &PlanContextConfig,
        checker_context: &mut CheckerContext,
        artifact_cache: &mut ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        let mut violations = vec![];
        let bad_runtime_path_entry_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(PackageRuleOptions::BadRuntimePathEntry(
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

        let missing_runtime_path_entry_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(
                    PackageRuleOptions::MissingRuntimePathEntryDependency(options),
                ) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let missing_dependency_artifact_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(
                    PackageRuleOptions::MissingDependencyArtifact(options),
                ) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let duplicate_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(PackageRuleOptions::DuplicateDependency(
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

        let empty_top_level_directory_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(PackageRuleOptions::EmptyTopLevelDirectory(
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

        let broken_link_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(PackageRuleOptions::BrokenLink(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let duplicate_runtime_binary_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(PackageRuleOptions::DuplicateRuntimeBinary(
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

        let duplicate_deps = artifact_context
            .deps
            .intersection(&artifact_context.build_deps);

        for duplicate_dep in duplicate_deps {
            violations.push(LeveledArtifactCheckViolation {
                level: duplicate_dependency_options.level,
                violation: ArtifactCheckViolation::Package(PackageRule::DuplicateDependency(
                    DuplicateDependency {
                        dep_ident: duplicate_dep.clone(),
                    },
                )),
            });
        }
        if !artifact_context.empty_top_level_dirs.is_empty() {
            for empty_top_level_dir in artifact_context.empty_top_level_dirs.iter() {
                violations.push(LeveledArtifactCheckViolation {
                    level: empty_top_level_directory_options.level,
                    violation: ArtifactCheckViolation::Package(
                        PackageRule::EmptyTopLevelDirectory(EmptyTopLevelDirectory {
                            directory: empty_top_level_dir.clone(),
                        }),
                    ),
                });
            }
        }
        if !artifact_context.broken_links.is_empty() {
            for (entry, link) in artifact_context.broken_links.iter() {
                violations.push(LeveledArtifactCheckViolation {
                    level: broken_link_options.level,
                    violation: ArtifactCheckViolation::Package(PackageRule::BrokenLink(
                        BrokenLink {
                            entry: entry.clone(),
                            link: link.clone(),
                        },
                    )),
                });
            }
        }

        let tdep_artifacts = artifact_context
            .tdeps
            .iter()
            .filter_map(|dep_ident| {
                if let Some(artifact) = artifact_cache.artifact(dep_ident).unwrap() {
                    Some((artifact.id.clone(), artifact.clone()))
                } else {
                    violations.push(LeveledArtifactCheckViolation {
                        level: missing_dependency_artifact_options.level,
                        violation: ArtifactCheckViolation::Package(
                            PackageRule::MissingDependencyArtifact(MissingDependencyArtifact {
                                dep_ident: dep_ident.clone(),
                            }),
                        ),
                    });
                    None
                }
            })
            .chain(Some((
                artifact_context.id.clone(),
                artifact_context.clone(),
            ))) // The artifact as it's own dependency
            .collect::<HashMap<PackageIdent, ArtifactContext>>();

        let mut runtime_binaries: HashMap<OsString, PathBuf> = HashMap::new();

        let runtime_path = artifact_context
            .runtime_path
            .iter()
            .filter_map(|search_path| {
                if let Some(dep_ident) = search_path.package_ident(artifact_context.target) {
                    if tdep_artifacts.contains_key(&dep_ident) {
                        let artifact_ctx = artifact_cache.artifact(&dep_ident).unwrap();
                        if let Some(artifact_ctx) = &artifact_ctx {
                            for (elf_path, elf_metadata) in &artifact_ctx.elfs {
                                if !elf_metadata.is_executable {
                                    continue;
                                }
                                if elf_path.parent().unwrap() == search_path {
                                    match runtime_binaries
                                        .entry(elf_path.file_name().unwrap().to_os_string())
                                    {
                                        Entry::Occupied(entry) => {
                                            if entry.get() != elf_path {
                                                // If the resolved executable is from a specified primary package,
                                                // we allow it and don't create a violation
                                                if duplicate_runtime_binary_options
                                                    .primary_packages
                                                    .iter()
                                                    .any(|dep_ident| {
                                                        entry
                                                            .get()
                                                            .package_ident(artifact_ctx.target)
                                                            .is_some_and(|ident| {
                                                                dep_ident
                                                                    .matcher()
                                                                    .matches_package_ident(&ident)
                                                            })
                                                    })
                                                {
                                                    continue;
                                                }
                                                violations.push(LeveledArtifactCheckViolation {
                                                    level: duplicate_runtime_binary_options.level,
                                                    violation: ArtifactCheckViolation::Package(
                                                        PackageRule::DuplicateRuntimeBinary(
                                                            DuplicateRuntimeBinary {
                                                                primary_binary: entry.get().clone(),
                                                                duplicate_binary: elf_path.clone(),
                                                            },
                                                        ),
                                                    ),
                                                });
                                            }
                                        }
                                        Entry::Vacant(entry) => {
                                            entry.insert(elf_path.clone());
                                        }
                                    }
                                }
                            }
                            for (script_path, script_metadata) in &artifact_ctx.scripts {
                                if !script_metadata.is_executable {
                                    continue;
                                }
                                if script_path.parent().unwrap() == search_path {
                                    match runtime_binaries
                                        .entry(script_path.file_name().unwrap().to_os_string())
                                    {
                                        Entry::Occupied(entry) => {
                                            if entry.get() != script_path {
                                                // If the resolved executable is from a specified primary package,
                                                // we allow it and don't create a violation
                                                if duplicate_runtime_binary_options
                                                    .primary_packages
                                                    .iter()
                                                    .any(|dep_ident| {
                                                        entry
                                                            .get()
                                                            .package_ident(artifact_ctx.target)
                                                            .is_some_and(|ident| {
                                                                dep_ident
                                                                    .matcher()
                                                                    .matches_package_ident(&ident)
                                                            })
                                                    })
                                                {
                                                    continue;
                                                }
                                                violations.push(LeveledArtifactCheckViolation {
                                                    level: duplicate_runtime_binary_options.level,
                                                    violation: ArtifactCheckViolation::Package(
                                                        PackageRule::DuplicateRuntimeBinary(
                                                            DuplicateRuntimeBinary {
                                                                primary_binary: entry.get().clone(),
                                                                duplicate_binary: script_path
                                                                    .clone(),
                                                            },
                                                        ),
                                                    ),
                                                });
                                            }
                                        }
                                        Entry::Vacant(entry) => {
                                            entry.insert(script_path.clone());
                                        }
                                    }
                                }
                            }
                        }
                        artifact_ctx
                    } else {
                        violations.push(LeveledArtifactCheckViolation {
                            level: missing_runtime_path_entry_dependency_options.level,
                            violation: ArtifactCheckViolation::Package(
                                PackageRule::MissingRuntimePathEntryDependency(
                                    MissingRuntimePathEntryDependency {
                                        entry: search_path.clone(),
                                        dep_ident,
                                    },
                                ),
                            ),
                        });
                        None
                    }
                } else {
                    violations.push(LeveledArtifactCheckViolation {
                        level: bad_runtime_path_entry_options.level,
                        violation: ArtifactCheckViolation::Package(
                            PackageRule::BadRuntimePathEntry(BadRuntimePathEntry {
                                entry: search_path.clone(),
                            }),
                        ),
                    });
                    None
                }
            })
            .collect();
        checker_context.tdeps = Some(tdep_artifacts);
        checker_context.runtime_artifacts = Some(runtime_path);
        checker_context.unused_deps = Some(artifact_context.deps.clone());
        violations.into_iter().collect()
    }
}

#[derive(Debug, Default)]
pub(crate) struct PackageAfterCheck {}

impl ArtifactCheck for PackageAfterCheck {
    #[cfg(target_os = "windows")]
    fn artifact_context_check(
        &self,
        _store: &Store,
        _rules: &PlanContextConfig,
        _checker_context: &mut CheckerContext,
        _artifact_cache: &mut ArtifactCache,
        _artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        // Currently, we do not know what the violations are for Windows; we will revisit this later.
        vec![].into_iter().collect()
    }

    #[cfg(not(target_os = "windows"))]
    fn artifact_context_check(
        &self,
        _store: &Store,
        rules: &PlanContextConfig,
        checker_context: &mut CheckerContext,
        _artifact_cache: &mut ArtifactCache,
        _artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        let mut violations = vec![];
        let unused_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Package(PackageRuleOptions::UnusedDependency(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");
        let unused_deps = checker_context.unused_deps.as_ref().unwrap();
        if !unused_deps.is_empty() {
            for unused_dep in unused_deps {
                if unused_dependency_options
                    .ignored_packages
                    .iter()
                    .any(|dep_ident| dep_ident.matcher().matches_package_ident(unused_dep))
                {
                    continue;
                }
                violations.push(LeveledArtifactCheckViolation {
                    level: unused_dependency_options.level,
                    violation: ArtifactCheckViolation::Package(PackageRule::UnusedDependency(
                        UnusedDependency {
                            dep_ident: unused_dep.clone(),
                        },
                    )),
                })
            }
        }
        violations.into_iter().collect()
    }
}
