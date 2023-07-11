use std::{
    fmt::Display,
    path::{Path, PathBuf},
    time::Instant,
};

use chrono::Duration;
use color_eyre::{
    eyre::{eyre, Context, Result},
    Help,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

use super::{Download, ShaSum};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct PackageSource {
    pub url: PackageSourceURL,
    pub shasum: PackageSha256Sum,
}

#[derive(Debug, Error)]
pub enum PackageSourceDownloadError {
    #[error("Downloaded source archive shasum does not match, expected {0} actual {1}")]
    Sha256SumMismatch(PackageSha256Sum, PackageSha256Sum),
    #[error("Unexpected IO error occurred while trying to download package source")]
    UnexpectedIOError(#[from] std::io::Error),
    #[error("Unexpected error occurred while trying to download package source")]
    UnexpectedError(#[from] color_eyre::eyre::Error),
}

impl PackageSource {
    pub fn download_and_verify_pkg_archive(
        &self,
        dest: impl AsRef<Path>,
    ) -> Result<Duration, PackageSourceDownloadError> {
        let start = Instant::now();
        debug!(
            "Downloading package source from {} to {}",
            self.url,
            dest.as_ref().display()
        );
        let mut download_attempts = 3;
        while download_attempts > 0 {
            match self.download_pkg_source(dest.as_ref()) {
                Ok(_) => {
                    break;
                }
                Err(_) if download_attempts > 0 => {
                    download_attempts -= 1;
                }
                Err(err) => return Err(PackageSourceDownloadError::UnexpectedError(err)),
            }
        }
        self.verify_pkg_archive(dest.as_ref())?;
        Ok(Duration::from_std(start.elapsed()).unwrap())
    }

    pub fn verify_pkg_archive(
        &self,
        dest: impl AsRef<Path>,
    ) -> Result<(), PackageSourceDownloadError> {
        let shasum = ShaSum::from_path(dest.as_ref())?;
        if *self.shasum.as_ref() != shasum {
            Err(PackageSourceDownloadError::Sha256SumMismatch(
                self.shasum.clone(),
                PackageSha256Sum(shasum),
            ))
        } else {
            debug!(
                "Verified package {} matches sha {}",
                dest.as_ref().display(),
                self.shasum.0
            );
            Ok(())
        }
    }

    fn download_pkg_source(&self, dest: impl AsRef<Path>) -> Result<()> {
        Download::new(&self.url.0, dest).execute()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct PackageSourceURL(Url);

impl PackageSourceURL {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageSourceURL> {
        Ok(PackageSourceURL(Url::parse(value.as_ref()).with_context(|| format!("Failed to parse package source url: {}", value.as_ref())).with_suggestion(|| "Please ensure your 'pkg_source' parameter contains a valid absolute URL like 'https://example.com'")?))
    }
    pub fn filename(&self) -> Result<PathBuf> {
        Ok(self
            .0
            .path()
            .split('/')
            .last()
            .ok_or_else(|| {
                eyre!(
                    "Package source url '{}' does not seem to refer to a file",
                    self.0
                )
            })?
            .into())
    }
}

impl Display for PackageSourceURL {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for PackageSourceURL {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        PackageSourceURL::parse(value)
    }
}

impl From<Url> for PackageSourceURL {
    fn from(value: Url) -> Self {
        PackageSourceURL(value)
    }
}

impl From<PackageSourceURL> for String {
    fn from(value: PackageSourceURL) -> Self {
        value.0.to_string()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]

pub struct PackageSha256Sum(ShaSum);

impl AsRef<ShaSum> for PackageSha256Sum {
    fn as_ref(&self) -> &ShaSum {
        &self.0
    }
}

impl From<String> for PackageSha256Sum {
    fn from(value: String) -> Self {
        PackageSha256Sum(ShaSum::from(value))
    }
}

impl From<PackageSha256Sum> for String {
    fn from(value: PackageSha256Sum) -> Self {
        String::from(value.0)
    }
}

impl Display for PackageSha256Sum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
