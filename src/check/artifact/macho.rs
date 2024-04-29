use std::{collections::HashSet, fmt::Display, path::PathBuf};

use lazy_static::lazy_static;
use owo_colors::OwoColorize;
use path_absolutize::Absolutize;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use crate::{
    check::{
        ArtifactCheck, ArtifactCheckViolation, ArtifactRuleOptions, CheckerContext,
        LeveledArtifactCheckViolation, PlanContextConfig, ViolationLevel,
    },
    core::{
        habitat::{MACOS_SYSTEM_DIRS, MACOS_SYSTEM_LIBS},
        ArtifactCache, ArtifactContext, GlobSetExpression, MachOType, PackageIdent, PackagePath,
    }, store::Store,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum MachORule {
    #[serde(rename = "missing-rpath-entry-dependency")]
    MissingRPathEntryDependency(MissingRPathEntryDependency),
    #[serde(rename = "bad-rpath-entry")]
    BadRPathEntry(BadRPathEntry),
    #[serde(rename = "unused-rpath-entry")]
    UnusedRPathEntry(UnusedRPathEntry),
    #[serde(rename = "missing-library-dependency")]
    MissingLibraryDependency(MissingLibraryDependency),
    #[serde(rename = "library-dependency-not-found")]
    LibraryDependencyNotFound(LibraryDependencyNotFound),
    #[serde(rename = "bad-library-dependency")]
    BadLibraryDependency(BadLibraryDependency),
}

impl Display for MachORule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MachORule::MissingRPathEntryDependency(rule) => write!(f, "{}", rule),
            MachORule::BadRPathEntry(rule) => write!(f, "{}", rule),
            MachORule::UnusedRPathEntry(rule) => write!(f, "{}", rule),
            MachORule::MissingLibraryDependency(rule) => write!(f, "{}", rule),
            MachORule::LibraryDependencyNotFound(rule) => write!(f, "{}", rule),
            MachORule::BadLibraryDependency(rule) => write!(f, "{}", rule),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "id", content = "options")]
pub(crate) enum MachORuleOptions {
    #[serde(rename = "missing-rpath-entry-dependency")]
    MissingRPathEntryDependency(MissingRPathEntryDependencyOptions),
    #[serde(rename = "bad-rpath-entry")]
    BadRPathEntry(BadRPathEntryOptions),
    #[serde(rename = "unused-rpath-entry")]
    UnusedRPathEntry(UnusedRPathEntryOptions),
    #[serde(rename = "missing-library-dependency")]
    MissingLibraryDependency(MissingLibraryDependencyOptions),
    #[serde(rename = "library-dependency-not-found")]
    LibraryDependencyNotFound(LibraryDependencyNotFoundOptions),
    #[serde(rename = "bad-library-dependency")]
    BadLibraryDependency(BadLibraryDependencyOptions),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingRPathEntryDependency {
    pub source: PathBuf,
    pub entry: PathBuf,
    pub dep_ident: PackageIdent,
}

impl Display for MissingRPathEntryDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: The rpath entry {} belongs to {} which is not a runtime dependency of this package", self.source.relative_package_path().unwrap().display().white(), self.entry.display().yellow(), self.dep_ident.yellow())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingRPathEntryDependencyOptions {
    #[serde(default = "MissingRPathEntryDependencyOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl MissingRPathEntryDependencyOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for MissingRPathEntryDependencyOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadRPathEntry {
    pub source: PathBuf,
    pub entry: PathBuf,
}

impl Display for BadRPathEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The rpath entry {} does not belong to a habitat package",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.entry.display().yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BadRPathEntryOptions {
    #[serde(default = "BadRPathEntryOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl BadRPathEntryOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for BadRPathEntryOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct UnusedRPathEntry {
    pub source: PathBuf,
    pub entry: PathBuf,
}

impl Display for UnusedRPathEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The rpath entry {} does not contain any required shared library",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.entry.display().yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct UnusedRPathEntryOptions {
    #[serde(default = "UnusedRPathEntryOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
    #[serde(default)]
    pub ignored_entries: GlobSetExpression,
}

impl UnusedRPathEntryOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for UnusedRPathEntryOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
            ignored_entries: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingLibraryDependency {
    pub source: PathBuf,
    pub library: String,
    pub dep_ident: PackageIdent,
}

impl Display for MissingLibraryDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The library {} belongs to {} which is not a runtime dependency of this package",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.library.yellow(),
            self.dep_ident.yellow()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingLibraryDependencyOptions {
    #[serde(default = "MissingLibraryDependencyOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl MissingLibraryDependencyOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for MissingLibraryDependencyOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LibraryDependencyNotFound {
    pub source: PathBuf,
    pub library: String,
}

impl Display for LibraryDependencyNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.library.contains("@rpath") {
            write!(
                f,
                "{}: The library {} could not be found in any rpath directories",
                self.source
                    .relative_package_path()
                    .unwrap()
                    .display()
                    .white(),
                self.library.yellow()
            )
        } else {
            write!(
                f,
                "{}: The library {} could not be found",
                self.source
                    .relative_package_path()
                    .unwrap()
                    .display()
                    .white(),
                self.library.yellow()
            )
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LibraryDependencyNotFoundOptions {
    #[serde(default = "LibraryDependencyNotFoundOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl LibraryDependencyNotFoundOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for LibraryDependencyNotFoundOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadLibraryDependency {
    pub source: PathBuf,
    pub library: String,
    pub library_path: PathBuf,
    pub macho_type: MachOType,
}

impl Display for BadLibraryDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The library {} at {} is a {}, it must be a shared library",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.library.yellow(),
            self.library_path.display().yellow(),
            self.macho_type
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct BadLibraryDependencyOptions {
    #[serde(default = "BadLibraryDependencyOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl BadLibraryDependencyOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for BadLibraryDependencyOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct MachOCheck {}

impl ArtifactCheck for MachOCheck {
    fn artifact_context_check(
        &self,
        store: &Store,
        rules: &PlanContextConfig,
        checker_context: &mut CheckerContext,
        _artifact_cache: &mut ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        let mut violations = vec![];
        let mut used_deps = HashSet::new();
        let tdep_artifacts = checker_context
            .tdeps
            .as_ref()
            .expect("Check context missing transitive dep artifacts");

        let missing_rpath_entry_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::MachO(MachORuleOptions::MissingRPathEntryDependency(
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
                if let ArtifactRuleOptions::MachO(MachORuleOptions::BadRPathEntry(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let unused_rpath_entry_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::MachO(MachORuleOptions::UnusedRPathEntry(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        let missing_library_dependency_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::MachO(MachORuleOptions::MissingLibraryDependency(
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

        let library_dependency_not_found_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::MachO(MachORuleOptions::LibraryDependencyNotFound(
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
                if let ArtifactRuleOptions::MachO(MachORuleOptions::BadLibraryDependency(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        for (macho_path, fat_metadata) in artifact_context.machos.iter() {
            let loader_path = macho_path.parent().unwrap().to_str().unwrap();
            for macho_arch_metadata in fat_metadata.archs.iter() {
                let mut unused_rpath_entries = macho_arch_metadata
                    .rpath
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>();
                // println!("BIN: {}", macho_path.display());
                // println!("NAME: {:?}", macho_arch_metadata.name);
                // println!("RPATH: {:?}", macho_arch_metadata.rpath);
                // println!("LIBS: {:?}", macho_arch_metadata.required_libraries);
                for library in macho_arch_metadata.required_libraries.iter() {
                    // Ignore this entry
                    if library == "self" {
                        continue;
                    }
                    if let Some(name) = &macho_arch_metadata.name {
                        if name == library {
                            continue;
                        }
                    }
                    let mut found = false;
                    // replace @rpath, @loader_path and @executable_path
                    if library.contains("@rpath") {
                        for rpath in macho_arch_metadata.rpath.iter() {
                            let normalized_rpath =
                                rpath.to_string_lossy().replace("@loader_path", loader_path);
                            let normalized_rpath = PathBuf::from(
                                normalized_rpath.replace("@executable_path", loader_path),
                            );
                            let search_path = normalized_rpath.absolutize().unwrap().to_path_buf();
                            let normalized_library_path = library.replace(
                                "@rpath",
                                search_path.to_str().expect("Invalid rpath entry"),
                            );
                            let library_path = PathBuf::from(normalized_library_path)
                                .absolutize()
                                .unwrap()
                                .to_path_buf();
                            assert!(library_path.is_absolute());
                            if let Some(dep_ident) =
                                library_path.package_ident(artifact_context.target)
                            {
                                if let Some(artifact) = tdep_artifacts.get(&dep_ident) {
                                    let dep_metadata = if let Some(dep_metadata) =
                                        artifact.machos.get(&library_path)
                                    {
                                        Some(dep_metadata)
                                    } else {
                                        let resolved_path = artifact
                                            .resolve_path(tdep_artifacts, library_path.as_path());
                                        if resolved_path != library_path {
                                            debug!(
                                                "In {}, following shared library path: {} -> {}",
                                                macho_path.display(),
                                                library_path.display(),
                                                resolved_path.display()
                                            );
                                            resolved_path
                                                .package_ident(artifact.target)
                                                .and_then(|p| tdep_artifacts.get(&p))
                                                .and_then(|a| a.machos.get(&resolved_path))
                                        } else {
                                            None
                                        }
                                    };
                                    if let Some(dep_metadata) = dep_metadata {
                                        for dep_arch_metadata in dep_metadata.archs.iter() {
                                            // Only check the metadata for the same arch
                                            if dep_arch_metadata.arch != macho_arch_metadata.arch {
                                                continue;
                                            }
                                            match dep_arch_metadata.file_type {
                                                MachOType::DynamicLibrary
                                                | MachOType::DynamicLibraryStub => {
                                                    found = true;
                                                    unused_rpath_entries.remove(rpath.as_path());
                                                    used_deps.insert(artifact.id.clone());
                                                    trace!(
                                                        "Found library {} required by {} at {}",
                                                        library,
                                                        macho_path.display(),
                                                        library_path.display()
                                                    );
                                                }
                                                _ => {
                                                    found = true;
                                                    unused_rpath_entries.remove(rpath.as_path());
                                                    used_deps.insert(artifact.id.clone());
                                                    trace!(
                                                        "Found library {} required by {} at {}",
                                                        library,
                                                        macho_path.display(),
                                                        library_path.display()
                                                    );
                                                    if !bad_library_dependency_options
                                                        .ignored_files
                                                        .is_match(
                                                            macho_path
                                                                .relative_package_path()
                                                                .unwrap(),
                                                        )
                                                    {
                                                        violations
                                                        .push(LeveledArtifactCheckViolation {
                                                            level: bad_library_dependency_options.level,
                                                            violation: ArtifactCheckViolation::MachO(
                                                                MachORule::BadLibraryDependency(
                                                                    BadLibraryDependency {
                                                                        source: macho_path.clone(),
                                                                        library:
                                                                            library
                                                                                .clone(),
                                                                        library_path: library_path.clone(),
                                                                        macho_type: dep_arch_metadata
                                                                            .file_type,
                                                                    },
                                                                ),
                                                            ),
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else if !missing_rpath_entry_dependency_options
                                    .ignored_files
                                    .is_match(macho_path.relative_package_path().unwrap())
                                {
                                    violations.push(LeveledArtifactCheckViolation {
                                        level: missing_rpath_entry_dependency_options.level,
                                        violation: ArtifactCheckViolation::MachO(
                                            MachORule::MissingRPathEntryDependency(
                                                MissingRPathEntryDependency {
                                                    source: macho_path.clone(),
                                                    entry: library_path,
                                                    dep_ident: dep_ident.clone(),
                                                },
                                            ),
                                        ),
                                    });
                                }
                            } else if MACOS_SYSTEM_LIBS.contains(library)
                                || MACOS_SYSTEM_DIRS.iter().any(|dir| library.starts_with(dir))
                            {
                                found = true;
                                trace!(
                                    "Ignoring system library {} required by {}",
                                    library,
                                    macho_path.display(),
                                );
                            }
                        }
                    } else {
                        let normalized_library_path = library.replace("@loader_path", loader_path);
                        let normalized_library_path =
                            normalized_library_path.replace("@executable_path", loader_path);
                        let library_path = PathBuf::from(normalized_library_path)
                            .absolutize()
                            .unwrap()
                            .to_path_buf();
                        assert!(library_path.is_absolute());
                        // Check if the libary is in a hab pkg
                        if let Some(dep_ident) = library_path.package_ident(artifact_context.target)
                        {
                            if let Some(artifact) = tdep_artifacts.get(&dep_ident) {
                                let dep_metadata = if let Some(dep_metadata) =
                                    artifact.machos.get(&library_path)
                                {
                                    Some(dep_metadata)
                                } else {
                                    let resolved_path = artifact
                                        .resolve_path(tdep_artifacts, library_path.as_path());
                                    if resolved_path != library_path {
                                        debug!(
                                            "In {}, following shared library path: {} -> {}",
                                            macho_path.display(),
                                            library_path.display(),
                                            resolved_path.display()
                                        );
                                        resolved_path
                                            .package_ident(artifact.target)
                                            .and_then(|p| tdep_artifacts.get(&p))
                                            .and_then(|a| a.machos.get(&resolved_path))
                                    } else {
                                        None
                                    }
                                };
                                if let Some(dep_metadata) = dep_metadata {
                                    for dep_arch_metadata in dep_metadata.archs.iter() {
                                        // Only check the metadata for the same arch
                                        if dep_arch_metadata.arch != macho_arch_metadata.arch {
                                            continue;
                                        }
                                        match dep_arch_metadata.file_type {
                                            MachOType::DynamicLibrary
                                            | MachOType::DynamicLibraryStub => {
                                                found = true;
                                                used_deps.insert(artifact.id.clone());
                                                trace!(
                                                    "Found library {} required by {} at {}",
                                                    library,
                                                    macho_path.display(),
                                                    library_path.display()
                                                );
                                            }
                                            _ => {
                                                found = true;
                                                used_deps.insert(artifact.id.clone());
                                                trace!(
                                                    "Found library {} required by {} at {}",
                                                    library,
                                                    macho_path.display(),
                                                    library_path.display()
                                                );
                                                if !bad_library_dependency_options
                                                    .ignored_files
                                                    .is_match(
                                                        macho_path.relative_package_path().unwrap(),
                                                    )
                                                {
                                                    violations.push(
                                                        LeveledArtifactCheckViolation {
                                                            level: bad_library_dependency_options
                                                                .level,
                                                            violation:
                                                                ArtifactCheckViolation::MachO(
                                                                    MachORule::BadLibraryDependency(
                                                                        BadLibraryDependency {
                                                                            source: macho_path
                                                                                .clone(),
                                                                            library: library
                                                                                .clone(),
                                                                            library_path: library_path.clone(),
                                                                            macho_type:
                                                                                dep_arch_metadata
                                                                                    .file_type,
                                                                        },
                                                                    ),
                                                                ),
                                                        },
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if !missing_library_dependency_options
                                .ignored_files
                                .is_match(macho_path.relative_package_path().unwrap())
                            {
                                violations.push(LeveledArtifactCheckViolation {
                                    level: missing_library_dependency_options.level,
                                    violation: ArtifactCheckViolation::MachO(
                                        MachORule::MissingLibraryDependency(
                                            MissingLibraryDependency {
                                                source: macho_path.clone(),
                                                library: library.clone(),
                                                dep_ident: dep_ident.clone(),
                                            },
                                        ),
                                    ),
                                });
                            }
                        } else if MACOS_SYSTEM_LIBS.contains(library)
                            || MACOS_SYSTEM_DIRS.iter().any(|dir| library.starts_with(dir))
                        {
                            found = true;
                            trace!(
                                "Ignoring system library {} required by {}",
                                library,
                                macho_path.display(),
                            );
                        }
                    }

                    if !found
                        && !library_dependency_not_found_options
                            .ignored_files
                            .is_match(macho_path.relative_package_path().unwrap())
                    {
                        violations.push(LeveledArtifactCheckViolation {
                            level: library_dependency_not_found_options.level,
                            violation: ArtifactCheckViolation::MachO(
                                MachORule::LibraryDependencyNotFound(LibraryDependencyNotFound {
                                    source: macho_path.clone(),
                                    library: library.clone(),
                                }),
                            ),
                        });
                    }
                }
                if !unused_rpath_entries.is_empty()
                    && !unused_rpath_entry_options
                        .ignored_files
                        .is_match(macho_path.relative_package_path().unwrap())
                {
                    for entry in unused_rpath_entries.iter() {
                        if !unused_rpath_entry_options.ignored_entries.is_match(entry) {
                            violations.push(LeveledArtifactCheckViolation {
                                level: unused_rpath_entry_options.level,
                                violation: ArtifactCheckViolation::MachO(
                                    MachORule::UnusedRPathEntry(UnusedRPathEntry {
                                        source: macho_path.clone(),
                                        entry: entry.to_path_buf(),
                                    }),
                                ),
                            });
                        }
                    }
                }
            }
        }
        for used_dep in used_deps {
            checker_context.mark_used(&used_dep);
        }

        violations.into_iter().collect()
    }
}
