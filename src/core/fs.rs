use chrono::{DateTime, NaiveDateTime, Utc};
use color_eyre::eyre::{eyre, Context, Result};
use globset::{Glob, GlobSetBuilder};
use infer::Infer;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

lazy_static! {
    static ref FILE_KIND_CHECKER: Infer = {
        let mut checker = Infer::new();
        checker.add("script", "", file_types::script_matcher);
        checker
    };
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FileKind {
    Tar,
    Bzip2,
    Gzip,
    Lzip,
    Xz,
    Compress,
    Zstd,
    Elf,
    Script,
    Other,
}

impl From<&str> for FileKind {
    fn from(value: &str) -> Self {
        match value {
            "application/x-tar" => FileKind::Tar,
            "application/x-bzip2" => FileKind::Bzip2,
            "application/gzip" => FileKind::Gzip,
            "application/x-lzip" => FileKind::Lzip,
            "application/x-compress" => FileKind::Compress,
            "application/x-xz" => FileKind::Xz,
            "application/zstd" => FileKind::Zstd,
            "application/x-executable" => FileKind::Elf,
            "script" => FileKind::Script,
            _ => FileKind::Other,
        }
    }
}

impl FileKind {
    pub fn detect(buf: &[u8]) -> FileKind {
        FileKind::from(
            FILE_KIND_CHECKER
                .get(buf)
                .map(|t| t.mime_type())
                .unwrap_or("unknown"),
        )
    }
    pub fn detect_from_reader<'a>(mut reader: impl Read) -> FileKind {
        let mut buffer = [0u8; 1024];
        let mut data = Vec::new();
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                break;
            }
            if data.len() > 512 {
                break;
            }
            data.extend_from_slice(&buffer[..n]);
        }
        FileKind::detect(&data)
    }

    pub fn detect_from_path(path: impl AsRef<Path>) -> Result<FileKind> {
        let file = File::open(path.as_ref())?;
        Ok(FileKind::detect_from_reader(file))
    }

    pub fn maybe_read_file(
        mut reader: impl Read,
        accepted_file_kinds: &[FileKind],
    ) -> Option<(FileKind, Vec<u8>)> {
        let mut buffer = [0u8; 1024];
        let mut data = Vec::new();
        let mut file_type = None;
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                if file_type.is_none() {
                    file_type = Some(FileKind::detect(&data));
                }
                break;
            }
            if file_type.is_none() && data.len() > 512 {
                let detected_file_type = FileKind::detect(&data);
                if !accepted_file_kinds.contains(&detected_file_type) {
                    return None;
                }
                file_type = Some(detected_file_type);
            }
            data.extend_from_slice(&buffer[..n]);
        }
        if let Some(file_type) = file_type {
            if accepted_file_kinds.contains(&file_type) {
                Some((file_type, data))
            } else {
                None
            }
        } else {
            None
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct FSRootPath(PathBuf);

impl Default for FSRootPath {
    fn default() -> Self {
        Self(PathBuf::from("/"))
    }
}

impl From<HabitatStudioRootPath> for FSRootPath {
    fn from(value: HabitatStudioRootPath) -> Self {
        FSRootPath(value.0)
    }
}

impl AsRef<Path> for FSRootPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct HabitatSourceCachePath(PathBuf);

impl AsRef<Path> for HabitatSourceCachePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct HabitatStudioRootPath(PathBuf);

impl AsRef<Path> for HabitatStudioRootPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct HabitatRootPath(PathBuf);

impl Default for HabitatRootPath {
    fn default() -> Self {
        HabitatRootPath::new(FSRootPath::default())
    }
}

impl HabitatRootPath {
    pub fn new(fs_root_path: FSRootPath) -> HabitatRootPath {
        HabitatRootPath(fs_root_path.as_ref().join("hab"))
    }
    pub fn studio_root(&self, studio_name: &str) -> HabitatStudioRootPath {
        HabitatStudioRootPath(self.0.join("studios").join(studio_name))
    }
    pub fn source_cache(&self) -> HabitatSourceCachePath {
        HabitatSourceCachePath(self.0.join("cache").join("src"))
    }
}

impl AsRef<Path> for HabitatRootPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

pub mod file_types {
    pub fn script_matcher(buf: &[u8]) -> bool {
        return buf.len() >= 2 && buf[0] == 0x23 && buf[1] == 0x21;
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct GlobSetExpression {
    pub patterns: Vec<String>,
    globset: globset::GlobSet,
}

impl Default for GlobSetExpression {
    fn default() -> Self {
        Self {
            patterns: Default::default(),
            globset: Default::default(),
        }
    }
}

impl GlobSetExpression {
    pub fn is_match(&self, path: impl AsRef<Path>) -> bool {
        self.globset.is_match(path)
    }
}

impl TryFrom<Vec<String>> for GlobSetExpression {
    type Error = color_eyre::eyre::Error;

    fn try_from(patterns: Vec<String>) -> Result<Self, Self::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns.iter() {
            builder.add(Glob::new(pattern).with_context(|| {
                format!("Invalid glob pattern '{}' in 'ignored_packages'", pattern)
            })?);
        }
        let globset = builder.build()?;
        Ok(GlobSetExpression { patterns, globset })
    }
}

impl Into<Vec<String>> for GlobSetExpression {
    fn into(self) -> Vec<String> {
        self.patterns
    }
}

pub trait Metadata {
    fn last_modifed_at(&self) -> Result<DateTime<Utc>>;
    fn set_last_modifed_at(&self, modified_at: DateTime<Utc>) -> Result<()>;
}

impl<T> Metadata for T
where
    T: AsRef<Path>,
{
    /// Cross platform method to fetch last modified time for a path
    fn last_modifed_at(&self) -> Result<DateTime<Utc>> {
        let modified_at =
            filetime::FileTime::from_last_modification_time(&self.as_ref().metadata()?);
        Ok(DateTime::<Utc>::from_utc(
            NaiveDateTime::from_timestamp_opt(
                modified_at.unix_seconds(),
                modified_at.nanoseconds(),
            )
            .ok_or(eyre!("Last modification timestamp out of range"))?,
            Utc,
        ))
    }

    /// Cross platform method to set last modified time for a path
    fn set_last_modifed_at(&self, modified_at: DateTime<Utc>) -> Result<()> {
        filetime::set_file_mtime(
            self.as_ref(),
            filetime::FileTime::from_unix_time(
                modified_at.timestamp(),
                modified_at.timestamp_subsec_nanos(),
            ),
        )?;
        Ok(())
    }
}
