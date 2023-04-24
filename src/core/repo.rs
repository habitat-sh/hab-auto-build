use color_eyre::eyre::{eyre, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    path::{Path, PathBuf},
};

use super::{AutoBuildContextPath, PlanContextPath, GlobSetExpression};

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoConfig {
    pub id: String,
    pub source: PathBuf,
    #[serde(default)]
    pub native_packages: GlobSetExpression,
    #[serde(default)]
    pub ignored_packages: GlobSetExpression,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Hash, Serialize, Deserialize)]
pub(crate) struct RepoContextID(String);

impl Display for RepoContextID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub(crate) struct RepoContextPath(PathBuf);

impl TryFrom<PathBuf> for RepoContextPath {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: PathBuf) -> std::result::Result<Self, Self::Error> {
        let value = value
            .canonicalize()
            .with_context(|| eyre!("Failed to canonicalize path to repo: '{}'", value.display()))?;
        if !value.is_dir() {
            Err(eyre!(
                "The repo path '{}' must be an accessible directory",
                value.display()
            ))
        } else {
            Ok(RepoContextPath(value))
        }
    }
}

impl AsRef<Path> for RepoContextPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RepoContext {
    pub id: RepoContextID,
    pub path: RepoContextPath,
    #[serde(skip)]
    pub ignore_globset: GlobSetExpression,
    #[serde(skip)]
    pub native_globset: GlobSetExpression,
}

impl RepoContext {
    pub fn new(
        config: &RepoConfig,
        auto_build_ctx_path: &AutoBuildContextPath,
    ) -> Result<RepoContext> {
        Ok(RepoContext {
            id: RepoContextID(config.id.clone()),
            path: if config.source.is_absolute() {
                config.source.clone().try_into()?
            } else {
                auto_build_ctx_path
                    .as_ref()
                    .join(config.source.as_path())
                    .try_into()?
            },
            ignore_globset: config.ignored_packages.clone(),
            native_globset: config.native_packages.clone(),
        })
    }

    pub fn is_ignored_plan(&self, plan_ctx_path: &PlanContextPath) -> bool {
        let relative_path = plan_ctx_path
            .as_ref()
            .strip_prefix(self.path.as_ref())
            .expect("Plan does not belong to repo");
        self.ignore_globset.is_match(relative_path)
    }

    pub fn is_native_plan(&self, plan_ctx_path: &PlanContextPath) -> bool {
        let relative_path = plan_ctx_path
            .as_ref()
            .strip_prefix(self.path.as_ref())
            .expect("Plan does not belong to repo");
        self.native_globset.is_match(relative_path)
    }
}
