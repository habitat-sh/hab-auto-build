use color_eyre::eyre::{eyre, Context, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    path::{Path, PathBuf},
};

use crate::store::Store;

use super::{AutoBuildContextPath, Git, GlobSetExpression, PlanContextPath};

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoConfig {
    pub id: String,
    pub source: RepoSource,
    #[serde(default)]
    pub native_packages: GlobSetExpression,
    #[serde(default)]
    pub ignored_packages: GlobSetExpression,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum RepoSource {
    #[serde(rename = "git")]
    Git(GitRepo),
    #[serde(rename = "local")]
    Local(LocalRepo),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GitRepo {
    pub url: Url,
    pub commit: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalRepo {
    pub path: PathBuf,
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
    pub source: RepoSource,
    #[serde(skip)]
    pub ignore_globset: GlobSetExpression,
    #[serde(skip)]
    pub native_globset: GlobSetExpression,
}

impl RepoContext {
    pub fn new(
        config: &RepoConfig,
        store: &Store,
        auto_build_ctx_path: &AutoBuildContextPath,
    ) -> Result<RepoContext> {
        let path = match &config.source {
            RepoSource::Git(source_repo) => {
                // Checkout git repository
                Git::clone(store, source_repo)?;
                Git::checkout(store, source_repo)?
                    .as_ref()
                    .to_path_buf()
                    .try_into()?
            }
            RepoSource::Local(source_repo) => {
                if source_repo.path.is_absolute() {
                    source_repo.path.clone().try_into()?
                } else {
                    auto_build_ctx_path
                        .as_ref()
                        .join(&source_repo.path)
                        .try_into()?
                }
            }
        };

        Ok(RepoContext {
            id: RepoContextID(config.id.clone()),
            path,
            source: config.source.clone(),
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
