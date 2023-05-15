use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use askalono::{ScanMode, ScanStrategy, Store, TextData};
use bzip2::read::BzDecoder;
use color_eyre::eyre::{Context, Result};
use flate2::bufread::GzDecoder;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use tar::Archive;
use tracing::{error, trace};
use xz2::bufread::XzDecoder;

use super::{Blake3, FileKind, PackageSha256Sum};

const LICENSE_GLOBS: &[&str] = &[
    // General
    "COPYING",
    "COPYING[.-]*",
    "COPYING[0-9]",
    "COPYING[0-9][.-]*",
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
    // Other license-specific (APACHE-2.0.txt, etc.)
    "agpl[.-]*",
    "gpl[.-]*",
    "lgpl[.-]*",
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
const LICENSE_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/license-cache.bin.gz"));

lazy_static! {
    static ref LICENSE_STORE: Store = Store::from_cache(LICENSE_DATA).unwrap();
    static ref LICENSE_GLOBSET: GlobSet = {
        let mut builder = GlobSetBuilder::new();
        // A GlobBuilder can be used to configure each glob's match semantics
        // independently.
        for glob in LICENSE_GLOBS {
            builder.add(
                GlobBuilder::new(format!("**/{}", glob).as_str())
                    .literal_separator(true)
                    .build().unwrap()
            );
        }
        for license in LICENSE_STORE.licenses() {
            builder.add(
                GlobBuilder::new(format!("**/{}", license).as_str())
                    .literal_separator(true)
                    .build().unwrap()
            );
            builder.add(
                GlobBuilder::new(format!("**/{}.txt", license).as_str())
                    .literal_separator(true)
                    .build().unwrap()
            );
        }
        builder.build().unwrap()
    };
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SourceContext {
    pub format: (FileKind, Option<FileKind>),
    pub licenses: BTreeSet<SourceLicenseContext>,
    pub source_shasum: Option<PackageSha256Sum>,
}

impl SourceContext {
    pub fn read_from_disk(
        path: impl AsRef<Path>,
        source_download_shasum: Option<PackageSha256Sum>,
    ) -> Result<SourceContext> {
        let file_type = FileKind::detect_from_path(path.as_ref())?;
        let file = BufReader::new(File::open(path.as_ref())?);
        let format;
        let mut licenses = BTreeSet::new();
        match file_type {
            FileKind::Tar => {
                format = (file_type, None);
                licenses = SourceContext::read_licenses_from_archive(Archive::new(file))?;
            }
            FileKind::Bzip2 => {
                if let FileKind::Tar = FileKind::detect_from_reader(BzDecoder::new(file)) {
                    let decoder = BzDecoder::new(BufReader::new(File::open(path.as_ref())?));
                    format = (file_type, Some(FileKind::Tar));
                    licenses = SourceContext::read_licenses_from_archive(Archive::new(decoder))?;
                } else {
                    // We just assume the inner file is a tar
                    let decoder = BzDecoder::new(BufReader::new(File::open(path.as_ref())?));
                    format = (file_type, Some(FileKind::Tar));
                    licenses = SourceContext::read_licenses_from_archive(Archive::new(decoder))?;
                }
            }
            FileKind::Gzip => {
                if let FileKind::Tar = FileKind::detect_from_reader(GzDecoder::new(file)) {
                    let decoder = GzDecoder::new(BufReader::new(File::open(path.as_ref())?));
                    format = (file_type, Some(FileKind::Tar));
                    licenses = SourceContext::read_licenses_from_archive(Archive::new(decoder))?;
                } else {
                    format = (file_type, None);
                    let decoder = GzDecoder::new(BufReader::new(File::open(path.as_ref())?));
                    licenses = SourceContext::read_licenses_from_archive(Archive::new(decoder))?;
                }
            }
            FileKind::Lzip => {
                todo!()
            }
            FileKind::Xz => match FileKind::detect_from_reader(XzDecoder::new(file)) {
                FileKind::Tar | FileKind::Other => {
                    let decoder = XzDecoder::new(BufReader::new(File::open(path.as_ref())?));
                    format = (file_type, Some(FileKind::Tar));
                    licenses = SourceContext::read_licenses_from_archive(Archive::new(decoder))?;
                }
                _ => {
                    format = (file_type, None);
                    todo!()
                }
            },
            FileKind::Compress => todo!(),
            FileKind::Zstd => {
                if let FileKind::Tar =
                    FileKind::detect_from_reader(zstd::stream::read::Decoder::new(file)?)
                {
                    let decoder = zstd::stream::read::Decoder::new(BufReader::new(File::open(
                        path.as_ref(),
                    )?))?;
                    format = (file_type, Some(FileKind::Tar));
                    licenses = SourceContext::read_licenses_from_archive(Archive::new(decoder))?;
                } else {
                    format = (file_type, None);
                    todo!()
                }
            }
            FileKind::Elf | FileKind::Script | FileKind::Other => {
                format = (file_type, None);
                licenses = BTreeSet::default();
            }
        }

        Ok(SourceContext {
            format,
            licenses,
            source_shasum: source_download_shasum,
        })
    }

    pub fn read_licenses_from_archive<R>(
        mut tar: Archive<R>,
    ) -> Result<BTreeSet<SourceLicenseContext>>
    where
        R: Read,
    {
        let mut licenses = BTreeSet::new();
        let strategy = ScanStrategy::new(&*LICENSE_STORE)
            .confidence_threshold(0.8)
            .mode(ScanMode::TopDown)
            .shallow_limit(0.98)
            .max_passes(50)
            .optimize(true);
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if LICENSE_GLOBSET.is_match(path.as_path()) {
                trace!("Scanning file {} for licenses", path.display());
                let mut text = String::new();
                if entry.read_to_string(&mut text).is_ok() {
                    let data = TextData::from(text.clone());
                    let mut detected_licenses = BTreeSet::new();
                    if let Ok(results) = strategy.scan(&data) {
                        for item in results.containing {
                            detected_licenses.insert(item.license.name.to_string());
                        }
                    }
                    trace!(
                        "Including license file {} in source context, with detected licenses: {:?}",
                        path.display(),
                        detected_licenses
                    );
                    licenses.insert(SourceLicenseContext {
                        path,
                        text,
                        detected_licenses,
                    });
                } else {
                    error!(target: "user-log", "Failed to read file {} in archive", path.display());
                }
            }
        }
        Ok(licenses)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SourceLicenseContext {
    pub path: PathBuf,
    pub text: String,
    pub detected_licenses: BTreeSet<String>,
}
