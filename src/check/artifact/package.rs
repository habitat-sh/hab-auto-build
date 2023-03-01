use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    check::{
        ArtifactCheck, ArtifactCheckViolation, ArtifactRuleOptions, CheckerContext, ContextRules,
        LeveledArtifactCheckViolation, ViolationLevel,
    },
    core::{ArtifactCache, ArtifactContext, PackageIdent, PackagePath},
};

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
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadRuntimePathEntry {
    pub entry: PathBuf,
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

#[derive(Debug, Default)]
pub(crate) struct PackageCheck {}

impl ArtifactCheck for PackageCheck {
    fn artifact_context_check(
        &self,
        rules: &ContextRules,
        checker_context: &mut CheckerContext,
        artifact_cache: &ArtifactCache,
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
                if let Some(artifact) = artifact_cache.artifact(dep_ident) {
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
            .chain(Some((artifact_context.id.clone(), artifact_context.clone())).into_iter()) // The artifact as it's own dependency
            .collect::<HashMap<PackageIdent, ArtifactContext>>();
        let runtime_path = artifact_context
            .runtime_path
            .iter()
            .filter_map(|search_path| {
                if let Some(dep_ident) = search_path.package_ident(artifact_context.target) {
                    if tdep_artifacts.get(&dep_ident).is_some() {
                        artifact_cache.artifact(&dep_ident).cloned()
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
        violations
            .into_iter()
            .filter(|v| v.level != ViolationLevel::Off)
            .collect()
    }
}
