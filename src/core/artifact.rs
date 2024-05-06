use chrono::{DateTime, NaiveDateTime, Utc};
use color_eyre::{
    eyre::{eyre, Context, Result},
    Help, SectionExt,
};
use diesel::Connection;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use goblin::{
    elf64::{
        dynamic::DF_1_PIE,
        header::{ET_DYN, ET_EXEC},
    },
    mach::{Mach, SingleArch},
    Object,
};
use ignore::{ParallelVisitor, ParallelVisitorBuilder, WalkBuilder, WalkState};
use lazy_static::lazy_static;
use path_absolutize::Absolutize;
use rayon::prelude::*;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsStr,
    fmt::Display,
    io::{BufRead, BufReader, Read, Write},
    ops::Deref,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        mpsc::{channel, Sender},
        Arc, RwLock, RwLockWriteGuard,
    },
    time::Instant,
};
use tar::Archive;
use tracing::{debug, error, info, trace};
use xz2::bufread::XzDecoder;

use crate::{
    core::{PackageArch, PackageOS},
    store::{self, Store},
};

use super::{
    Blake3, FSRootPath, FileKind, HabitatRootPath, PackageBuildVersion, PackageDepIdent,
    PackageIdent, PackageName, PackageOrigin, PackagePath, PackageRelease, PackageResolvedDepIdent,
    PackageResolvedRelease, PackageResolvedVersion, PackageSha256Sum, PackageSource,
    PackageSourceURL, PackageTarget, PackageType, PackageVersion, PlanContextID,
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
            "hab/pkgs/*/*/*/*/TARGET",
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
const ARTIFACT_DATA_EXTRACT_SCRIPT: &[u8] = include_bytes!("../scripts/artifact_data_extract.sh");

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct ArtifactCachePath(PathBuf);

impl ArtifactCachePath {
    pub fn new(hab_root: HabitatRootPath) -> ArtifactCachePath {
        ArtifactCachePath(hab_root.as_ref().join("cache").join("artifacts"))
    }
    pub fn artifact_path(&self, ident: &PackageIdent) -> ArtifactPath {
        ArtifactPath(self.0.join(ident.artifact_name()))
    }
}

impl AsRef<Path> for ArtifactCachePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl Default for ArtifactCachePath {
    fn default() -> Self {
        ArtifactCachePath(
            FSRootPath::default().as_ref().join(
                ["hab", "cache", "artifacts"]
                    .into_iter()
                    .collect::<PathBuf>(),
            ),
        )
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
            BTreeMap<PackageResolvedVersion, BTreeMap<PackageResolvedRelease, LazyArtifactContext>>,
        >,
    >,
>;

#[derive(Debug, Clone)]
pub(crate) enum LazyArtifactContext {
    NotLoaded(MinimalArtifactContext),
    Loaded(ArtifactContext),
}

impl LazyArtifactContext {
    pub fn id(&self) -> &PackageIdent {
        match self {
            LazyArtifactContext::NotLoaded(ctx) => &ctx.id,
            LazyArtifactContext::Loaded(ctx) => &ctx.id,
        }
    }
}

pub(crate) struct ArtifactCache {
    pub path: ArtifactCachePath,
    known_artifacts: Arc<RwLock<ArtifactList>>,
    store: Store,
}

impl ArtifactCache {
    pub fn new(artifact_cache_path: ArtifactCachePath, store: &Store) -> Result<ArtifactCache> {
        let start = Instant::now();
        let artifact_cache = ArtifactCache {
            path: artifact_cache_path,
            known_artifacts: Arc::new(RwLock::new(ArtifactList::default())),
            store: store.clone(),
        };
        let artifact_cache_walker = WalkBuilder::new(artifact_cache.path.as_ref()).build_parallel();
        std::thread::scope(|scope| {
            let (sender, receiver) = channel();
            let mut artifact_indexer_builder = ArtifactIndexerBuilder::new(store, sender);
            let artifact_indexer_thread =
                scope.spawn(move || artifact_cache_walker.visit(&mut artifact_indexer_builder));
            let mut known_artifact_count = 0;

            while let Ok(artifact_ctx) = receiver.recv() {
                known_artifact_count += 1;
                artifact_cache.artifact_add(store, artifact_ctx)?;
            }
            artifact_indexer_thread
                .join()
                .expect("Failed to join artifact indexer thread to parent thread");
            info!(
                "Detected {} artifacts at {} in {}s",
                known_artifact_count,
                artifact_cache.path.as_ref().display(),
                start.elapsed().as_secs_f32()
            );
            Ok(artifact_cache)
        })
    }

    pub fn artifact_add(
        &self,
        _store: &Store,
        artifact_ctx: LazyArtifactContext,
    ) -> Result<PackageIdent> {
        let mut known_artifacts = self.known_artifacts.write().unwrap();
        if let LazyArtifactContext::Loaded(artifact_ctx) = &artifact_ctx {
            if artifact_ctx.is_dirty {
                self.store_artifact(&mut known_artifacts, artifact_ctx)?;
            }
        }
        let artifact_ident = artifact_ctx.id().clone();
        self.index_artifact(&mut known_artifacts, artifact_ctx);
        Ok(artifact_ident)
    }

    fn store_artifact(
        &self,
        _known_artifacts: &mut RwLockWriteGuard<'_, ArtifactList>, // we take reference to the write lock so we can gaurantee that there is no other modification in flight
        artifact_ctx: &ArtifactContext,
    ) -> Result<()> {
        self.store
            .get_connection()?
            .immediate_transaction(|connection| {
                store::artifact_context_put(connection, &artifact_ctx.hash, artifact_ctx)
                    .with_context(|| format!("Failed to add artifact {} to store", artifact_ctx.id))
            })?;
        trace!("Added artifact {} to store", artifact_ctx.id);
        Ok(())
    }

    fn index_artifact(
        &self,
        known_artifacts: &mut RwLockWriteGuard<'_, ArtifactList>,
        lazy_artifact_ctx: LazyArtifactContext,
    ) {
        let artifact_ident = lazy_artifact_ctx.id().clone();
        known_artifacts
            .entry(artifact_ident.origin.clone())
            .or_default()
            .entry(artifact_ident.name.clone())
            .or_default()
            .entry(artifact_ident.target)
            .or_default()
            .entry(artifact_ident.version.clone())
            .or_default()
            .insert(artifact_ident.release.clone(), lazy_artifact_ctx);

        trace!("Indexed artifact {}", artifact_ident);
    }

    pub fn latest_plan_minimal_artifact(
        &self,
        build_ident: &PlanContextID,
    ) -> Option<MinimalArtifactContext> {
        let build_ident = build_ident.as_ref();
        self.known_artifacts
            .read()
            .unwrap()
            .get(&build_ident.origin)
            .and_then(|a| a.get(&build_ident.name))
            .and_then(|a| a.get(&build_ident.target))
            .and_then(|a| match &build_ident.version {
                PackageBuildVersion::Static(version) => a.get(version),
                PackageBuildVersion::Dynamic => a.values().next_back(),
            })
            .and_then(|a| a.values().next_back())
            .map(|a| match a {
                LazyArtifactContext::NotLoaded(a) => a.clone(),
                LazyArtifactContext::Loaded(a) => MinimalArtifactContext::from(a),
            })
    }

    pub fn latest_plan_artifact(
        &self,
        build_ident: &PlanContextID,
    ) -> Result<Option<ArtifactContext>> {
        let build_ident = build_ident.as_ref();
        let lazy_artifact = self
            .known_artifacts
            .read()
            .unwrap()
            .get(&build_ident.origin)
            .and_then(|a| a.get(&build_ident.name))
            .and_then(|a| a.get(&build_ident.target))
            .and_then(|a| match &build_ident.version {
                PackageBuildVersion::Static(version) => a.get(version),
                PackageBuildVersion::Dynamic => a.values().next_back(),
            })
            .and_then(|a| a.values().next_back())
            .cloned();
        self.load_lazy_artifact(lazy_artifact)
    }

    pub fn latest_minimal_artifact(
        &self,
        dep_ident: &PackageResolvedDepIdent,
    ) -> Option<MinimalArtifactContext> {
        self.known_artifacts
            .read()
            .unwrap()
            .get(&dep_ident.origin)
            .and_then(|a| a.get(&dep_ident.name))
            .and_then(|a| a.get(&dep_ident.target))
            .and_then(|a| match &dep_ident.version {
                PackageVersion::Resolved(version) => a.get(version),
                PackageVersion::Unresolved => a.values().next_back(),
            })
            .and_then(|a| match &dep_ident.release {
                PackageRelease::Resolved(release) => a.get(release),
                PackageRelease::Unresolved => a.values().next_back(),
            })
            .map(|a| match a {
                LazyArtifactContext::NotLoaded(a) => a.clone(),
                LazyArtifactContext::Loaded(a) => MinimalArtifactContext::from(a),
            })
    }

    pub fn latest_artifact(
        &self,
        dep_ident: &PackageResolvedDepIdent,
    ) -> Result<Option<ArtifactContext>> {
        let lazy_artifact = self
            .known_artifacts
            .read()
            .unwrap()
            .get(&dep_ident.origin)
            .and_then(|a| a.get(&dep_ident.name))
            .and_then(|a| a.get(&dep_ident.target))
            .and_then(|a| match &dep_ident.version {
                PackageVersion::Resolved(version) => a.get(version),
                PackageVersion::Unresolved => a.values().next_back(),
            })
            .and_then(|a| match &dep_ident.release {
                PackageRelease::Resolved(release) => a.get(release),
                PackageRelease::Unresolved => a.values().next_back(),
            })
            .cloned();
        self.load_lazy_artifact(lazy_artifact)
    }

    #[allow(dead_code)]
    pub fn minimal_artifact(&self, dep_ident: &PackageIdent) -> Option<MinimalArtifactContext> {
        self.known_artifacts
            .read()
            .unwrap()
            .get(&dep_ident.origin)
            .and_then(|a| a.get(&dep_ident.name))
            .and_then(|a| a.get(&dep_ident.target))
            .and_then(|a| a.get(&dep_ident.version))
            .and_then(|a| a.get(&dep_ident.release))
            .map(|a| match a {
                LazyArtifactContext::NotLoaded(a) => a.clone(),
                LazyArtifactContext::Loaded(a) => MinimalArtifactContext::from(a),
            })
    }

    pub fn artifact(&self, dep_ident: &PackageIdent) -> Result<Option<ArtifactContext>> {
        let lazy_artifact = self
            .known_artifacts
            .read()
            .unwrap()
            .get(&dep_ident.origin)
            .and_then(|a| a.get(&dep_ident.name))
            .and_then(|a| a.get(&dep_ident.target))
            .and_then(|a| a.get(&dep_ident.version))
            .and_then(|a| a.get(&dep_ident.release))
            .cloned();
        self.load_lazy_artifact(lazy_artifact)
    }
    fn load_lazy_artifact(
        &self,
        lazy_artifact: Option<LazyArtifactContext>,
    ) -> Result<Option<ArtifactContext>> {
        match lazy_artifact {
            Some(lazy_artifact) => match lazy_artifact {
                LazyArtifactContext::NotLoaded(minimal_artifact_ctx) => {
                    // Check known artifacts once before attempting to read from disk
                    let mut known_artifacts = self.known_artifacts.write().unwrap();
                    let dep_ident = &minimal_artifact_ctx.id;
                    let known_artifact = known_artifacts
                        .get(&dep_ident.origin)
                        .and_then(|a| a.get(&dep_ident.name))
                        .and_then(|a| a.get(&dep_ident.target))
                        .and_then(|a| a.get(&dep_ident.version))
                        .and_then(|a| a.get(&dep_ident.release));
                    if let Some(LazyArtifactContext::Loaded(artifact_ctx)) = known_artifact {
                        Ok(Some(artifact_ctx.clone()))
                    } else {
                        let artifact_ctx = ArtifactContext::read_from_disk(
                            minimal_artifact_ctx.path.as_ref().unwrap(),
                            None,
                        )?;
                        self.store_artifact(&mut known_artifacts, &artifact_ctx)?;
                        self.index_artifact(
                            &mut known_artifacts,
                            LazyArtifactContext::Loaded(artifact_ctx.clone()),
                        );
                        Ok(Some(artifact_ctx))
                    }
                }
                LazyArtifactContext::Loaded(artifact_ctx) => Ok(Some(artifact_ctx)),
            },
            None => Ok(None),
        }
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

impl Display for ElfType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElfType::Executable => write!(f, "executable"),
            ElfType::SharedLibrary => write!(f, "shared-library"),
            ElfType::PieExecutable => write!(f, "pie-executable"),
            ElfType::Relocatable => write!(f, "relocatable"),
            ElfType::Other => write!(f, "other"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MachOMetadata {
    pub archs: Vec<SingleArchMachOMetadata>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct SingleArchMachOMetadata {
    pub arch: (u32, u32),
    pub name: Option<String>,
    pub required_libraries: Vec<String>,
    pub rpath: Vec<PathBuf>,
    pub file_type: MachOType,
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub(crate) enum MachOType {
    #[serde(rename = "object")]
    Object,
    #[serde(rename = "executable")]
    Executable,
    #[serde(rename = "dynamic-library")]
    DynamicLibrary,
    #[serde(rename = "preload")]
    Preload,
    #[serde(rename = "core")]
    Core,
    #[serde(rename = "dynamic-linker")]
    DynamicLinker,
    #[serde(rename = "bundle")]
    Bundle,
    #[serde(rename = "dynamic-library-stub")]
    DynamicLibraryStub,
    #[serde(rename = "debug-symbols")]
    DebugSymbols,
    #[serde(rename = "other")]
    Other(u32),
}

impl Display for MachOType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MachOType::Object => write!(f, "object"),
            MachOType::Executable => write!(f, "executable"),
            MachOType::DynamicLibrary => write!(f, "dynamic-library"),
            MachOType::Preload => write!(f, "preload"),
            MachOType::Core => write!(f, "core"),
            MachOType::DynamicLinker => write!(f, "dynamic-linker"),
            MachOType::Bundle => write!(f, "bundle"),
            MachOType::DynamicLibraryStub => write!(f, "dynamic-library-stub"),
            MachOType::DebugSymbols => write!(f, "debug-symbols"),
            MachOType::Other(_) => write!(f, "other"),
        }
    }
}

impl From<u32> for MachOType {
    fn from(value: u32) -> Self {
        match value {
            0x1 => MachOType::Object,
            0x2 => MachOType::Executable,
            0x4 => MachOType::Core,
            0x5 => MachOType::Preload,
            0x6 => MachOType::DynamicLibrary,
            0x7 => MachOType::DynamicLinker,
            0x8 => MachOType::Bundle,
            0x9 => MachOType::DynamicLibraryStub,
            0xA => MachOType::DebugSymbols,
            value => MachOType::Other(value),
        }
    }
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
pub(crate) struct RawArtifactData {
    pub licenses: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactContext(Arc<InnerArtifactContext>);

impl Deref for ArtifactContext {
    type Target = InnerArtifactContext;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<InnerArtifactContext> for ArtifactContext {
    fn from(value: InnerArtifactContext) -> Self {
        Self(Arc::new(value))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct InnerArtifactContext {
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
    pub machos: HashMap<PathBuf, MachOMetadata>,
    pub empty_top_level_dirs: HashSet<PathBuf>,
    pub links: BTreeMap<PathBuf, PathBuf>,
    pub broken_links: HashMap<PathBuf, PathBuf>,
    pub empty_links: HashSet<PathBuf>,
    pub scripts: HashMap<PathBuf, ScriptMetadata>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct MinimalArtifactContext(Arc<InnerMinimalArtifactContext>);

impl From<InnerMinimalArtifactContext> for MinimalArtifactContext {
    fn from(value: InnerMinimalArtifactContext) -> Self {
        Self(Arc::new(value))
    }
}

impl Deref for MinimalArtifactContext {
    type Target = InnerMinimalArtifactContext;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InnerMinimalArtifactContext {
    pub id: PackageIdent,
    pub created_at: DateTime<Utc>,
    pub path: Option<PathBuf>,
}

impl From<&ArtifactContext> for MinimalArtifactContext {
    fn from(artifact_ctx: &ArtifactContext) -> Self {
        InnerMinimalArtifactContext {
            id: artifact_ctx.id.clone(),
            created_at: artifact_ctx.created_at,
            path: None,
        }
        .into()
    }
}

enum RawArtifactItem {
    MetaFile(String, String),
    Resource(PathBuf, u32, FileKind, Vec<u8>),
}
enum IndexedArtifactItem {
    PackageIdent(PackageDepIdent),
    PackageTarget(PackageTarget),
    PackageType(PackageType),
    PackageSource(PackageSource),
    Licenses(Vec<String>),
    PackageDeps(HashSet<PackageDepIdent>),
    PackageTDeps(HashSet<PackageDepIdent>),
    PackageBuildDeps(HashSet<PackageDepIdent>),
    RuntimePath(Vec<PathBuf>),
    Interpreters(Vec<PathBuf>),
    Script((PathBuf, ScriptMetadata)),
    Elf((PathBuf, ElfMetadata)),
    MachO((PathBuf, MachOMetadata)),
}

impl ArtifactContext {
    pub fn lazy_read_from_disk(
        artifact_path: impl AsRef<Path>,
        _hash: Option<&Blake3>,
    ) -> Result<MinimalArtifactContext> {
        let start = Instant::now();
        let f = std::fs::File::open(artifact_path.as_ref())?;
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
        let target = artifact_path
            .as_ref()
            .file_stem()
            .and_then(|v| v.to_str())
            .map(|v| {
                let mut iter = v.rsplitn(3, '-');
                let os = iter.next().map(PackageOS::parse).transpose()?;
                let arch = iter.next().map(PackageArch::parse).transpose()?;
                Ok::<PackageTarget, color_eyre::eyre::Error>(PackageTarget {
                    arch: arch.ok_or(eyre!("Invalid artifact target architecture"))?,
                    os: os.ok_or(eyre!("Invalid artifact target os"))?,
                })
            })
            .transpose()?
            .ok_or(eyre!("Invalid artifact name"))?;

        if let Some(entry) = (tar.entries()?).next() {
            let entry = entry?;
            let path = entry.path()?;

            id = path.package_ident(target);
        }

        let id = id.ok_or(eyre!("Package artifact malformed"))?;
        debug!(
            "Artifact {} metadata loaded from {} in {}s",
            id,
            artifact_path.as_ref().display(),
            start.elapsed().as_secs_f32()
        );
        Ok(InnerMinimalArtifactContext {
            created_at: DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDateTime::parse_from_str(id.release.to_string().as_str(), "%Y%m%d%H%M%S")
                    .expect("Invalid release value"),
                Utc,
            ),
            id,
            path: Some(artifact_path.as_ref().to_path_buf()),
        }
        .into())
    }

    pub fn read_from_disk(
        artifact_path: impl AsRef<Path>,
        hash: Option<&Blake3>,
    ) -> Result<ArtifactContext> {
        let start = Instant::now();

        let f = std::fs::File::open(artifact_path.as_ref())?;
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
        let mut machos = HashMap::new();

        let indexed_item_batches = tar
            .entries()?
            .filter_map(|entry| entry.ok())
            .map(|mut entry| {
                let header = entry.header();
                let entry_type = header.entry_type();
                let path = entry.path()?.to_path_buf();
                let entry_install_path = FSRootPath::default().as_ref().join(&path);
                if entry_type.is_dir() {
                    let is_top_level_dir = entry_install_path.components().count() == 8;
                    if is_top_level_dir {
                        empty_top_level_dirs
                            .insert(entry_install_path.components().take(8).collect::<PathBuf>());
                    }
                    return Ok::<_, color_eyre::eyre::Error>(None);
                }

                let top_level_dir = entry_install_path.components().take(8).collect::<PathBuf>();
                empty_top_level_dirs.remove(&top_level_dir);

                if entry_type.is_hard_link() || entry_type.is_symlink() {
                    if let Ok(Some(link_path)) = header.link_name() {
                        let canonical_link_path = if link_path.is_relative() {
                            if entry_type.is_hard_link() {
                                FSRootPath::default().as_ref().join(link_path)
                            } else {
                                entry_install_path
                                    .parent()
                                    .unwrap()
                                    .join(link_path)
                                    .absolutize()
                                    .unwrap()
                                    .to_path_buf()
                            }
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
                    return Ok::<_, color_eyre::eyre::Error>(None);
                } else if !entry_type.is_file() {
                    return Ok::<_, color_eyre::eyre::Error>(None);
                }

                let file_name = path.file_name().unwrap().to_str().unwrap();
                let file_mode = header.mode()?;
                let matches = METADATA_GLOBSET.matches(&path);
                // Check if the file is executable
                // https://stackoverflow.com/questions/37062143/how-to-check-if-file-is-executable-using-bitwise-operations-in-rust
                if !matches.is_empty() {
                    let mut data = String::new();
                    entry.read_to_string(&mut data)?;
                    Ok::<_, color_eyre::eyre::Error>(Some(RawArtifactItem::MetaFile(
                        file_name.to_string(),
                        data,
                    )))
                } else if let Some((kind, data)) = FileKind::maybe_read_file(
                    entry,
                    &[FileKind::Elf, FileKind::Script, FileKind::MachBinary],
                ) {
                    Ok::<_, color_eyre::eyre::Error>(Some(RawArtifactItem::Resource(
                        entry_install_path,
                        file_mode,
                        kind,
                        data,
                    )))
                } else {
                    Ok::<_, color_eyre::eyre::Error>(None)
                }
            })
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|raw_item| {
                if let Some(raw_item) = raw_item? {
                    match raw_item {
                        RawArtifactItem::MetaFile(file_name, data) => {
                            Ok::<_, color_eyre::eyre::Error>(match file_name.as_str() {
                                "IDENT" => {
                                    vec![IndexedArtifactItem::PackageIdent(PackageDepIdent::parse(
                                        data.trim(),
                                    )?)]
                                }
                                "PACKAGE_TYPE" => {
                                    vec![IndexedArtifactItem::PackageType(PackageType::parse(
                                        data.trim(),
                                    )?)]
                                }
                                "DEPS" => {
                                    vec![IndexedArtifactItem::PackageDeps(
                                        data.lines()
                                            .map(PackageDepIdent::parse)
                                            .collect::<Result<HashSet<_>>>()?,
                                    )]
                                }
                                "TDEPS" => {
                                    vec![IndexedArtifactItem::PackageTDeps(
                                        data.lines()
                                            .map(PackageDepIdent::parse)
                                            .collect::<Result<HashSet<_>>>()?,
                                    )]
                                }
                                "BUILD_DEPS" => {
                                    vec![IndexedArtifactItem::PackageBuildDeps(
                                        data.lines()
                                            .map(PackageDepIdent::parse)
                                            .collect::<Result<HashSet<_>>>()?,
                                    )]
                                }
                                "RUNTIME_PATH" => {
                                    vec![IndexedArtifactItem::RuntimePath(
                                        data.split(':').map(PathBuf::from).collect::<Vec<_>>(),
                                    )]
                                }
                                "INTERPRETERS" => {
                                    vec![IndexedArtifactItem::Interpreters(
                                        data.lines().map(PathBuf::from).collect::<Vec<_>>(),
                                    )]
                                }
                                "MANIFEST" => {
                                    let mut result = Vec::new();
                                    let mut pkg_source = None;
                                    let mut pkg_shasum = None;
                                    let mut plan_source = String::new();
                                    let mut plan_source_header_read = false;
                                    for line in data.lines() {
                                        if let Some(value) = line.strip_prefix("* __Target__:") {
                                            let patterns: &[_] = &[' ', '`', '\n'];
                                            if let Ok(target) =
                                                PackageTarget::parse(value.trim_matches(patterns))
                                            {
                                                result.push(IndexedArtifactItem::PackageTarget(
                                                    target,
                                                ));
                                            }
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
                                            pkg_shasum =
                                                Some(value.trim_matches(patterns).to_owned());
                                        }
                                        if plan_source_header_read {
                                            plan_source.push_str(line);
                                            plan_source.push('\n');
                                        }
                                        if line.starts_with("## Plan Source") {
                                            plan_source_header_read = true;
                                        }
                                    }
                                    if let (Some(url), Some(shasum)) = (pkg_source, pkg_shasum) {
                                        result.push(IndexedArtifactItem::PackageSource(
                                            PackageSource {
                                                url: PackageSourceURL::from(url),
                                                shasum: PackageSha256Sum::from(shasum),
                                            },
                                        ));
                                    }
                                    plan_source = plan_source
                                        .split_once("```bash")
                                        .unwrap()
                                        .1
                                        .rsplit_once("```")
                                        .unwrap()
                                        .0
                                        .to_string();
                                    result.push(IndexedArtifactItem::Licenses(
                                        ArtifactContext::extract_licenses_from_plan_source(
                                            &plan_source,
                                        )?,
                                    ));
                                    result
                                }
                                _ => {
                                    vec![]
                                }
                            })
                        }
                        RawArtifactItem::Resource(path, file_mode, kind, data) => {
                            Ok(match Resource::from_data(&path, file_mode, kind, data) {
                                Err(err) => {
                                    error!(
                                        "Failed to read {} detected as {:?} resource: {:?}",
                                        path.display(),
                                        kind,
                                        err
                                    );
                                    vec![]
                                }
                                Ok(resource) => match resource {
                                    Resource::Elf(metadata) => {
                                        vec![IndexedArtifactItem::Elf((path, metadata))]
                                    }
                                    Resource::Script(metadata) => {
                                        vec![IndexedArtifactItem::Script((path, metadata))]
                                    }
                                    Resource::MachO(metadata) => {
                                        vec![IndexedArtifactItem::MachO((path, metadata))]
                                    }
                                    _ => {
                                        vec![]
                                    }
                                },
                            })
                        }
                    }
                } else {
                    Ok(vec![])
                }
            })
            .collect::<Vec<_>>();

        for indexed_item_batch in indexed_item_batches {
            let indexed_item_batch = indexed_item_batch?;
            for indexed_item in indexed_item_batch {
                match indexed_item {
                    IndexedArtifactItem::PackageIdent(value) => {
                        id = Some(value);
                    }
                    IndexedArtifactItem::PackageTarget(value) => {
                        target = Some(value);
                    }
                    IndexedArtifactItem::PackageType(value) => {
                        package_type = value;
                    }
                    IndexedArtifactItem::PackageSource(value) => {
                        source = Some(value);
                    }
                    IndexedArtifactItem::Licenses(value) => {
                        licenses = value;
                    }
                    IndexedArtifactItem::PackageDeps(value) => {
                        deps = value;
                    }
                    IndexedArtifactItem::PackageTDeps(value) => {
                        tdeps = value;
                    }
                    IndexedArtifactItem::PackageBuildDeps(value) => {
                        build_deps = value;
                    }
                    IndexedArtifactItem::RuntimePath(value) => {
                        runtime_path = value;
                    }
                    IndexedArtifactItem::Interpreters(value) => {
                        interpreters = value;
                    }
                    IndexedArtifactItem::Script((path, metadata)) => {
                        scripts.insert(path, metadata);
                    }
                    IndexedArtifactItem::Elf((path, metadata)) => {
                        elfs.insert(path, metadata);
                    }
                    IndexedArtifactItem::MachO((path, metadata)) => {
                        machos.insert(path, metadata);
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
        let hash = if let Some(hash) = hash {
            hash.clone()
        } else {
            Blake3::from_path(artifact_path.as_ref()).with_context(|| {
                format!(
                    "Failed to generate hash for artifact {}",
                    artifact_path.as_ref().display(),
                )
            })?
        };
        debug!(
            "Artifact {} data loaded from {} in {}s",
            id,
            artifact_path.as_ref().display(),
            start.elapsed().as_secs_f32()
        );
        Ok(InnerArtifactContext {
            created_at: DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDateTime::parse_from_str(id.release.to_string().as_str(), "%Y%m%d%H%M%S")
                    .expect("Invalid release value"),
                Utc,
            ),
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
            machos,
            hash: hash.clone(),
            is_dirty: true,
        }
        .into())
    }

    fn extract_licenses_from_plan_source(plan_source: &str) -> Result<Vec<String>> {
        let mut child =  Command::new("bash")
            .arg("-s")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to execute bash shell")
            .with_suggestion(|| "Make sure you have bash installed on your system, and that it's location is included in your PATH")?;
        let mut stdin = child
            .stdin
            .take()
            .expect("Failed to acquire stdin to bash process");
        stdin.write_all(plan_source.as_bytes())?;
        stdin.write_all(ARTIFACT_DATA_EXTRACT_SCRIPT)?;
        stdin.flush()?;
        drop(stdin);
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            let raw_data: RawArtifactData = serde_json::from_slice(&output.stdout)
                .context("Failed to read extracted JSON data from plan source in MANIFEST")
                .with_section(move || stdout.header("stdout: "))
                .with_section(move || stderr.header("stderr: "))
                .with_section(move || plan_source.to_owned().header("manifest:"))
                .with_suggestion(|| "Ensure your plan file does not generate output outside the standard functions like 'do_begin', 'do_prepare', 'do_build', 'do_check' and 'do_install'")?;
            Ok(raw_data.licenses)
        } else {
            Err(eyre!(
                "Failed to extract plan data from plan source in MANIFEST, bash process exited with code: {}",
                output.status,
            )
            .with_section(move || stdout.header("stdout: "))
            .with_section(move || stderr.header("stderr: ")))
            .with_section(move || plan_source.to_owned().header("manifest:"))
        }
    }
    /// Search for an executable with the given name.
    /// This function only returns a result if the found executable
    /// has the executable permission set.
    pub fn search_runtime_executable(
        &self,
        tdeps: &HashMap<PackageIdent, ArtifactContext>,
        executable_name: impl AsRef<Path>,
    ) -> Option<ExecutableMetadata<'_>> {
        for path in self.runtime_path.iter() {
            let executable_path = path.join(executable_name.as_ref());
            let resolved_executable_path = if let Some(executable_package_ident) =
                executable_path.package_ident(self.target)
            {
                if let Some(tdep) = tdeps.get(&executable_package_ident) {
                    tdep.resolve_path(tdeps, executable_path)
                } else {
                    continue;
                }
            } else {
                continue;
            };
            if let Some(metadata) = self.elfs.get(&resolved_executable_path) {
                return Some(ExecutableMetadata::Elf(metadata));
            }
            if let Some(metadata) = self.scripts.get(&resolved_executable_path) {
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
        let mut resolved_path = path.as_ref().to_path_buf();
        let mut current_artifact =
            if let Some(next_artifact_ctx) = resolved_path.package_ident(self.target) {
                tdeps.get(&next_artifact_ctx)
            } else {
                None
            };
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
                        && !artifact_ctx.links.contains_key(&link)
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
                        resolved_path =
                            parent_link.join(resolved_path.strip_prefix(parent).unwrap());
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

    pub fn resolve_path_and_intermediates(
        &self,
        tdeps: &HashMap<PackageIdent, ArtifactContext>,
        path: impl AsRef<Path>,
    ) -> (PathBuf, Vec<PathBuf>) {
        let mut resolved_path = path.as_ref().to_path_buf();
        let mut intermediate_paths = vec![resolved_path.clone()];
        let mut current_artifact =
            if let Some(next_artifact_ctx) = resolved_path.package_ident(self.target) {
                tdeps.get(&next_artifact_ctx)
            } else {
                None
            };
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
                        && !artifact_ctx.links.contains_key(&link)
                    {
                        resolved_path = link.to_path_buf();
                        intermediate_paths.push(resolved_path.clone());
                        current_artifact = None;
                    } else {
                        current_artifact = tdeps.get(&next_artifact_ctx);
                        resolved_path = link.to_path_buf();
                        intermediate_paths.push(resolved_path.clone());
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
                        resolved_path =
                            parent_link.join(resolved_path.strip_prefix(parent).unwrap());
                        intermediate_paths.push(resolved_path.clone());
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
        (resolved_path, intermediate_paths)
    }
}

pub(crate) enum ExecutableMetadata<'a> {
    Elf(&'a ElfMetadata),
    Script(&'a ScriptMetadata),
}

impl<'a> ExecutableMetadata<'a> {
    pub fn is_executable(&self) -> bool {
        match self {
            ExecutableMetadata::Elf(metadata) => metadata.is_executable,
            ExecutableMetadata::Script(metadata) => metadata.is_executable,
        }
    }
}

pub(crate) struct ArtifactIndexer<'a> {
    store: &'a Store,
    sender: Sender<LazyArtifactContext>,
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
                        .send(LazyArtifactContext::Loaded(artifact_ctx))
                        .expect("Failed to send artifact context to parent thread");
                } else {
                    match ArtifactContext::lazy_read_from_disk(entry.path(), Some(&hash)) {
                        Ok(artifact_ctx) => {
                            self.sender
                                .send(LazyArtifactContext::NotLoaded(artifact_ctx))
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

pub(crate) enum Resource {
    Elf(ElfMetadata),
    MachO(MachOMetadata),
    Script(ScriptMetadata),
    JavaClass,
}

impl Resource {
    pub fn from_data(
        path: impl AsRef<Path>,
        file_mode: u32,
        kind: FileKind,
        data: Vec<u8>,
    ) -> Result<Resource> {
        match kind {
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
            FileKind::Elf | FileKind::MachBinary => {
                let object = Object::parse(&data)?;
                // Determine the exact elf type, for more details check the following:
                // ELF Header (Section 1-3): https://www.cs.cmu.edu/afs/cs/academic/class/15213-f00/docs/elf.pdf
                // https://unix.stackexchange.com/questions/89211/how-to-test-whether-a-linux-binary-was-compiled-as-position-independent-code/435038#435038
                match object {
                    Object::Elf(object) => {
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
                    }
                    Object::Mach(macho) => {
                        let mut metadata = MachOMetadata { archs: Vec::new() };
                        match macho {
                            Mach::Fat(macho) => {
                                for index in 0..macho.narches {
                                    match macho.get(index) {
                                        Ok(SingleArch::MachO(macho)) => {
                                            metadata.archs.push(SingleArchMachOMetadata {
                                                arch: (
                                                    macho.header.cputype,
                                                    macho.header.cpusubtype,
                                                ),
                                                name: macho.name.map(String::from),
                                                required_libraries: macho
                                                    .libs
                                                    .into_iter()
                                                    .map(String::from)
                                                    .collect(),
                                                rpath: macho
                                                    .rpaths
                                                    .into_iter()
                                                    .map(PathBuf::from)
                                                    .collect(),
                                                file_type: MachOType::from(macho.header.filetype),
                                            });
                                        }
                                        Ok(SingleArch::Archive(_archive)) => {}
                                        Err(goblin::error::Error::Malformed(_)) => {
                                            // MachBinaries and JavaClasses unfortunately share the same magic bytes. We can only know for sure
                                            // by attempting to parse the object once
                                            // https://stackoverflow.com/questions/73546728/magic-value-collision-between-macho-fat-binaries-and-java-class-files
                                            return Ok(Resource::JavaClass);
                                        }
                                        Err(err) => return Err(eyre!(err)),
                                    }
                                }
                            }
                            Mach::Binary(macho) => {
                                metadata.archs.push(SingleArchMachOMetadata {
                                    arch: (macho.header.cputype, macho.header.cpusubtype),
                                    name: macho.name.map(String::from),
                                    required_libraries: macho
                                        .libs
                                        .into_iter()
                                        .map(String::from)
                                        .collect(),
                                    rpath: macho.rpaths.into_iter().map(PathBuf::from).collect(),
                                    file_type: MachOType::from(macho.header.filetype),
                                });
                            }
                        }
                        Ok(Resource::MachO(metadata))
                    }
                    _ => Err(eyre!("Unexpected binary type")),
                }
            }
            _ => {
                unreachable!()
            }
        }
    }
}

pub(crate) struct ArtifactIndexerBuilder<'a> {
    store: &'a Store,
    sender: Sender<LazyArtifactContext>,
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
    pub fn new(store: &'a Store, sender: Sender<LazyArtifactContext>) -> ArtifactIndexerBuilder {
        ArtifactIndexerBuilder { store, sender }
    }
}
