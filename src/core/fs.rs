use color_eyre::eyre::Result;
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

impl From<PathBuf> for FSRootPath {
    fn from(value: PathBuf) -> Self {
        FSRootPath(value)
    }
}

impl AsRef<Path> for FSRootPath {
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
