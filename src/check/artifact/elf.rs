use std::{collections::HashSet, fmt::Display, path::PathBuf};

use owo_colors::OwoColorize;
use path_absolutize::Absolutize;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use crate::{
    check::{
        ArtifactCheck, ArtifactCheckViolation, ArtifactRuleOptions, CheckerContext,
        LeveledArtifactCheckViolation, PlanContextConfig, ViolationLevel,
    },
    core::{ArtifactCache, ArtifactContext, ElfType, GlobSetExpression, PackageIdent, PackagePath},
    store::Store,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum ElfRule {
    #[serde(rename = "missing-rpath-entry-dependency")]
    MissingRPathEntryDependency(MissingRPathEntryDependency),
    #[serde(rename = "bad-rpath-entry")]
    BadRPathEntry(BadRPathEntry),
    #[serde(rename = "unused-rpath-entry")]
    UnusedRPathEntry(UnusedRPathEntry),
    #[serde(rename = "missing-runpath-entry-dependency")]
    MissingRunPathEntryDependency(MissingRunPathEntryDependency),
    #[serde(rename = "bad-runpath-entry")]
    BadRunPathEntry(BadRunPathEntry),
    #[serde(rename = "unused-runpath-entry")]
    UnusedRunPathEntry(UnusedRunPathEntry),
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

impl Display for ElfRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElfRule::MissingRPathEntryDependency(rule) => write!(f, "{}", rule),
            ElfRule::BadRPathEntry(rule) => write!(f, "{}", rule),
            ElfRule::UnusedRPathEntry(rule) => write!(f, "{}", rule),
            ElfRule::MissingRunPathEntryDependency(rule) => write!(f, "{}", rule),
            ElfRule::BadRunPathEntry(rule) => write!(f, "{}", rule),
            ElfRule::UnusedRunPathEntry(rule) => write!(f, "{}", rule),
            ElfRule::LibraryDependencyNotFound(rule) => write!(f, "{}", rule),
            ElfRule::BadLibraryDependency(rule) => write!(f, "{}", rule),
            ElfRule::BadELFInterpreter(rule) => write!(f, "{}", rule),
            ElfRule::HostELFInterpreter(rule) => write!(f, "{}", rule),
            ElfRule::ELFInterpreterNotFound(rule) => write!(f, "{}", rule),
            ElfRule::MissingELFInterpreterDependency(rule) => write!(f, "{}", rule),
            ElfRule::UnexpectedELFInterpreter(rule) => write!(f, "{}", rule),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "id", content = "options")]
pub(crate) enum ElfRuleOptions {
    #[serde(rename = "missing-rpath-entry-dependency")]
    MissingRPathEntryDependency(MissingRPathEntryDependencyOptions),
    #[serde(rename = "bad-rpath-entry")]
    BadRPathEntry(BadRPathEntryOptions),
    #[serde(rename = "unused-rpath-entry")]
    UnusedRPathEntry(UnusedRPathEntryOptions),
    #[serde(rename = "missing-runpath-entry-dependency")]
    MissingRunPathEntryDependency(MissingRunPathEntryDependencyOptions),
    #[serde(rename = "bad-runpath-entry")]
    BadRunPathEntry(BadRunPathEntryOptions),
    #[serde(rename = "unused-runpath-entry")]
    UnusedRunPathEntry(UnusedRunPathEntryOptions),
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
pub(crate) struct MissingRunPathEntryDependency {
    pub source: PathBuf,
    pub entry: PathBuf,
    pub dep_ident: PackageIdent,
}

impl Display for MissingRunPathEntryDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: The runpath entry {} belongs to {} which is not a runtime dependency of this package", self.source.relative_package_path().unwrap().display().white(), self.entry.display().yellow(), self.dep_ident.yellow())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingRunPathEntryDependencyOptions {
    #[serde(default = "MissingRunPathEntryDependencyOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl MissingRunPathEntryDependencyOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for MissingRunPathEntryDependencyOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadRunPathEntry {
    pub source: PathBuf,
    pub entry: PathBuf,
}

impl Display for BadRunPathEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The runpath entry {} does not belong to a habitat package",
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
pub(crate) struct BadRunPathEntryOptions {
    #[serde(default = "BadRunPathEntryOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl BadRunPathEntryOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for BadRunPathEntryOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct UnusedRunPathEntry {
    pub source: PathBuf,
    pub entry: PathBuf,
}

impl Display for UnusedRunPathEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The runpath entry {} does not contain any required shared library",
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
pub(crate) struct UnusedRunPathEntryOptions {
    #[serde(default = "UnusedRunPathEntryOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
    #[serde(default)]
    pub ignored_entries: GlobSetExpression,
}

impl UnusedRunPathEntryOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for UnusedRunPathEntryOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
            ignored_entries: GlobSetExpression::default(),
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
        write!(
            f,
            "{}: The library {} could not be found in any rpath / runpath directories",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white(),
            self.library.yellow()
        )
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
    pub elf_type: ElfType,
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
            self.elf_type
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

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingELFInterpreter {
    pub source: PathBuf,
}

impl Display for MissingELFInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The executable has no ELF interpreter",
            self.source
                .relative_package_path()
                .unwrap()
                .display()
                .white()
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingELFInterpreterOptions {
    #[serde(default = "MissingELFInterpreterOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl MissingELFInterpreterOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for MissingELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BadELFInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

impl Display for BadELFInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The ELF interpreter {} is not valid",
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
pub(crate) struct BadELFInterpreterOptions {
    #[serde(default = "BadELFInterpreterOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl BadELFInterpreterOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for BadELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HostELFInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

impl Display for HostELFInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The ELF interpreter {} does not belong to a habitat package",
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
pub(crate) struct HostELFInterpreterOptions {
    #[serde(default = "HostELFInterpreterOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl HostELFInterpreterOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for HostELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ELFInterpreterNotFound {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

impl Display for ELFInterpreterNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: The ELF interpreter {} could not be found in {}",
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
pub(crate) struct ELFInterpreterNotFoundOptions {
    #[serde(default = "ELFInterpreterNotFoundOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl ELFInterpreterNotFoundOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for ELFInterpreterNotFoundOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingELFInterpreterDependency {
    pub source: PathBuf,
    pub interpreter: PathBuf,
    pub interpreter_dependency: PackageIdent,
}

impl Display for MissingELFInterpreterDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: The ELF interpreter {} belongs to {} which is not a runtime dependency of this package", self.source.relative_package_path().unwrap().display().white(), self.interpreter.display().yellow(), self.interpreter_dependency.yellow())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingELFInterpreterDependencyOptions {
    #[serde(default = "MissingELFInterpreterDependencyOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl MissingELFInterpreterDependencyOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for MissingELFInterpreterDependencyOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct UnexpectedELFInterpreter {
    pub source: PathBuf,
    pub interpreter: PathBuf,
}

impl Display for UnexpectedELFInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: The ELF shared-library should not have an interpreter set, but found interpreter {}", self.source.relative_package_path().unwrap().display().white(), self.interpreter.display().yellow())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct UnexpectedELFInterpreterOptions {
    #[serde(default = "UnexpectedELFInterpreterOptions::level")]
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignored_files: GlobSetExpression,
}

impl UnexpectedELFInterpreterOptions {
    fn level() -> ViolationLevel {
        ViolationLevel::Error
    }
}

impl Default for UnexpectedELFInterpreterOptions {
    fn default() -> Self {
        Self {
            level: Self::level(),
            ignored_files: GlobSetExpression::default(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ElfCheck {}

impl ArtifactCheck for ElfCheck {
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

        let unused_rpath_entry_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::UnusedRPathEntry(options)) =
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

        let unused_runpath_entry_options = rules
            .artifact_rules
            .iter()
            .filter_map(|rule| {
                if let ArtifactRuleOptions::Elf(ElfRuleOptions::UnusedRunPathEntry(options)) =
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

            if let Some(interpreter_path) = metadata.interpreter.as_ref() {
                if !metadata.is_executable {
                    if !unexpected_elf_interpreter_options
                        .ignored_files
                        .is_match(path.relative_package_path().unwrap())
                    {
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
                if let Some(file_name) = interpreter_path.file_name().and_then(|x| x.to_str()) {
                    interpreter_name = Some(file_name.to_string());
                    if let Some(interpreter_dep) =
                        interpreter_path.package_ident(artifact_context.target)
                    {                        
                        if let Some(interpreter_artifact_ctx) = tdep_artifacts.get(&interpreter_dep)
                        {
                            if interpreter_artifact_ctx
                                .elfs
                                .get(interpreter_path.as_path())
                                .is_none()
                            {
                                let resolved_interpreter_path = interpreter_artifact_ctx
                                    .resolve_path(tdep_artifacts, interpreter_path.as_path());
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
                                        .and_then(|a| a.elfs.get(&resolved_interpreter_path))
                                        .is_none()
                                        && !elf_interpreter_not_found_options
                                            .ignored_files
                                            .is_match(path.relative_package_path().unwrap())
                                    {
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
                                    } else {
                                        used_deps.insert(interpreter_dep.clone());
                                    }
                                } else if !elf_interpreter_not_found_options
                                    .ignored_files
                                    .is_match(path.relative_package_path().unwrap())
                                {
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
                            } else {
                                used_deps.insert(interpreter_dep);
                            }
                        } else if interpreter_dep == artifact_context.id {
                            debug!("Interpreter belongs to the same package");
                        } else if !missing_elf_interpreter_dependency_options
                            .ignored_files
                            .is_match(path.relative_package_path().unwrap())
                        {
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
                    } else if !host_elf_interpreter_options
                        .ignored_files
                        .is_match(path.relative_package_path().unwrap())
                    {
                        violations.push(LeveledArtifactCheckViolation {
                            level: host_elf_interpreter_options.level,
                            violation: ArtifactCheckViolation::Elf(ElfRule::HostELFInterpreter(
                                HostELFInterpreter {
                                    source: path.clone(),
                                    interpreter: interpreter_path.clone(),
                                },
                            )),
                        });
                    }
                } else if !bad_elf_interpreter_options
                    .ignored_files
                    .is_match(path.relative_package_path().unwrap())
                {
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

            let mut unused_rpath_entries = metadata
                .rpath
                .iter()
                .map(|p| p.absolutize())
                .collect::<Result<HashSet<_>, _>>()
                .unwrap();
            let mut unused_runpath_entries = metadata
                .runpath
                .iter()
                .map(|p| p.absolutize())
                .collect::<Result<HashSet<_>, _>>()
                .unwrap();
            for library in metadata.required_libraries.iter() {
                let mut found = false;
                // If the library is the interpreter skip it
                if let Some(interpreter_name) = interpreter_name.as_ref() {
                    if interpreter_name == library.as_str() {
                        continue;
                    }
                }

                for search_path in metadata.rpath.iter() {
                    let search_path = search_path.absolutize().unwrap();
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
                                    trace!(
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
                                        trace!(
                                            "Found library {} required by {} in rpath entry {}",
                                            library,
                                            path.display(),
                                            search_path.display()
                                        );
                                        unused_rpath_entries.remove(&search_path);
                                        used_deps.insert(artifact.id.clone());
                                        break;
                                    }
                                    ElfType::Executable
                                    | ElfType::PieExecutable
                                    | ElfType::Other => {
                                        found = true;
                                        trace!(
                                            "Found library {} required by {} in rpath entry {}",
                                            library,
                                            path.display(),
                                            search_path.display()
                                        );
                                        unused_rpath_entries.remove(&search_path);
                                        used_deps.insert(artifact.id.clone());
                                        if !bad_library_dependency_options
                                            .ignored_files
                                            .is_match(path.relative_package_path().unwrap())
                                        {
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
                                        }
                                        break;
                                    }
                                }
                            }
                        } else if !missing_rpath_entry_dependency_options
                            .ignored_files
                            .is_match(path.relative_package_path().unwrap())
                        {
                            violations.push(LeveledArtifactCheckViolation {
                                level: missing_rpath_entry_dependency_options.level,
                                violation: ArtifactCheckViolation::Elf(
                                    ElfRule::MissingRPathEntryDependency(
                                        MissingRPathEntryDependency {
                                            source: path.clone(),
                                            entry: search_path.to_path_buf(),
                                            dep_ident: dep_ident.clone(),
                                        },
                                    ),
                                ),
                            });
                        }
                    } else if !bad_rpath_entry_options
                        .ignored_files
                        .is_match(path.relative_package_path().unwrap())
                    {
                        violations.push(LeveledArtifactCheckViolation {
                            level: bad_rpath_entry_options.level,
                            violation: ArtifactCheckViolation::Elf(ElfRule::BadRPathEntry(
                                BadRPathEntry {
                                    source: path.clone(),
                                    entry: search_path.to_path_buf(),
                                },
                            )),
                        });
                    }
                }

                for search_path in metadata.runpath.iter() {
                    let search_path = search_path.absolutize().unwrap();
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
                                    trace!(
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
                                        trace!(
                                            "Found library {} required by {} in runpath entry {}",
                                            library,
                                            path.display(),
                                            search_path.display()
                                        );
                                        unused_runpath_entries.remove(&search_path);
                                        used_deps.insert(artifact.id.clone());
                                        break;
                                    }
                                    ElfType::Executable
                                    | ElfType::PieExecutable
                                    | ElfType::Other => {
                                        found = true;
                                        trace!(
                                            "Found library {} required by {} in runpath entry {}",
                                            library,
                                            path.display(),
                                            search_path.display()
                                        );
                                        unused_runpath_entries.remove(&search_path);
                                        used_deps.insert(artifact.id.clone());
                                        if !bad_library_dependency_options
                                            .ignored_files
                                            .is_match(path.relative_package_path().unwrap())
                                        {
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
                                        }
                                        break;
                                    }
                                }
                            }
                        } else if !missing_runpath_entry_dependency_options
                            .ignored_files
                            .is_match(path.relative_package_path().unwrap())
                        {
                            violations.push(LeveledArtifactCheckViolation {
                                level: missing_runpath_entry_dependency_options.level,
                                violation: ArtifactCheckViolation::Elf(
                                    ElfRule::MissingRunPathEntryDependency(
                                        MissingRunPathEntryDependency {
                                            source: path.clone(),
                                            entry: search_path.to_path_buf(),
                                            dep_ident: dep_ident.clone(),
                                        },
                                    ),
                                ),
                            });
                        }
                    } else if !bad_runpath_entry_options
                        .ignored_files
                        .is_match(path.relative_package_path().unwrap())
                    {
                        violations.push(LeveledArtifactCheckViolation {
                            level: bad_runpath_entry_options.level,
                            violation: ArtifactCheckViolation::Elf(ElfRule::BadRunPathEntry(
                                BadRunPathEntry {
                                    source: path.clone(),
                                    entry: search_path.to_path_buf(),
                                },
                            )),
                        });
                    }
                }

                if !found
                    && !library_dependency_not_found_options
                        .ignored_files
                        .is_match(path.relative_package_path().unwrap())
                {
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
            if !unused_rpath_entries.is_empty()
                && !unused_rpath_entry_options
                    .ignored_files
                    .is_match(path.relative_package_path().unwrap())
            {
                for entry in unused_rpath_entries.iter() {
                    if !unused_rpath_entry_options.ignored_entries.is_match(entry) {
                        violations.push(LeveledArtifactCheckViolation {
                            level: unused_rpath_entry_options.level,
                            violation: ArtifactCheckViolation::Elf(ElfRule::UnusedRPathEntry(
                                UnusedRPathEntry {
                                    source: path.clone(),
                                    entry: entry.to_path_buf(),
                                },
                            )),
                        });
                    }
                }
            }
            if !unused_runpath_entries.is_empty()
                && !unused_runpath_entry_options
                    .ignored_files
                    .is_match(path.relative_package_path().unwrap())
            {
                for entry in unused_runpath_entries.iter() {
                    if !unused_runpath_entry_options.ignored_entries.is_match(entry) {
                        violations.push(LeveledArtifactCheckViolation {
                            level: unused_runpath_entry_options.level,
                            violation: ArtifactCheckViolation::Elf(ElfRule::UnusedRunPathEntry(
                                UnusedRunPathEntry {
                                    source: path.clone(),
                                    entry: entry.to_path_buf(),
                                },
                            )),
                        });
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
