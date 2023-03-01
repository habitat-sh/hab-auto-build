use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use bzip2::read::BzDecoder;
use color_eyre::eyre::Result;
use flate2::bufread::GzDecoder;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use tar::Archive;
use tracing::error;
use xz2::bufread::XzDecoder;

use super::FileKind;

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

lazy_static! {
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
        builder.build().unwrap()
    };
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SourceContext {
    pub format: (FileKind, Option<FileKind>),
    pub licenses: Vec<SourceLicenseContext>,
}

impl SourceContext {
    pub fn read_from_disk(path: impl AsRef<Path>) -> Result<SourceContext> {
        let file_type = FileKind::detect_from_path(path.as_ref())?;
        let file = BufReader::new(File::open(path.as_ref())?);
        let format;
        let mut licenses = Vec::new();
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
                    format = (file_type, None);
                }
            }
            FileKind::Gzip => {
                if let FileKind::Tar = FileKind::detect_from_reader(GzDecoder::new(file)) {
                    let decoder = GzDecoder::new(BufReader::new(File::open(path.as_ref())?));
                    format = (file_type, Some(FileKind::Tar));
                    licenses = SourceContext::read_licenses_from_archive(Archive::new(decoder))?;
                } else {
                    format = (file_type, None);
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
                }
            }
            FileKind::Elf | FileKind::Script | FileKind::Other => {
                format = (file_type, None);
                licenses = vec![];
            }
        }

        Ok(SourceContext { format, licenses })
    }

    pub fn read_licenses_from_archive<R>(mut tar: Archive<R>) -> Result<Vec<SourceLicenseContext>>
    where
        R: Read,
    {
        let mut licenses = Vec::new();
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if LICENSE_GLOBSET.is_match(path.as_path()) {
                let mut text = String::new();
                if entry.read_to_string(&mut text).is_ok() {
                    licenses.push(SourceLicenseContext { path, text })
                } else {
                    error!(target: "user-log", "Failed to read file {} in archive", path.display());
                }
            }
        }
        Ok(licenses)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SourceLicenseContext {
    pub path: PathBuf,
    pub text: String,
}
