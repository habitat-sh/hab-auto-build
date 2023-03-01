use chrono::{DateTime, Utc};
use color_eyre::eyre::{eyre, Result};
use diesel::Connection;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use goblin::{
    elf64::{
        dynamic::DF_1_PIE,
        header::{ET_DYN, ET_EXEC},
    },
    Object,
};
use ignore::{ParallelVisitor, ParallelVisitorBuilder, WalkBuilder, WalkState};
use lazy_static::lazy_static;
use path_absolutize::Absolutize;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsStr,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::mpsc::{channel, Sender},
    time::Instant,
};
use tar::Archive;
use tracing::{debug, error, info};
use xz2::bufread::XzDecoder;

use crate::store::{self, Store};

use super::{
    Blake3, FSRootPath, FileKind, HabitatRootPath, PackageDepIdent, PackageIdent, PackageName,
    PackageOrigin, PackagePath, PackageRelease, PackageResolvedDepIdent, PackageResolvedRelease,
    PackageResolvedVersion, PackageSha256Sum, PackageSource, PackageSourceURL, PackageTarget,
    PackageType, PackageVersion, PlanContextID,
};

lazy_static! {
    static ref METADATA_GLOBSET: GlobSet = {
        let mut globset_builder = GlobSetBuilder::new();
        for pattern in [
            "hab/pkgs/*/*/*/*/MANIFEST",
            "hab/pkgs/*/*/*/*/RUNTIME_PATH",
            "hab/pkgs/*/*/*/*/DEPS",
            "hab/pkgs/*/*/*/*/TDEPS",
            "hab/pkgs/*/*/*/*/BUILD_DEPS",
            "hab/pkgs/*/*/*/*/PACKAGE_TYPE",
            "hab/pkgs/*/*/*/*/IDENT",
            "hab/pkgs/*/*/*/*/INTERPRETERS",
            "hab/pkgs/*/*/*/*/PKG_CONFIG_PATH",
        ] {
            globset_builder.add(
                GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .build()
                    .unwrap(),
            );
        }
        globset_builder.build().unwrap()
    };
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct ArtifactCachePath(PathBuf);

impl ArtifactCachePath {
    pub fn new(hab_root: HabitatRootPath) -> ArtifactCachePath {
        ArtifactCachePath(hab_root.as_ref().join("cache").join("artifacts"))
    }
}

impl AsRef<Path> for ArtifactCachePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl Default for ArtifactCachePath {
    fn default() -> Self {
        ArtifactCachePath::new(HabitatRootPath::default())
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub(crate) struct ArtifactPath(PathBuf);

impl AsRef<Path> for ArtifactPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

type ArtifactList = HashMap<
    PackageOrigin,
    HashMap<
        PackageName,
        HashMap<
            PackageTarget,
            BTreeMap<PackageResolvedVersion, BTreeMap<PackageResolvedRelease, ArtifactContext>>,
        >,
    >,
>;

pub(crate) struct ArtifactCache {
    known_artifacts: ArtifactList,
}

impl ArtifactCache {
    pub fn new(artifact_cache_path: ArtifactCachePath, store: &Store) -> Result<ArtifactCache> {
        let start = Instant::now();
        let mut known_artifacts = ArtifactList::default();
        let artifact_cache_walker = WalkBuilder::new(artifact_cache_path.as_ref()).build_parallel();
        std::thread::scope(|scope| {
            let (sender, receiver) = channel();
            let mut artifact_indexer_builder = ArtifactIndexerBuilder::new(store, sender);
            let artifact_indexer_thread =
                scope.spawn(move || artifact_cache_walker.visit(&mut artifact_indexer_builder));
            let mut known_artifact_count = 0;
            let mut new_artifact_count = 0;
            while let Ok(artifact_ctx) = receiver.recv() {
                if artifact_ctx.is_dirty {
                    new_artifact_count += 1;
                    store
                        .get_connection()?
                        .immediate_transaction(|connection| {
                            store::artifact_context_put(
                                connection,
                                &artifact_ctx.hash,
                                &artifact_ctx,
                            )
                        })?;
                }
                known_artifact_count += 1;

                known_artifacts
                    .entry(artifact_ctx.id.origin.clone())
                    .or_default()
                    .entry(artifact_ctx.id.name.clone())
                    .or_default()
                    .entry(artifact_ctx.id.target)
                    .or_default()
                    .entry(artifact_ctx.id.version.clone())
                    .or_default()
                    .entry(artifact_ctx.id.release.clone())
                    .or_insert(artifact_ctx);
            }
            artifact_indexer_thread
                .join()
                .expect("Failed to join artifact indexer thread to parent thread");
            info!(
                "Detected {} artifacts ({} new artifacts) in {}s",
                known_artifact_count,
                new_artifact_count,
                start.elapsed().as_secs_f32()
            );
            Ok(ArtifactCache { known_artifacts })
        })
    }

    pub fn latest_plan_artifact(&self, build_ident: &PlanContextID) -> Option<&ArtifactContext> {
        let build_ident = build_ident.as_ref();
        self.known_artifacts
            .get(&build_ident.origin)
            .and_then(|a| a.get(&build_ident.name))
            .and_then(|a| a.get(&build_ident.target))
            .and_then(|a| match &build_ident.version {
                version @ PackageResolvedVersion::Static(_) => a.get(version),
                PackageResolvedVersion::Dynamic => a.values().rev().next(),
            })
            .and_then(|a| a.values().rev().next())
    }

    pub fn latest_artifact(&self, dep_ident: &PackageResolvedDepIdent) -> Option<&ArtifactContext> {
        self.known_artifacts
            .get(&dep_ident.origin)
            .and_then(|a| a.get(&dep_ident.name))
            .and_then(|a| a.get(&dep_ident.target))
            .and_then(|a| match &dep_ident.version {
                PackageVersion::Resolved(version @ PackageResolvedVersion::Static(_)) => {
                    a.get(version)
                }
                PackageVersion::Unresolved
                | PackageVersion::Resolved(PackageResolvedVersion::Dynamic) => {
                    a.values().rev().next()
                }
            })
            .and_then(|a| match &dep_ident.release {
                PackageRelease::Resolved(release) => a.get(release),
                PackageRelease::Unresolved => a.values().rev().next(),
            })
    }

    pub fn artifact(&self, dep_ident: &PackageIdent) -> Option<&ArtifactContext> {
        self.known_artifacts
            .get(&dep_ident.origin)
            .and_then(|a| a.get(&dep_ident.name))
            .and_then(|a| a.get(&dep_ident.target))
            .and_then(|a| a.get(&dep_ident.version))
            .and_then(|a| a.get(&dep_ident.release))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ElfMetadata {
    pub required_libraries: Vec<String>,
    pub rpath: Vec<PathBuf>,
    pub runpath: Vec<PathBuf>,
    pub interpreter: Option<PathBuf>,
    pub elf_type: ElfType,
    pub is_executable: bool,
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub(crate) enum ElfType {
    #[serde(rename = "executable")]
    Executable,
    #[serde(rename = "shared-library")]
    SharedLibrary,
    #[serde(rename = "pie-executable")]
    PieExecutable,
    #[serde(rename = "relocatable")]
    Relocatable,
    #[serde(rename = "other")]
    Other,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ScriptMetadata {
    pub interpreter: ScriptInterpreterMetadata,
    pub is_executable: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ScriptInterpreterMetadata {
    pub raw: String,
    pub command: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ArtifactContext {
    pub id: PackageIdent,
    pub hash: Blake3,
    #[serde(default, skip)]
    pub is_dirty: bool,
    pub target: PackageTarget,
    pub package_type: PackageType,
    pub deps: HashSet<PackageIdent>,
    pub tdeps: HashSet<PackageIdent>,
    pub build_deps: HashSet<PackageIdent>,
    pub runtime_path: Vec<PathBuf>,
    pub interpreters: Vec<PathBuf>,
    pub source: Option<PackageSource>,
    pub licenses: Vec<String>,
    pub elfs: HashMap<PathBuf, ElfMetadata>,
    pub empty_top_level_dirs: HashSet<PathBuf>,
    pub links: BTreeMap<PathBuf, PathBuf>,
    pub broken_links: HashMap<PathBuf, PathBuf>,
    pub empty_links: HashSet<PathBuf>,
    pub scripts: HashMap<PathBuf, ScriptMetadata>,
    pub created_at: DateTime<Utc>,
}

impl ArtifactContext {
    /// Search for an executable with the given name.
    /// This function only returns a result if the found executable
    /// has the executable permission set.
    pub fn search_runtime_executable(
        &self,
        executable_name: impl AsRef<Path>,
    ) -> Option<ExecutableMetadata<'_>> {
        for path in self.runtime_path.iter() {
            let executable_path = path.join(executable_name.as_ref());

            if let Some(metadata) = self.elfs.get(&executable_path) {
                if metadata.is_executable {
                    return Some(ExecutableMetadata::Elf(metadata));
                }
            }
            if let Some(metadata) = self.scripts.get(&executable_path) {
                if metadata.is_executable {
                    return Some(ExecutableMetadata::Script(metadata));
                }
            }
        }
        None
    }

    /// Search for an executable with the given name.
    /// This function returns a result regardless of it's
    /// executable permission bit.
    pub fn search_runtime(
        &self,
        executable_name: impl AsRef<Path>,
    ) -> Option<ExecutableMetadata<'_>> {
        for path in self.runtime_path.iter() {
            let executable_path = path.join(executable_name.as_ref());
            if let Some(metadata) = self.elfs.get(&executable_path) {
                return Some(ExecutableMetadata::Elf(metadata));
            }
            if let Some(metadata) = self.scripts.get(&executable_path) {
                return Some(ExecutableMetadata::Script(metadata));
            }
        }
        None
    }

    pub fn resolve_path(
        &self,
        tdeps: &HashMap<PackageIdent, ArtifactContext>,
        path: impl AsRef<Path>,
    ) -> PathBuf {
        let mut current_artifact = Some(self);
        let mut resolved_path = path.as_ref().to_path_buf();
        while let Some(artifact_ctx) = current_artifact {
            if let Some(link) = artifact_ctx.links.get(resolved_path.as_path()) {
                let link = if link.is_absolute() {
                    link.to_path_buf()
                } else {
                    resolved_path
                        .parent()
                        .unwrap()
                        .join(link)
                        .absolutize()
                        .unwrap()
                        .to_path_buf()
                };
                if let Some(next_artifact_ctx) = link.package_ident(artifact_ctx.target) {
                    if next_artifact_ctx == artifact_ctx.id
                        && artifact_ctx.links.get(&link).is_none()
                    {
                        resolved_path = link.to_path_buf();
                        current_artifact = None;
                    } else {
                        current_artifact = tdeps.get(&next_artifact_ctx);
                        resolved_path = link.to_path_buf();
                    }
                }
            } else {
                let mut current_parent = resolved_path.parent();
                let mut is_in_symlinked_dir = false;
                while let Some(parent) = current_parent {
                    if let Some(parent_link) = artifact_ctx.links.get(parent) {
                        let parent_link = if parent_link.is_absolute() {
                            parent_link.to_path_buf()
                        } else {
                            parent
                                .parent()
                                .unwrap()
                                .join(parent_link)
                                .absolutize()
                                .unwrap()
                                .to_path_buf()
                        };
                        resolved_path = parent_link
                            .join(resolved_path.strip_prefix(parent).unwrap());
                        is_in_symlinked_dir = true;
                        break;
                    } else {
                        current_parent = parent.parent()
                    }
                }
                if is_in_symlinked_dir {
                    if let Some(next_artifact_ctx) =
                        resolved_path.package_ident(artifact_ctx.target)
                    {
                        current_artifact = tdeps.get(&next_artifact_ctx);
                    } else {
                        current_artifact = None;
                    }
                } else {
                    current_artifact = None;
                }
            }
        }
        resolved_path
    }
}

pub(crate) enum ExecutableMetadata<'a> {
    Elf(&'a ElfMetadata),
    Script(&'a ScriptMetadata),
}

pub(crate) struct ArtifactIndexer<'a> {
    store: &'a Store,
    sender: Sender<ArtifactContext>,
}

impl<'a> ParallelVisitor for ArtifactIndexer<'a> {
    fn visit(
        &mut self,
        entry: std::result::Result<ignore::DirEntry, ignore::Error>,
    ) -> ignore::WalkState {
        if let Ok(entry) = entry {
            if let Some("hart") = entry.path().extension().and_then(OsStr::to_str) {
                let hash = Blake3::from_path(entry.path()).unwrap_or_else(|_| {
                    panic!(
                        "Failed to generate hash for artifact {}",
                        entry.path().display()
                    )
                });
                if let Some(artifact_ctx) = self
                    .store
                    .get_connection()
                    .expect("Failed to open connection to hab-auto-build sqlite database")
                    .transaction(|connection| store::artifact_context_get(connection, &hash))
                    .expect("Failed to read artifact context from hab-auto-build sqlite database")
                {
                    debug!("Artifact {} loaded from cache", artifact_ctx.id);
                    self.sender
                        .send(artifact_ctx)
                        .expect("Failed to send artifact context to parent thread");
                } else {
                    match ArtifactIndexer::read_from_disk(entry.path(), &hash) {
                        Ok(artifact_ctx) => {
                            debug!("Artifact {} loaded from disk", artifact_ctx.id);
                            self.sender
                                .send(artifact_ctx)
                                .expect("Failed to send artifact context to parent thread");
                        }
                        Err(err) => {
                            error!(
                                "Failed to read contents of package artifact '{}': {}",
                                entry.path().display(),
                                err
                            );
                        }
                    }
                }
            } else {
                return WalkState::Continue;
            }
        }
        WalkState::Continue
    }
}

impl<'a> ArtifactIndexer<'a> {
    fn read_from_disk(artifact_path: impl AsRef<Path>, hash: &Blake3) -> Result<ArtifactContext> {
        let f = std::fs::File::open(artifact_path.as_ref())?;
        let created_at = f.metadata()?.modified()?;
        let mut reader = std::io::BufReader::new(f);

        // We skip the first 5 lines
        let mut line = String::new();
        let mut skip_lines = 5;
        loop {
            match reader.read_line(&mut line) {
                Ok(0) => {
                    return Err(eyre!(
                        "The file '{}' is not a valid .hart file",
                        artifact_path.as_ref().display()
                    ));
                }
                Ok(_) => {
                    skip_lines -= 1;
                    if skip_lines == 0 {
                        break;
                    } else {
                        continue;
                    }
                }
                Err(err) => {
                    return Err(eyre!(
                        "The file '{}' is not a valid .hart file: {:?}",
                        artifact_path.as_ref().display(),
                        err
                    ));
                }
            }
        }
        let decoder = XzDecoder::new(reader);
        let mut tar = Archive::new(decoder);

        let mut id = None;
        let mut target = None;
        let mut package_type = PackageType::Standard;
        let mut source = None;
        let mut licenses = Vec::new();
        let mut deps = HashSet::new();
        let mut tdeps = HashSet::new();
        let mut build_deps = HashSet::new();
        let mut runtime_path = Vec::new();
        let mut interpreters = Vec::new();
        let mut empty_top_level_dirs = HashSet::new();
        let mut broken_links = HashMap::new();
        let mut empty_links = HashSet::new();
        let mut links = BTreeMap::new();
        let mut scripts = HashMap::new();
        let mut elfs = HashMap::new();

        for entry in tar.entries()? {
            let mut entry = entry?;
            let header = entry.header();
            let entry_type = header.entry_type();
            let path = entry.path()?;
            let entry_install_path = FSRootPath::default().as_ref().join(&path);
            if entry_type.is_dir() {
                let is_top_level_dir = entry_install_path.components().count() == 8;
                if is_top_level_dir {
                    empty_top_level_dirs
                        .insert(entry_install_path.components().take(8).collect::<PathBuf>());
                }
                continue;
            }

            let top_level_dir = entry_install_path.components().take(8).collect::<PathBuf>();
            empty_top_level_dirs.remove(&top_level_dir);

            if entry_type.is_hard_link() || entry_type.is_symlink() {
                if let Ok(Some(link_path)) = header.link_name() {
                    let canonical_link_path = if link_path.is_relative() {
                        entry_install_path
                            .parent()
                            .unwrap()
                            .join(link_path)
                            .absolutize()
                            .unwrap()
                            .to_path_buf()
                    } else {
                        link_path.absolutize().unwrap().to_path_buf()
                    };
                    if !canonical_link_path.is_package_path() {
                        broken_links.insert(entry_install_path, canonical_link_path);
                    } else {
                        links.insert(entry_install_path, canonical_link_path);
                    }
                } else {
                    empty_links.insert(entry_install_path);
                }
                continue;
            } else if !entry_type.is_file() {
                continue;
            }

            let file_name = path.file_name().unwrap().to_str().unwrap();
            let file_mode = header.mode()?;
            let matches = METADATA_GLOBSET.matches(&path);
            // Check if the file is executable
            // https://stackoverflow.com/questions/37062143/how-to-check-if-file-is-executable-using-bitwise-operations-in-rust
            if !matches.is_empty() {
                match file_name {
                    "IDENT" => {
                        let mut data = String::new();
                        entry.read_to_string(&mut data)?;
                        id = Some(PackageDepIdent::parse(data.trim())?);
                    }
                    "PACKAGE_TYPE" => {
                        let mut data = String::new();
                        entry.read_to_string(&mut data)?;
                        package_type = PackageType::parse(data.trim())?;
                    }
                    "DEPS" => {
                        let mut data = String::new();
                        entry.read_to_string(&mut data)?;
                        deps = data
                            .lines()
                            .map(PackageDepIdent::parse)
                            .collect::<Result<HashSet<_>>>()?;
                    }
                    "TDEPS" => {
                        let mut data = String::new();
                        entry.read_to_string(&mut data)?;
                        tdeps = data
                            .lines()
                            .map(PackageDepIdent::parse)
                            .collect::<Result<HashSet<_>>>()?;
                    }
                    "BUILD_DEPS" => {
                        let mut data = String::new();
                        entry.read_to_string(&mut data)?;
                        build_deps = data
                            .lines()
                            .map(PackageDepIdent::parse)
                            .collect::<Result<HashSet<_>>>()?;
                    }
                    "RUNTIME_PATH" => {
                        let mut data = String::new();
                        entry.read_to_string(&mut data)?;
                        runtime_path = data.split(':').map(PathBuf::from).collect::<Vec<_>>();
                    }
                    "INTERPRETERS" => {
                        let mut data = String::new();
                        entry.read_to_string(&mut data)?;
                        interpreters = data.lines().map(PathBuf::from).collect::<Vec<_>>();
                    }
                    "MANIFEST" => {
                        let mut entry = BufReader::new(entry);
                        let mut pkg_source = None;
                        let mut pkg_shasum = None;
                        loop {
                            let mut line = String::new();
                            match entry.read_line(&mut line) {
                                Ok(0) => break,
                                Ok(_) => {
                                    if let Some(value) = line.strip_prefix("* __Target__:") {
                                        let patterns: &[_] = &[' ', '`', '\n'];
                                        target =
                                            PackageTarget::parse(value.trim_matches(patterns)).ok();
                                    }
                                    if let Some(value) = line.strip_prefix("* __Source__:") {
                                        let src = value
                                            .trim()
                                            .split_terminator(&['[', ']'])
                                            .collect::<Vec<_>>();
                                        if let Some(url) = src.get(1) {
                                            pkg_source = Some(Url::parse(url)?);
                                        }
                                    }
                                    if let Some(value) = line.strip_prefix("* __SHA__:") {
                                        let patterns: &[_] = &[' ', '`', '\n'];
                                        pkg_shasum = Some(value.trim_matches(patterns).to_owned());
                                    }
                                    if let Some(value) = line.strip_prefix("* __License__:") {
                                        licenses = value
                                            .trim()
                                            .split(' ')
                                            .map(String::from)
                                            .collect::<Vec<String>>();
                                    }
                                }
                                Err(err) => {
                                    error!(target: "user-log", "Failed to read MANIFEST file: {}", err);
                                    break;
                                }
                            }
                        }
                        if let (Some(url), Some(shasum)) = (pkg_source, pkg_shasum) {
                            source = Some(PackageSource {
                                url: PackageSourceURL::from(url),
                                shasum: PackageSha256Sum::from(shasum),
                            })
                        }
                    }
                    _ => {}
                }
            } else {
                match Resource::from_reader(&entry_install_path, file_mode, entry) {
                    Ok(resource) => match resource {
                        Resource::Elf(metadata) => {
                            elfs.insert(entry_install_path.to_path_buf(), metadata);
                        }
                        Resource::Script(metadata) => {
                            scripts.insert(entry_install_path.to_path_buf(), metadata);
                        }
                        Resource::Other => {}
                    },
                    Err(err) => {
                        error!(target: "user-log", "Failed to read entry {} in artifact {}: {}", entry_install_path.display(), artifact_path.as_ref().display(), err);
                    }
                }
            }
        }

        let target = target.ok_or(eyre!(
            "Package artifact missing target in MANIFEST metafile"
        ))?;
        let id = id
            .ok_or(eyre!("Package artifact missing IDENT metafile"))?
            .to_resolved_dep_ident(target)
            .to_ident()
            .unwrap();
        let deps = deps
            .into_iter()
            .map(|d| d.to_resolved_dep_ident(target).to_ident().unwrap())
            .collect();
        let tdeps = tdeps
            .into_iter()
            .map(|d| d.to_resolved_dep_ident(target).to_ident().unwrap())
            .collect();
        let build_deps = build_deps
            .into_iter()
            .map(|d| d.to_resolved_dep_ident(target).to_ident().unwrap())
            .collect();
        Ok(ArtifactContext {
            id,
            target,
            package_type,
            source,
            deps,
            tdeps,
            build_deps,
            licenses,
            runtime_path,
            interpreters,
            empty_top_level_dirs,
            broken_links,
            empty_links,
            links,
            scripts,
            elfs,
            hash: hash.clone(),
            is_dirty: true,
            created_at: DateTime::<Utc>::from(created_at),
        })
    }
}

pub(crate) enum Resource {
    Elf(ElfMetadata),
    Script(ScriptMetadata),
    Other,
}

impl Resource {
    pub fn from_reader(
        path: impl AsRef<Path>,
        file_mode: u32,
        reader: impl Read,
    ) -> Result<Resource> {
        if let Some((file_type, data)) =
            FileKind::maybe_read_file(reader, &[FileKind::Elf, FileKind::Script])
        {
            match file_type {
                FileKind::Script => {
                    let mut line = String::new();
                    let mut reader = BufReader::new(data.as_slice());
                    reader.read_line(&mut line)?;
                    let mut parts = line.strip_prefix("#!").unwrap().trim().split(' ');
                    let command = PathBuf::from(
                        parts
                            .next()
                            .ok_or(eyre!("Missing interpreter command"))?
                            .to_string(),
                    );
                    let args = parts.map(String::from).collect();
                    Ok(Resource::Script(ScriptMetadata {
                        interpreter: ScriptInterpreterMetadata {
                            raw: line,
                            command,
                            args,
                        },
                        is_executable: file_mode & 0o111 != 0,
                    }))
                }
                FileKind::Elf => {
                    let object = Object::parse(&data)?;
                    // Determine the exact elf type, for more details check the following:
                    // ELF Header (Section 1-3): https://www.cs.cmu.edu/afs/cs/academic/class/15213-f00/docs/elf.pdf
                    // https://unix.stackexchange.com/questions/89211/how-to-test-whether-a-linux-binary-was-compiled-as-position-independent-code/435038#435038
                    if let Object::Elf(object) = object {
                        let is_executable = file_mode & 0o111 != 0;
                        let elf_type = if object.header.e_type == ET_DYN {
                            if let Some(dynamic) = object.dynamic {
                                if dynamic.info.flags_1 & DF_1_PIE == DF_1_PIE {
                                    ElfType::PieExecutable
                                } else {
                                    ElfType::SharedLibrary
                                }
                            } else if is_executable {
                                ElfType::Executable
                            } else {
                                ElfType::SharedLibrary
                            }
                        } else if object.header.e_type == ET_EXEC {
                            ElfType::Executable
                        } else {
                            ElfType::Other
                        };

                        Ok(Resource::Elf(ElfMetadata {
                            required_libraries: object
                                .libraries
                                .into_iter()
                                .map(String::from)
                                .collect(),
                            rpath: object
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
                                .collect::<Vec<_>>(),
                            runpath: object
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
                                .collect::<Vec<_>>(),
                            interpreter: object.interpreter.map(PathBuf::from),
                            elf_type,
                            is_executable,
                        }))
                    } else {
                        Err(eyre!("Unexpected binary type"))
                    }
                }
                _ => unreachable!(),
            }
        } else {
            Ok(Resource::Other)
        }
    }
}

pub(crate) struct ArtifactIndexerBuilder<'a> {
    store: &'a Store,
    sender: Sender<ArtifactContext>,
}

impl<'s, 'a> ParallelVisitorBuilder<'s> for ArtifactIndexerBuilder<'a>
where
    'a: 's,
{
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(ArtifactIndexer {
            store: self.store,
            sender: self.sender.clone(),
        })
    }
}

impl<'a> ArtifactIndexerBuilder<'a> {
    pub fn new(store: &'a Store, sender: Sender<ArtifactContext>) -> ArtifactIndexerBuilder {
        ArtifactIndexerBuilder { store, sender }
    }
}
