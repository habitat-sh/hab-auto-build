use anyhow::{anyhow, Context, Result};

use askalono::{ScanMode, ScanStrategy, Store, TextData};

use bzip2::bufread::BzDecoder;
use colored::Colorize;
use flate2::bufread::GzDecoder;
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use goblin::Object;
use headway::ProgressBarIterable;
use infer::Infer;
use lazy_static::lazy_static;
use reqwest::{
    header::{self, CONTENT_DISPOSITION, LOCATION},
    redirect::Policy,
    Method, RequestBuilder, Url,
};
use sha2::{Digest, Sha256};
use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    env,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};
use tar::Archive;
use tempdir::TempDir;
use tokio::{
    fs::{remove_file, rename, File},
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
};
use tracing::{debug, error, info, trace, warn};
use xz2::bufread::XzDecoder;

use crate::{
    PackageArtifact, PackageIdent, PackageMetadata, PackageType, ValidFilePath, HAB_CACHE_SRC_PATH,
    HAB_PKGS_PATH,
};

lazy_static! {
    static ref PLATFORM_SHELLS: Vec<PathBuf> = vec![
        PathBuf::from(option_env!("HAB_PLATFORM_SHELL").unwrap_or("/bin/sh")),
        PathBuf::from("/bin/false")
    ];
    static ref MANIFEST_METAFILE: PathBuf = PathBuf::from("MANIFEST");
}

const LICENSE_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/license-cache.bin.gz"));
const DEPRECATED_LICENSE_DATA: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/deprecated-license-cache.bin.gz"));

const LICENSE_GLOBS: &[&str] = &[
    // General
    "COPYING",
    "COPYING[.-]*",
    "COPYRIGHT",
    "COPYRIGHT[.-]*",
    "EULA",
    "EULA[.-]*",
    "licen[cs]e",
    "licen[cs]e.*",
    "LICEN[CS]E",
    "LICEN[CS]E[.-]*",
    "*[.-]LICEN[CS]E*",
    "NOTICE",
    "NOTICE[.-]*",
    "PATENTS",
    "PATENTS[.-]*",
    "UNLICEN[CS]E",
    "UNLICEN[CS]E[.-]*",
    // GPL (gpl.txt, etc.)
    "agpl[.-]*",
    "gpl[.-]*",
    "lgpl[.-]*",
    // Other license-specific (APACHE-2.0.txt, etc.)
    "AGPL-*[0-9]*",
    "APACHE-*[0-9]*",
    "BSD-*[0-9]*",
    "CC-BY-*",
    "GFDL-*[0-9]*",
    "GNU-*[0-9]*",
    "GPL-*[0-9]*",
    "LGPL-*[0-9]*",
    "MIT-*[0-9]*",
    "MPL-*[0-9]*",
    "OFL-*[0-9]*",
];

pub struct LicenseCheck {
    file_type_checker: Infer,
    fs_root: PathBuf,
    license_globs: GlobSet,
    license_store: Store,
    deprecated_license_store: Store,
}

impl LicenseCheck {
    fn id(&self) -> &'static str {
        "LICENSE_CHECK"
    }
    fn description(&self) -> &'static str {
        "Checks package licenses"
    }
    pub fn new(fs_root: impl AsRef<Path>) -> Result<LicenseCheck> {
        let license_store = Store::from_cache(LICENSE_DATA)?;
        let deprecated_license_store = Store::from_cache(DEPRECATED_LICENSE_DATA)?;
        debug!("{} licenses loaded", license_store.licenses().count());
        debug!(
            "{} deprecated licenses loaded",
            deprecated_license_store.licenses().count()
        );
        let mut builder = GlobSetBuilder::new();
        // A GlobBuilder can be used to configure each glob's match semantics
        // independently.
        for glob in LICENSE_GLOBS {
            builder.add(
                GlobBuilder::new(format!("**/{}", glob).as_str())
                    .literal_separator(true)
                    .build()?,
            );
        }

        let license_globs = builder.build()?;
        Ok(LicenseCheck {
            file_type_checker: Infer::new(),
            license_store,
            deprecated_license_store,
            license_globs,
            fs_root: fs_root.as_ref().to_path_buf(),
        })
    }
    async fn visit_dir_start(
        &mut self,
        _abs_path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        Ok(DirReport::default())
    }
    async fn visit_dir_end(
        &mut self,
        _path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        Ok(DirReport::default())
    }
    async fn visit_child_dir(
        &mut self,
        _path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        Ok(DirReport::default())
    }
    async fn visit_file(
        &mut self,
        path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        if rel_path.as_ref() == MANIFEST_METAFILE.as_path() {
            let file = tokio::fs::File::open(path).await?;
            let mut reader = tokio::io::BufReader::new(file);
            let mut pkg_source = None;
            let mut pkg_shasum = None;
            let mut pkg_licenses = None;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(src) = line.strip_prefix("* __Source__:") {
                            let src = src.trim().split_terminator(&['[', ']']).collect::<Vec<_>>();
                            if let Some(url) = src.get(1) {
                                pkg_source = Some(Url::parse(url)?);
                            }
                        }
                        if let Some(shasum) = line.strip_prefix("* __SHA__:") {
                            let patterns: &[_] = &[' ', '`', '\n'];
                            pkg_shasum = Some(shasum.trim_matches(patterns).to_owned());
                        }
                        if let Some(licenses) = line.strip_prefix("* __License__:") {
                            pkg_licenses = Some(
                                licenses
                                    .trim()
                                    .split(' ')
                                    .map(String::from)
                                    .collect::<Vec<String>>(),
                            );
                        }
                    }
                    Err(err) => {
                        error!("Failed to read package MANIFEST file");
                        break;
                    }
                }
            }
            if let (Some(url), Some(sha)) = (pkg_source, pkg_shasum) {
                let mut report = FileReport::default();
                let (detected_licenses, suspected_licenses) =
                    self.check(self.fs_root.as_path(), url, &sha).await?;
                if !detected_licenses.is_empty() {
                    if let Some(pkg_licenses) = pkg_licenses {
                        for pkg_license in pkg_licenses.iter() {
                            if !detected_licenses.contains(pkg_license) {
                                report.warnings.push(format!("Package has license '{}' which was not found in the source, detected licenses: {:?}", pkg_license, detected_licenses))
                            }
                        }
                        let pkg_licenses = pkg_licenses.iter().cloned().collect::<BTreeSet<_>>();
                        if report.warnings.is_empty() && detected_licenses != pkg_licenses {
                            let additional_licenses = detected_licenses
                                .difference(&pkg_licenses)
                                .collect::<BTreeSet<_>>();
                            if !additional_licenses.is_empty() {
                                report.warnings.push(format!(
                                    "Package has licenses {:?}, however additional licenses were detected in source: {:?}",
                                    pkg_licenses,
                                    additional_licenses
                                ));
                            }
                        }
                    } else {
                        report.errors.push(format!("Package has no licenses specified but the following licenses were detected: {:?}", detected_licenses))
                    }
                } else if let Some(pkg_licenses) = pkg_licenses {
                    for pkg_license in pkg_licenses.iter() {
                        if !suspected_licenses.contains(pkg_license) {
                            if suspected_licenses.is_empty() {
                                report.warnings.push(format!("Package has license '{}' which could not be detected in the source, no licenses could be detected in the source", pkg_license));
                            } else {
                                report.warnings.push(format!("Package has license '{}' which could not be detected in the source, suspected licenses are: {:?}", pkg_license, suspected_licenses));
                            }
                        }
                    }
                    let pkg_licenses = pkg_licenses.iter().cloned().collect::<BTreeSet<_>>();
                    if suspected_licenses != pkg_licenses {
                        let additional_licenses = detected_licenses
                            .difference(&pkg_licenses)
                            .collect::<BTreeSet<_>>();
                        if report.warnings.is_empty() && !additional_licenses.is_empty() {
                            report.warnings.push(format!(
                                "Package has licenses {:?}, however additional suspected licenses were detected in source: {:?}",
                                pkg_licenses,
                                additional_licenses
                            ));
                        }
                    }
                } else {
                    report.warnings.push(format!("Package has no license specified but the following licenses are suspected to be present in the source: {:?}", suspected_licenses))
                }
                Ok(report)
            } else {
                Ok(FileReport::default())
            }
        } else {
            Ok(FileReport::default())
        }
    }
    async fn visit_symlink(
        &mut self,
        _path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        Ok(FileReport::default())
    }
    async fn visit_package_end(&mut self) -> Result<PackageReport> {
        Ok(PackageReport::default())
    }
    pub async fn check(
        &self,
        fs_root: impl AsRef<Path>,
        package_source: Url,
        package_shasum: &str,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
        let package_archive_name: String = package_source
            .path()
            .split('/')
            .last()
            .ok_or_else(|| anyhow!("Invalid package source url: {}", package_source))?
            .to_owned();
        let mut package_archive = fs_root
            .as_ref()
            .join(HAB_CACHE_SRC_PATH.as_path())
            .join(&package_archive_name);
        let mut package_archive_is_verified = false;
        if package_archive.is_file() {
            if let Err(err) = self
                .verify_pkg_archive(package_archive.as_path(), package_shasum)
                .await
            {
                warn!(
                    "Existing package source archive failed verification: {}",
                    err
                );
                package_archive = fs_root
                    .as_ref()
                    .join(HAB_CACHE_SRC_PATH.as_path())
                    .join(format!("{}-{}", package_shasum, &package_archive_name));
            } else {
                package_archive_is_verified = true;
            }
        }
        if package_archive.is_file() {
            if let Err(err) = self
                .verify_pkg_archive(package_archive.as_path(), package_shasum)
                .await
            {
                warn!(
                    "Previously downloaded package source archive failed verification: {}",
                    err
                );
            } else {
                package_archive_is_verified = true;
            }
        }

        if !package_archive_is_verified {
            let tmp_dir = TempDir::new("hab-auto-build-download")?;
            let tmp_package_archive = tmp_dir.path().join(&package_archive_name);
            match self
                .download_and_verify_pkg_archive(
                    &package_source,
                    package_shasum,
                    tmp_package_archive.as_path(),
                )
                .await
            {
                Ok(_) => {}
                Err(err) => {
                    let debug_path = std::env::temp_dir().join(&package_archive_name);
                    error!("There was a problem downloading and verifying the package archive from {}, you can check the downloaded file data at {}: {:#}", package_source, debug_path.display(), err);
                    tokio::fs::rename(tmp_package_archive.as_path(), debug_path).await?;
                }
            };
            match tokio::fs::rename(tmp_package_archive.as_path(), package_archive.as_path()).await
            {
                Ok(_) => self.scan_package_archive(package_archive),
                Err(_) => {
                    // We may be unable to copy the file to the destination folder due to permission issues
                    self.scan_package_archive(tmp_package_archive)
                }
            }
        } else {
            self.scan_package_archive(package_archive)
        }
    }

    async fn download_and_verify_pkg_archive(
        &self,
        package_source: &Url,
        package_shasum: &str,
        package_archive: impl AsRef<Path>,
    ) -> Result<()> {
        debug!(
            "Downloading source to {}",
            package_archive.as_ref().display()
        );
        let mut download_attempts = 3;
        while download_attempts > 0 {
            match self
                .download_pkg_source(package_source, package_archive.as_ref())
                .await
            {
                Ok(_) => {
                    break;
                }
                Err(_) if download_attempts > 0 => {
                    download_attempts -= 1;
                }
                Err(err) => return Err(err),
            }
        }
        self.verify_pkg_archive(package_archive.as_ref(), package_shasum)
            .await?;

        Ok(())
    }

    async fn verify_pkg_archive(
        &self,
        package_archive: impl AsRef<Path>,
        package_shasum: &str,
    ) -> Result<()> {
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 4096];
        let file = tokio::fs::File::open(package_archive.as_ref()).await?;
        let mut reader = tokio::io::BufReader::new(file);
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    break;
                }
                Ok(n) => {
                    hasher.update(&buffer[..n]);
                }
                Err(err) => return Err(anyhow!("Error reading archive: {}", err)),
            }
        }

        // read hash digest and consume hasher
        let result = hasher.finalize();
        let downloaded_shasum: String = result
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<String>>()
            .join("");

        if package_shasum != downloaded_shasum {
            Err(anyhow!(
                "Package shasum does not match: {}, expected {}",
                downloaded_shasum,
                package_shasum
            ))
        } else {
            debug!(
                "Verified package {} matches sha {}",
                package_archive.as_ref().display(),
                package_shasum
            );
            Ok(())
        }
    }

    async fn download_pkg_source(
        &self,
        package_url: &Url,
        package_archive: impl AsRef<Path>,
    ) -> Result<()> {
        let client = reqwest::ClientBuilder::new()
            .redirect(Policy::none())
            .gzip(false)
            .deflate(false)
            .tcp_nodelay(true)
            .build()?;
        let mut url = package_url.to_owned();
        let mut final_response = None;
        let mut base_headers = reqwest::header::HeaderMap::new();
        // We put a common user agent as some remote hosts forbid downloads otherwise
        base_headers.append("User-Agent", "curl/7.68.0".parse().unwrap());
        let mut additional_headers = reqwest::header::HeaderMap::new();
        while final_response.is_none() {
            let mut request = reqwest::Request::new(Method::GET, url.clone());
            request
                .headers_mut()
                .extend(base_headers.clone().into_iter());
            request
                .headers_mut()
                .extend(additional_headers.clone().into_iter());
            additional_headers.clear();

            let response = client.execute(request).await?;
            let headers = response.headers();
            if let Some(content_disposition) = headers.get(header::CONTENT_DISPOSITION) {
                if let Ok(value) = content_disposition.to_str() {
                    debug!("Received content disposition header {}", value);
                    if value.trim().starts_with("attachment") {
                        additional_headers
                            .append(header::CONTENT_DISPOSITION, content_disposition.to_owned());
                    }
                }
            }
            if let Some(redirect_url) = headers.get(header::LOCATION) {
                debug!("Redirecting to {}", redirect_url.to_str()?);
                url = Url::parse(redirect_url.to_str()?)?;
            } else {
                final_response = Some(response);
            }
        }

        if let Some(mut response) = final_response {
            let mut file = File::create(package_archive.as_ref()).await?;
            while let Some(chunk) = response.chunk().await.with_context(|| {
                anyhow!(
                    "Failed to download package source from {} to {}",
                    package_url,
                    package_archive.as_ref().display()
                )
            })? {
                file.write_all(&chunk).await?;
            }
            file.shutdown().await?;
            debug!(
                "Package source downloaded from {} to {}",
                url,
                package_archive.as_ref().display()
            );
            Ok(())
        } else {
            Err(anyhow!("Failed to download package archive from {}", url))
        }
    }
    fn scan_package_archive(
        &self,
        package_archive: impl AsRef<Path>,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
        debug!(
            "Scanning package archive {}",
            package_archive.as_ref().display()
        );
        let file = std::fs::File::open(package_archive.as_ref())?;
        let reader = std::io::BufReader::new(file);
        match package_archive
            .as_ref()
            .extension()
            .and_then(|x| x.to_str())
        {
            Some("bz2" | "tb2" | "tbz" | "tbz2" | "tz2") => {
                self.scan_package_archive_contents(BzDecoder::new(reader))
            }
            Some("gz" | "taz" | "tgz") => {
                self.scan_package_archive_contents(GzDecoder::new(reader))
            }
            Some("lzma" | "tlz") => self.scan_package_archive_contents(XzDecoder::new(reader)),
            Some("xz" | "txz") => self.scan_package_archive_contents(XzDecoder::new(reader)),
            Some("zst" | "tzst") => {
                self.scan_package_archive_contents(zstd::stream::read::Decoder::new(reader)?)
            }
            Some("tar") => self.scan_package_archive_contents(reader),
            None => {
                let file_type = self
                    .file_type_checker
                    .get_from_path(package_archive.as_ref())
                    .ok()
                    .unwrap_or(None)
                    .map(|f| f.mime_type().to_string());
                if let Some(file_type) = file_type {
                    match file_type.as_str() {
                        "application/gzip" => {
                            return self.scan_package_archive_contents(GzDecoder::new(reader));
                        }
                        _ => {
                            error!(
                                "Detected archive {} as {}, unsure how to process it",
                                package_archive.as_ref().display(),
                                file_type
                            );
                        }
                    }
                } else {
                    error!(
                        "Unable to detect compression for '{}', please ensure it is named correctly",
                        package_archive.as_ref().display(),
                    );
                }
                Ok((BTreeSet::default(), BTreeSet::default()))
            }
            Some(_) => {
                warn!("Could not scan package archive for licenses");
                Ok((BTreeSet::default(), BTreeSet::default()))
            }
        }
    }

    fn scan_package_archive_contents(
        &self,
        decoder: impl Read,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
        let mut licenses = BTreeSet::default();
        let mut suspected_licenses = BTreeSet::default();
        let mut tar = Archive::new(decoder);
        let scan_strategies = vec![
            ScanStrategy::new(&self.license_store)
                .confidence_threshold(0.8)
                .mode(ScanMode::Elimination)
                .max_passes(5)
                .optimize(true),
            ScanStrategy::new(&self.deprecated_license_store)
                .confidence_threshold(0.8)
                .mode(ScanMode::Elimination)
                .max_passes(5)
                .optimize(true),
        ];
        let deep_scan_strategies = vec![
            ScanStrategy::new(&self.license_store)
                .confidence_threshold(0.8)
                .mode(ScanMode::TopDown)
                .max_passes(50)
                .optimize(true),
            ScanStrategy::new(&self.deprecated_license_store)
                .confidence_threshold(0.8)
                .mode(ScanMode::TopDown)
                .max_passes(50)
                .optimize(true),
        ];
        let suspect_scan_strategies = vec![
            ScanStrategy::new(&self.license_store)
                .confidence_threshold(0.3)
                .mode(ScanMode::TopDown)
                .max_passes(5)
                .optimize(true),
            ScanStrategy::new(&self.deprecated_license_store)
                .confidence_threshold(0.3)
                .mode(ScanMode::TopDown)
                .max_passes(5)
                .optimize(true),
        ];
        for entry in tar.entries()? {
            match entry {
                Ok(entry) => {
                    if !entry.header().entry_type().is_file() {
                        trace!("Skipping entry {} in archive", entry.path()?.display());
                        continue;
                    }
                    let entry_path = entry.path()?.to_path_buf();
                    if self.license_globs.is_match(entry_path.as_path()) {
                        trace!("Checking entry {} in archive", entry.path()?.display());
                        let mut reader = std::io::BufReader::new(entry);

                        let mut file_data = String::new();
                        match reader.read_to_string(&mut file_data) {
                            Ok(_) => {
                                let data: TextData = file_data.into();
                                let mut file_licenses = BTreeSet::new();
                                for strategy in scan_strategies.iter() {
                                    let results = strategy.scan(&data)?;
                                    for item in results.containing {
                                        file_licenses.insert(item.license.name.to_string());
                                        debug!(
                                            "{} detected in {}",
                                            item.license.name,
                                            entry_path.display()
                                        );
                                    }
                                }
                                if file_licenses.is_empty() {
                                    // Do a lower quality scan if we haven't detected any licenses yet
                                    for strategy in suspect_scan_strategies.iter() {
                                        let results = strategy.scan(&data)?;
                                        for item in results.containing {
                                            file_licenses.insert(item.license.name.to_string());
                                            debug!(
                                                "{} suspected in {}",
                                                item.license.name,
                                                entry_path.display()
                                            );
                                        }
                                    }
                                    suspected_licenses.append(&mut file_licenses);
                                } else if file_licenses.len() >= 5 {
                                    // Do a more costly scan for licenses if we find a lot of them
                                    for strategy in deep_scan_strategies.iter() {
                                        let results = strategy.scan(&data)?;
                                        for item in results.containing {
                                            file_licenses.insert(item.license.name.to_string());
                                            debug!(
                                                "{} detected in {}",
                                                item.license.name,
                                                entry_path.display()
                                            );
                                        }
                                    }
                                }
                                licenses.append(&mut file_licenses);
                            }
                            Err(err) => {
                                trace!("Unable to read file {}: {}", entry_path.display(), err);
                            }
                        };
                    } else {
                        trace!("Skipping entry {} in archive", entry.path()?.display());
                    }
                }
                Err(err) => {
                    error!("Error reading entries: {}", err);
                }
            }
        }
        Ok((licenses, suspected_licenses))
    }
}

pub struct ArtifactChecker<'a> {
    artifact: PackageArtifact,
    checks: Vec<Check<'a>>,
}

impl<'a> ArtifactChecker<'a> {
    pub async fn new(
        artifact: PackageArtifact,
        metadata: &'a PackageMetadata,
        fs_root: impl AsRef<Path>,
    ) -> Result<ArtifactChecker<'a>> {
        let checks = vec![
            Check::EmptyTopLevelDir(EmptyTopLevelDirCheck::default()),
            Check::Dependency(DependencyCheck::new(metadata)),
            Check::LicenseCheck(LicenseCheck::new(fs_root)?),
        ];

        Ok(ArtifactChecker { artifact, checks })
    }

    pub async fn check(&mut self) -> Result<ArtifactReport> {
        let mut report = ArtifactReport::new(
            self.artifact.ident.borrow().into(),
            self.artifact.path.clone(),
        );
        let mut next_dirs = VecDeque::new();
        let install_dir = self.artifact.install_dir();
        next_dirs.push_back(install_dir.clone());
        while !next_dirs.is_empty() {
            let current_dir = next_dirs.pop_front().unwrap();
            trace!("Checking {}", current_dir.display());
            let rel_current_dir = current_dir
                .as_path()
                .strip_prefix(install_dir.as_path())
                .unwrap();
            let mut read_dir = tokio::fs::read_dir(current_dir.as_path())
                .await
                .with_context(|| anyhow!("Failed to read directory {}", current_dir.display()))?;
            for check in self.checks.iter_mut() {
                report.dir_report_append(
                    rel_current_dir,
                    check
                        .visit_dir_start(current_dir.as_path(), rel_current_dir)
                        .await?,
                );
            }
            while let Some(entry) = read_dir.next_entry().await.with_context(|| {
                anyhow!(
                    "Failed to read next entry from directory {}",
                    current_dir.display()
                )
            })? {
                let entry_path = entry.path();
                let entry_metadata = entry.metadata().await.with_context(|| {
                    anyhow!(
                        "Failed to directory entry metadata {}",
                        entry_path.display()
                    )
                })?;
                let rel_entry_path = entry_path
                    .as_path()
                    .strip_prefix(install_dir.as_path())
                    .unwrap();

                if entry_metadata.is_dir() {
                    for check in self.checks.iter_mut() {
                        report.dir_report_append(
                            rel_entry_path,
                            check
                                .visit_child_dir(entry_path.as_path(), rel_entry_path)
                                .await?,
                        );
                    }
                    next_dirs.push_back(entry.path());
                } else if entry_metadata.is_file() {
                    trace!("Checking {}", entry_path.display());
                    for check in self.checks.iter_mut() {
                        report.file_report_append(
                            rel_entry_path,
                            check
                                .visit_file(entry_path.as_path(), rel_entry_path)
                                .await?,
                        );
                    }
                } else if entry_metadata.is_symlink() {
                    trace!("Checking symlink {}", entry_path.display());
                    for check in self.checks.iter_mut() {
                        report.file_report_append(
                            rel_entry_path,
                            check
                                .visit_symlink(entry_path.as_path(), rel_entry_path)
                                .await?,
                        );
                    }
                }
            }
            for check in self.checks.iter_mut() {
                report.dir_report_append(
                    rel_current_dir,
                    check
                        .visit_dir_end(current_dir.as_path(), rel_current_dir)
                        .await?,
                );
            }
        }
        for check in self.checks.iter_mut() {
            report.package_report_append(check.visit_package_end().await?);
        }
        Ok(report)
    }
}

pub enum Check<'a> {
    EmptyTopLevelDir(EmptyTopLevelDirCheck),
    LicenseCheck(LicenseCheck),
    Dependency(DependencyCheck<'a>),
}

impl<'a> Check<'a> {
    fn id(&self) -> &'static str {
        match self {
            Check::EmptyTopLevelDir(check) => check.id(),
            Check::Dependency(check) => check.id(),
            Check::LicenseCheck(check) => check.id(),
        }
    }
    fn description(&self) -> &'static str {
        match self {
            Check::EmptyTopLevelDir(check) => check.description(),
            Check::Dependency(check) => check.description(),
            Check::LicenseCheck(check) => check.description(),
        }
    }
    async fn visit_dir_start(
        &mut self,
        abs_path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        match self {
            Check::EmptyTopLevelDir(ref mut check) => {
                check.visit_dir_start(&abs_path, &rel_path).await
            }
            Check::Dependency(ref mut check) => check.visit_dir_start(&abs_path, &rel_path).await,
            Check::LicenseCheck(ref mut check) => check.visit_dir_start(&abs_path, &rel_path).await,
        }
        .with_context(|| {
            anyhow!(
                "Check {} failed while entering directory '{}'",
                self.id(),
                abs_path.as_ref().display()
            )
        })
    }
    async fn visit_dir_end(
        &mut self,
        abs_path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        match self {
            Check::EmptyTopLevelDir(ref mut check) => {
                check.visit_dir_end(&abs_path, &rel_path).await
            }
            Check::Dependency(ref mut check) => check.visit_dir_end(&abs_path, &rel_path).await,
            Check::LicenseCheck(ref mut check) => check.visit_dir_end(&abs_path, &rel_path).await,
        }
        .with_context(|| {
            anyhow!(
                "Check {} failed while exiting directory '{}'",
                self.id(),
                abs_path.as_ref().display()
            )
        })
    }
    async fn visit_child_dir(
        &mut self,
        abs_path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        match self {
            Check::EmptyTopLevelDir(ref mut check) => {
                check.visit_child_dir(&abs_path, &rel_path).await
            }
            Check::Dependency(ref mut check) => check.visit_child_dir(&abs_path, &rel_path).await,
            Check::LicenseCheck(ref mut check) => check.visit_child_dir(&abs_path, &rel_path).await,
        }
        .with_context(|| {
            anyhow!(
                "Check {} failed while checking child directory '{}'",
                self.id(),
                &abs_path.as_ref().display()
            )
        })
    }
    async fn visit_file(
        &mut self,
        abs_path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        match self {
            Check::EmptyTopLevelDir(ref mut check) => check.visit_file(&abs_path, &rel_path).await,
            Check::Dependency(ref mut check) => check.visit_file(&abs_path, &rel_path).await,
            Check::LicenseCheck(ref mut check) => check.visit_file(&abs_path, &rel_path).await,
        }
        .with_context(|| {
            anyhow!(
                "Check {} failed while checking file '{}'",
                self.id(),
                &abs_path.as_ref().display()
            )
        })
    }

    async fn visit_symlink(
        &mut self,
        abs_path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        match self {
            Check::EmptyTopLevelDir(ref mut check) => {
                check.visit_symlink(&abs_path, &rel_path).await
            }
            Check::Dependency(ref mut check) => check.visit_symlink(&abs_path, &rel_path).await,
            Check::LicenseCheck(ref mut check) => check.visit_symlink(&abs_path, &rel_path).await,
        }
        .with_context(|| {
            anyhow!(
                "Check {} failed while checking symlink '{}'",
                self.id(),
                &abs_path.as_ref().display()
            )
        })
    }
    async fn visit_package_end(&mut self) -> Result<PackageReport> {
        match self {
            Check::EmptyTopLevelDir(ref mut check) => check.visit_package_end().await,
            Check::Dependency(ref mut check) => check.visit_package_end().await,
            Check::LicenseCheck(ref mut check) => check.visit_package_end().await,
        }
        .with_context(|| anyhow!("Check {} failed while completing package checks", self.id()))
    }
}

#[derive(Debug)]
pub struct ArtifactReport {
    ident: PackageIdent,
    file_path: ValidFilePath,
    warnings: u64,
    errors: u64,
    package_issues: PackageReport,
    dir_issues: BTreeMap<PathBuf, DirReport>,
    file_issues: BTreeMap<PathBuf, FileReport>,
}

impl ArtifactReport {
    pub fn new(ident: PackageIdent, file_path: ValidFilePath) -> ArtifactReport {
        ArtifactReport {
            ident,
            file_path,
            warnings: 0,
            errors: 0,
            package_issues: PackageReport::default(),
            dir_issues: BTreeMap::default(),
            file_issues: BTreeMap::default(),
        }
    }

    pub fn status(&self) -> ReportStatus {
        if self.errors != 0 {
            return ReportStatus::Error;
        }
        if self.warnings != 0 {
            return ReportStatus::Warning;
        }
        ReportStatus::Ok
    }

    fn package_report_append(&mut self, mut package_report: PackageReport) {
        if matches!(package_report.status(), ReportStatus::Ok) {
            return;
        }
        if !package_report.errors.is_empty() {
            self.errors += 1;
        }
        if !package_report.warnings.is_empty() {
            self.warnings += 1;
        }
        self.package_issues
            .errors
            .append(&mut package_report.errors);
        self.package_issues
            .warnings
            .append(&mut package_report.warnings);
    }
    fn dir_report_append(&mut self, dir_path: impl AsRef<Path>, mut dir_report: DirReport) {
        if matches!(dir_report.status(), ReportStatus::Ok) {
            return;
        }
        if !dir_report.errors.is_empty() {
            self.errors += 1;
        }
        if !dir_report.warnings.is_empty() {
            self.warnings += 1;
        }
        self.dir_issues
            .entry(dir_path.as_ref().to_path_buf())
            .and_modify(|report| {
                report.errors.append(&mut dir_report.errors);
                report.warnings.append(&mut dir_report.warnings);
            })
            .or_insert(dir_report);
    }
    fn file_report_append(&mut self, file_path: impl AsRef<Path>, mut file_report: FileReport) {
        if matches!(file_report.status(), ReportStatus::Ok) {
            return;
        }
        if !file_report.errors.is_empty() {
            self.errors += 1;
        }
        if !file_report.warnings.is_empty() {
            self.warnings += 1;
        }
        self.file_issues
            .entry(file_path.as_ref().to_path_buf())
            .and_modify(|report| {
                report.errors.append(&mut file_report.errors);
                report.warnings.append(&mut file_report.warnings);
            })
            .or_insert(file_report);
    }
    pub fn print(&self, only_summary: bool) {
        if !only_summary {
            for error in self.package_issues.errors.iter() {
                println!("{}: {}", self.ident, error.red());
            }
            for warning in self.package_issues.warnings.iter() {
                println!("{}: {}", self.ident, warning.yellow());
            }

            for (dir_path, dir_report) in self.dir_issues.iter() {
                println!(
                    "{}: {} - {}",
                    self.ident,
                    dir_path.display().to_string().blue().bold(),
                    dir_report.status()
                );

                for error in dir_report.errors.iter() {
                    println!(
                        "{}: {} - {}",
                        self.ident,
                        dir_path.display().to_string().blue().bold(),
                        error.red()
                    );
                }
                for warning in dir_report.warnings.iter() {
                    println!(
                        "{}: {} - {}",
                        self.ident,
                        dir_path.display().to_string().blue().bold(),
                        warning.yellow()
                    );
                }
            }
            for (file_path, file_report) in self.file_issues.iter() {
                println!(
                    "{}: {} - {}",
                    self.ident,
                    file_path.display().to_string().white(),
                    file_report.status()
                );
                for error in file_report.errors.iter() {
                    println!(
                        "{}: {} - {}",
                        self.ident,
                        file_path.display().to_string().white(),
                        error.red()
                    );
                }
                for warning in file_report.warnings.iter() {
                    println!(
                        "{}: {} - {}",
                        self.ident,
                        file_path.display().to_string().white(),
                        warning.yellow()
                    );
                }
            }
        }

        let mut output = format!("{}: {}", self.ident, self.status(),);
        if self.errors != 0 {
            output = format!(
                "{} {}",
                output,
                format!("[{} errors]", self.errors).bright_red()
            );
        }
        if self.warnings != 0 {
            output = format!(
                "{} {}",
                output,
                format!("[{} warnings]", self.warnings).bright_yellow()
            );
        }
        println!("{}", output);
    }
}

pub enum ReportStatus {
    Error,
    Warning,
    Ok,
}

impl std::fmt::Display for ReportStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportStatus::Error => write!(f, "{}", "NOT OK".bright_red().bold()),
            ReportStatus::Warning => write!(f, "{}", "OK".bright_yellow().bold()),
            ReportStatus::Ok => write!(f, "{}", "OK".bright_green().bold()),
        }
    }
}

#[derive(Debug, Default)]
pub struct PackageReport {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl PackageReport {
    pub fn status(&self) -> ReportStatus {
        if !self.errors.is_empty() {
            return ReportStatus::Error;
        }
        if !self.warnings.is_empty() {
            return ReportStatus::Warning;
        }
        ReportStatus::Ok
    }
}

#[derive(Debug, Default)]
pub struct DirReport {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl DirReport {
    pub fn status(&self) -> ReportStatus {
        if !self.errors.is_empty() {
            return ReportStatus::Error;
        }
        if !self.warnings.is_empty() {
            return ReportStatus::Warning;
        }
        ReportStatus::Ok
    }
}

#[derive(Debug, Default)]
pub struct FileReport {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl FileReport {
    pub fn status(&self) -> ReportStatus {
        if !self.errors.is_empty() {
            return ReportStatus::Error;
        }
        if !self.warnings.is_empty() {
            return ReportStatus::Warning;
        }
        ReportStatus::Ok
    }
}

#[derive(Debug, Default)]
pub struct EmptyTopLevelDirCheck {
    dir_entry_count: Option<usize>,
}

impl EmptyTopLevelDirCheck {
    fn id(&self) -> &'static str {
        "EMPTY_TOP_LEVEL_DIR"
    }
    fn description(&self) -> &'static str {
        "Checks if an installed package directory is empty"
    }
    async fn visit_dir_start(
        &mut self,
        _abs_path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        self.dir_entry_count = Some(0);
        Ok(DirReport::default())
    }
    async fn visit_dir_end(
        &mut self,
        _path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        let file_count = self.dir_entry_count.take();
        if file_count.unwrap() == 0 && rel_path.as_ref().components().count() == 1 {
            Ok(DirReport {
                warnings: vec![format!(
                    "Top level directory is empty, considered removing it in your plan"
                )],
                errors: vec![],
            })
        } else {
            Ok(DirReport::default())
        }
    }
    async fn visit_child_dir(
        &mut self,
        _path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        self.dir_entry_count = self.dir_entry_count.map(|count| count + 1);
        Ok(DirReport::default())
    }
    async fn visit_file(
        &mut self,
        _path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        self.dir_entry_count = self.dir_entry_count.map(|count| count + 1);
        Ok(FileReport::default())
    }
    async fn visit_symlink(
        &mut self,
        _path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        self.dir_entry_count = self.dir_entry_count.map(|count| count + 1);
        Ok(FileReport::default())
    }
    async fn visit_package_end(&mut self) -> Result<PackageReport> {
        Ok(PackageReport::default())
    }
}

pub struct DependencyCheck<'a> {
    file_type_checker: Infer,
    package_metadata: &'a PackageMetadata,
    unused_deps: HashSet<PackageIdent>,
}

impl<'a> DependencyCheck<'a> {
    fn script_matcher(buf: &[u8]) -> bool {
        return buf.len() >= 2 && buf[0] == 0x23 && buf[1] == 0x21;
    }
    pub fn new(package_metadata: &'a PackageMetadata) -> DependencyCheck<'a> {
        let mut file_type_checker = infer::Infer::new();
        file_type_checker.add("script", "", DependencyCheck::script_matcher);
        DependencyCheck {
            file_type_checker,
            unused_deps: package_metadata.deps.clone(),
            package_metadata,
        }
    }
}

impl<'a> DependencyCheck<'a> {
    fn id(&self) -> &'static str {
        "DEPENDENCY_CHECK"
    }
    fn description(&self) -> &'static str {
        "Checks all dynamic linker dependencies, script interpreters and runtime dependencies"
    }
    async fn visit_dir_start(
        &mut self,
        _abs_path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        Ok(DirReport::default())
    }
    async fn visit_dir_end(
        &mut self,
        _path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        Ok(DirReport::default())
    }
    async fn visit_child_dir(
        &mut self,
        _path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<DirReport> {
        Ok(DirReport::default())
    }
    async fn visit_file(
        &mut self,
        path: impl AsRef<Path>,
        rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        match self.file_type_checker.get_from_path(path.as_ref()) {
            Ok(file_type) => {
                if let Some(file_type) = file_type {
                    let mime_type = file_type.mime_type();
                    if mime_type == "application/x-executable" {
                        debug!("Checking libraries for {}", rel_path.as_ref().display());
                        let buffer = tokio::fs::read(path.as_ref()).await?;
                        let object = Object::parse(&buffer)?;
                        match object {
                            Object::Elf(elf) => {
                                let mut report = FileReport::default();
                                let interpreter_search_path = if let Some(interpreter) =
                                    elf.interpreter
                                {
                                    let interpreter_path = PathBuf::from(interpreter);
                                    if let Ok(interpreter_path) =
                                        interpreter_path.strip_prefix(HAB_PKGS_PATH.as_path())
                                    {
                                        let _interpreter_package =
                                            self.package_metadata.all_runtime_deps().find(|dep| {
                                                if interpreter_path.starts_with(PathBuf::from(*dep))
                                                {
                                                    self.unused_deps.remove(dep);
                                                    true
                                                } else {
                                                    false
                                                }
                                            });
                                    } else {
                                        match self.package_metadata.pkg_type {
                                            PackageType::Standard => {
                                                report.errors.push(format!("Executable's ELF interpreter does not belong to a hab package: {}",interpreter))
                                            },
                                            PackageType::Native => {
                                                report.warnings.push(format!("Executable's ELF interpreter does not belong to a hab package: {}",interpreter))
                                            },
                                        }
                                    }
                                    interpreter_path.parent().map(|p| p.to_path_buf())
                                } else {
                                    None
                                };
                                let rpaths = elf
                                    .rpaths
                                    .iter()
                                    .flat_map(|v| v.split(':'))
                                    .map(|v| {
                                        if v.contains("$ORIGIN") {
                                            PathBuf::from(v.replace(
                                                "$ORIGIN",
                                                path.as_ref().parent().unwrap().to_str().unwrap(),
                                            ))
                                        } else {
                                            PathBuf::from(v)
                                        }
                                    })
                                    .collect::<Vec<_>>();
                                let runpaths = elf
                                    .runpaths
                                    .iter()
                                    .flat_map(|v| v.split(':'))
                                    .map(|v| {
                                        if v.contains("$ORIGIN") {
                                            PathBuf::from(v.replace(
                                                "$ORIGIN",
                                                path.as_ref().parent().unwrap().to_str().unwrap(),
                                            ))
                                        } else {
                                            PathBuf::from(v)
                                        }
                                    })
                                    .collect::<Vec<_>>();
                                for rpath in rpaths.iter() {
                                    if !rpath.starts_with(HAB_PKGS_PATH.as_path()) {
                                        match self.package_metadata.pkg_type {
                                            PackageType::Standard => {
                                                report.errors.push(format!(
                                            "RPATH directory '{}' does not belong to a hab package",
                                            rpath.display()
                                        ));
                                            }
                                            PackageType::Native => {
                                                report.warnings.push(format!(
                                            "RPATH directory '{}' does not belong to a hab package",
                                            rpath.display()
                                        ));
                                            }
                                        }
                                    }
                                }
                                for runpath in runpaths.iter() {
                                    if !runpath.starts_with(HAB_PKGS_PATH.as_path()) {
                                        match self.package_metadata.pkg_type {
                                            PackageType::Standard => {
                                                report.errors.push(format!(
                                            "RUNPATH directory '{}' does not belong to a hab package",
                                            runpath.display()
                                        ));
                                            }
                                            PackageType::Native => {
                                                report.warnings.push(format!(
                                            "RUNPATH directory '{}' does not belong to a hab package",
                                            runpath.display()
                                        ));
                                            }
                                        }
                                    }
                                }
                                for library in elf.libraries.iter() {
                                    let mut found = false;
                                    for search_path in rpaths
                                        .iter()
                                        .chain(runpaths.iter())
                                        .chain(interpreter_search_path.iter())
                                    {
                                        let library_path = search_path.join(library);
                                        if library_path.is_file() {
                                            found = true;
                                            trace!(
                                                "For {} library {} found in {}",
                                                rel_path.as_ref().display(),
                                                library,
                                                library_path.display()
                                            );
                                            self.package_metadata.all_runtime_deps().find(|dep| {
                                                if library_path
                                                    .strip_prefix(HAB_PKGS_PATH.as_path())
                                                    .ok()
                                                    .map(|p| p.starts_with(PathBuf::from(*dep)))
                                                    .unwrap_or_default()
                                                {
                                                    self.unused_deps.remove(dep);
                                                    true
                                                } else {
                                                    false
                                                }
                                            });
                                            break;
                                        }
                                    }
                                    if !found {
                                        match self.package_metadata.pkg_type {
                                            PackageType::Standard => {
                                                report.errors.push(format!(
                                            "Library {} not found in any RPATH or RUNPATH directory: {:?}",
                                            library,
                                            rpaths
                                                .iter()
                                                .chain(runpaths.iter())
                                                .chain(interpreter_search_path.iter())
                                                .collect::<Vec<_>>(),
                                        ));
                                            }
                                            PackageType::Native => {
                                                report.warnings.push(format!(
                                            "Library {} not found in any RPATH or RUNPATH directory: {:?}",
                                            library,
                                            rpaths
                                                .iter()
                                                .chain(runpaths.iter())
                                                .chain(interpreter_search_path.iter())
                                                .collect::<Vec<_>>(),
                                        ));
                                            }
                                        }
                                    }
                                }
                                Ok(report)
                            }
                            _ => Ok(FileReport::default()),
                        }
                    } else if mime_type == "script" {
                        let mut interpreter = String::new();
                        let file = File::open(path.as_ref()).await?;
                        let mut reader = BufReader::new(file);
                        match reader.read_line(&mut interpreter).await {
                            Ok(_) => {
                                let interpreter = interpreter.strip_prefix("#!").unwrap();
                                if let Some(interpreter) = interpreter.split_whitespace().next() {
                                    let interpreter = PathBuf::from(interpreter);
                                    if PLATFORM_SHELLS.contains(&interpreter) {
                                        Ok(FileReport::default())
                                    } else if let Ok(interpreter_path) =
                                        interpreter.strip_prefix(HAB_PKGS_PATH.as_path())
                                    {
                                        let interpreter_package =
                                            self.package_metadata.all_runtime_deps().find(|dep| {
                                                if interpreter_path.starts_with(PathBuf::from(*dep))
                                                {
                                                    self.unused_deps.remove(dep);
                                                    true
                                                } else {
                                                    false
                                                }
                                            });

                                        if let Some(_) = interpreter_package {
                                            // Check that interpreter exists
                                            if interpreter.is_file() {
                                                Ok(FileReport::default())
                                            } else {
                                                Ok(FileReport {
                                                    warnings: vec![],
                                                    errors: vec![format!(
                                                        "Script interpreter does not exist: {}",
                                                        interpreter.display()
                                                    )],
                                                })
                                            }
                                        } else {
                                            Ok(FileReport {
                                        warnings: vec![],
                                        errors: vec![format!(
                                            "Script interpreter's package is not a runtime dependency: {}",
                                            interpreter_path.components().take(4).collect::<PathBuf>().display()
                                        )],
                                    })
                                        }
                                    } else if interpreter.is_relative() {
                                        Ok(FileReport {
                                            warnings: vec![format!(
                                                "Script uses relative interpreter: {}",
                                                interpreter.display()
                                            )],
                                            errors: vec![],
                                        })
                                    } else {
                                        match self.package_metadata.pkg_type {
                                            PackageType::Standard => Ok(FileReport {
                                                warnings: vec![],
                                                errors: vec![format!(
                                                    "Script uses interpreter on host system: {}",
                                                    interpreter.display()
                                                )],
                                            }),
                                            PackageType::Native => Ok(FileReport {
                                                warnings: vec![format!(
                                                    "Script uses interpreter on host system: {}",
                                                    interpreter.display()
                                                )],
                                                errors: vec![],
                                            }),
                                        }
                                    }
                                } else {
                                    Ok(FileReport {
                                        warnings: vec![],
                                        errors: vec![format!(
                                            "Script interpreter not specified after shebang '#!'",
                                        )],
                                    })
                                }
                            }
                            Err(err) => Ok(FileReport {
                                warnings: vec![format!(
                                    "File starts with '#!' but has no interpreter: {}",
                                    err
                                )],
                                errors: vec![],
                            }),
                        }
                    } else {
                        trace!(
                            "File {} has type {}",
                            rel_path.as_ref().display(),
                            file_type.mime_type()
                        );
                        Ok(FileReport::default())
                    }
                } else {
                    trace!(
                        "Failed to determine file type of {}",
                        rel_path.as_ref().display(),
                    );
                    Ok(FileReport::default())
                }
            }
            Err(err) => {
                if err.kind() == ErrorKind::PermissionDenied {
                    Ok(FileReport {
                        errors: vec![],
                        warnings: vec![format!("File could not be verified due to insufficient permissions, try re-running check as root")]
                    })
                } else {
                    Err(err.into())
                }
            }
        }
    }
    async fn visit_symlink(
        &mut self,
        path: impl AsRef<Path>,
        _rel_path: impl AsRef<Path>,
    ) -> Result<FileReport> {
        if !path.as_ref().exists() {
            Ok(FileReport {
                warnings: vec![],
                errors: vec![format!("Broken symlink, points to non-existent file",)],
            })
        } else {
            Ok(FileReport::default())
        }
    }
    async fn visit_package_end(&mut self) -> Result<PackageReport> {
        if !self.unused_deps.is_empty() {
            Ok(PackageReport {
                errors: vec![],
                warnings: vec![format!(
                    "Package does not seem to use the following runtime deps: {}",
                    self.unused_deps
                        .iter()
                        .map(|d| d.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )],
            })
        } else {
            Ok(PackageReport::default())
        }
    }
}
