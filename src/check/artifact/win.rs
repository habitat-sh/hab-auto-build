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
    core::{ArtifactCache, ArtifactContext, PeType, GlobSetExpression, PackageIdent, PackagePath},
    store::Store,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum PeRule {
    #[serde(rename = "library-dependency-not-found")]
    LibraryDependencyNotFound(LibraryDependencyNotFound),
}

impl Display for PeRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeRule::LibraryDependencyNotFound(rule) => write!(f, "{}", rule),
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

#[derive(Debug, Default)]
pub(crate) struct PeCheck {}

impl ArtifactCheck for PeCheck {
    fn artifact_context_check(
        &self,
        _store: &Store,
        rules: &PlanContextConfig,
        checker_context: &mut CheckerContext,
        _artifact_cache: &mut ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        let mut violations = vec![];
        // let mut used_deps = HashSet::new();
        let tdep_artifacts = checker_context
            .tdeps
            .as_ref()
            .expect("Check context missing transitive dep artifacts");

        violations.into_iter().collect()
    }
}
