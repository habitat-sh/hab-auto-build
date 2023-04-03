use std::{
    fmt::Display,
    path::{Path, PathBuf},
    str::FromStr,
};

use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};

use color_eyre::{
    eyre::{eyre, Result},
    Help,
};
use lazy_static::lazy_static;
use regex::Regex;

use super::FSRootPath;

lazy_static! {
    static ref IDENTIFIER_REGEX: Regex = Regex::new("^[A-Za-z0-9_-]+$").unwrap();
}
const DYNAMIC_VERSION: &str = "**DYNAMIC**";
const BUILD_RELEASE: &str = "**DYNAMIC**";

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageName(String);

impl PackageName {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageName> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(eyre!("Package name is empty"));
        }
        if !IDENTIFIER_REGEX.is_match(value) {
            return Err(eyre!("Invalid package name '{}'", value).with_suggestion(|| "The package name can only contain letters(A-Z, a-z), digits(0-9), underscore(_) and minus('-') symbols"));
        }
        Ok(PackageName(value.to_string()))
    }
}

impl Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageOrigin(String);

impl PackageOrigin {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageOrigin> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(eyre!("Package origin is empty"));
        }
        if !IDENTIFIER_REGEX.is_match(value) {
            return Err(eyre!("Invalid package origin '{}'", value).with_suggestion(|| "The package name can only contain letters(A-Z, a-z), digits(0-9), underscore(_) and minus('-') symbols"));
        }
        Ok(PackageOrigin(value.to_string()))
    }
}

impl Display for PackageOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(try_from = "Option<String>", into = "Option<String>")]
pub enum PackageVersion {
    Resolved(PackageResolvedVersion),
    Unresolved,
}

impl TryFrom<Option<String>> for PackageVersion {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: Option<String>) -> std::result::Result<Self, Self::Error> {
        match value {
            Some(value) => Ok(PackageVersion::Resolved(PackageResolvedVersion::parse(
                value,
            )?)),
            None => Ok(PackageVersion::Unresolved),
        }
    }
}

impl From<PackageVersion> for Option<String> {
    fn from(value: PackageVersion) -> Self {
        match value {
            PackageVersion::Resolved(version) => Some(version.to_string()),
            PackageVersion::Unresolved => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageResolvedVersion(String);

impl PackageResolvedVersion {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageResolvedVersion> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(eyre!("Package version is empty"));
        }
        Ok(PackageResolvedVersion(value.to_string()))
    }
}

impl Display for PackageResolvedVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(try_from = "String", into = "String")]
pub enum PackageBuildVersion {
    Static(PackageResolvedVersion),
    Dynamic,
}

impl PackageBuildVersion {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageBuildVersion> {
        let value = value.as_ref();
        if value == DYNAMIC_VERSION {
            Ok(PackageBuildVersion::Dynamic)
        } else {
            Ok(PackageBuildVersion::Static(PackageResolvedVersion::parse(
                value,
            )?))
        }
    }
}

impl Display for PackageBuildVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageBuildVersion::Static(version) => write!(f, "{}", version),
            PackageBuildVersion::Dynamic => write!(f, "{}", DYNAMIC_VERSION),
        }
    }
}

impl TryFrom<String> for PackageBuildVersion {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        PackageBuildVersion::parse(value)
    }
}

impl From<PackageBuildVersion> for String {
    fn from(value: PackageBuildVersion) -> Self {
        value.to_string()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(try_from = "Option<String>", into = "Option<String>")]
pub enum PackageRelease {
    Resolved(PackageResolvedRelease),
    Unresolved,
}

impl TryFrom<Option<String>> for PackageRelease {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: Option<String>) -> std::result::Result<Self, Self::Error> {
        match value {
            Some(value) => Ok(PackageRelease::Resolved(PackageResolvedRelease::parse(
                value,
            )?)),
            None => Ok(PackageRelease::Unresolved),
        }
    }
}

impl From<PackageRelease> for Option<String> {
    fn from(value: PackageRelease) -> Self {
        match value {
            PackageRelease::Resolved(release) => Some(release.to_string()),
            PackageRelease::Unresolved => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageResolvedRelease(String);

impl PackageResolvedRelease {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageResolvedRelease> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(eyre!("Package release is empty"));
        }
        Ok(PackageResolvedRelease(value.to_string()))
    }
}

impl Display for PackageResolvedRelease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageTarget {
    pub arch: PackageArch,
    pub os: PackageOS,
}

impl Default for PackageTarget {
    fn default() -> Self {
        let os = if cfg!(target_os = "linux") {
            PackageOS::Linux
        } else if cfg!(target_os = "macos") {
            PackageOS::Darwin
        } else if cfg!(target_os = "windows") {
            PackageOS::Windows
        } else {
            panic!("Unsupported target os");
        };
        let arch = if cfg!(target_arch = "aarch64") {
            PackageArch::Aarch64
        } else if cfg!(target_arch = "x86_64") {
            PackageArch::X86_64
        } else {
            panic!("Unsupported target architecture");
        };
        PackageTarget { arch, os }
    }
}

impl Display for PackageTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.arch, self.os)
    }
}

impl PackageTarget {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageTarget> {
        let value = value.as_ref();
        if let Some((arch, os)) = value.split_once('-') {
            Ok(PackageTarget {
                arch: PackageArch::parse(arch)?,
                os: PackageOS::parse(os)?,
            })
        } else {
            Err(eyre!("Invalid package target: {}", value))
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackageArch {
    X86_64,
    Aarch64,
}

impl Display for PackageArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageArch::X86_64 => write!(f, "x86_64"),
            PackageArch::Aarch64 => write!(f, "aarch64"),
        }
    }
}

impl PackageArch {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageArch> {
        let value = value.as_ref();
        match value {
            "x86_64" => Ok(PackageArch::X86_64),
            "aarch64" => Ok(PackageArch::Aarch64),
            _ => Err(eyre!("Unsupported package architecture: {}", value)),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackageOS {
    Linux,
    Darwin,
    Windows,
}

impl PackageOS {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageOS> {
        let value = value.as_ref();
        match value {
            "linux" => Ok(PackageOS::Linux),
            "darwin" => Ok(PackageOS::Darwin),
            "windows" => Ok(PackageOS::Windows),
            _ => Err(eyre!("Unsupported package operating system: {}", value)),
        }
    }
}

impl Display for PackageOS {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageOS::Linux => write!(f, "linux"),
            PackageOS::Darwin => write!(f, "darwin"),
            PackageOS::Windows => write!(f, "windows"),
        }
    }
}

pub trait PackagePath {
    fn is_package_path(&self) -> bool;
    fn package_path(&self) -> Option<PathBuf>;
    fn relative_package_path(&self) -> Option<PathBuf>;
    fn package_ident(&self, target: PackageTarget) -> Option<PackageIdent>;
}

impl<T> PackagePath for T
where
    T: AsRef<Path>,
{
    fn is_package_path(&self) -> bool {
        let mut components = self.as_ref().components();

        components.next();
        let hab_folder = components.next().and_then(|c| c.as_os_str().to_str());
        let pkg_folder = components.next().and_then(|c| c.as_os_str().to_str());
        let origin = components.next().and_then(|c| c.as_os_str().to_str());
        let name = components.next().and_then(|c| c.as_os_str().to_str());
        let version = components.next().and_then(|c| c.as_os_str().to_str());
        let release = components.next().and_then(|c| c.as_os_str().to_str());

        if let (Some("hab"), Some("pkgs"), Some(origin), Some(name), Some(version), Some(release)) =
            (hab_folder, pkg_folder, origin, name, version, release)
        {
            matches!(
                (
                    PackageOrigin::parse(origin),
                    PackageName::parse(name),
                    PackageResolvedVersion::parse(version),
                    PackageResolvedRelease::parse(release),
                ),
                (Ok(_), Ok(_), Ok(_), Ok(_))
            )
        } else {
            false
        }
    }
    fn package_path(&self) -> Option<PathBuf> {
        let mut components = self.as_ref().components();

        components.next();
        let hab_folder = components.next().and_then(|c| c.as_os_str().to_str());
        let pkg_folder = components.next().and_then(|c| c.as_os_str().to_str());
        let origin = components.next().and_then(|c| c.as_os_str().to_str());
        let name = components.next().and_then(|c| c.as_os_str().to_str());
        let version = components.next().and_then(|c| c.as_os_str().to_str());
        let release = components.next().and_then(|c| c.as_os_str().to_str());

        if let (Some("hab"), Some("pkgs"), Some(origin), Some(name), Some(version), Some(release)) =
            (hab_folder, pkg_folder, origin, name, version, release)
        {
            Some(
                FSRootPath::default().as_ref().join(
                    ["hab", "pkgs", origin, name, version, release]
                        .into_iter()
                        .collect::<PathBuf>(),
                ),
            )
        } else {
            None
        }
    }

    fn relative_package_path(&self) -> Option<PathBuf> {
        self.package_path()
            .map(|p| self.as_ref().strip_prefix(p).unwrap().to_path_buf())
    }

    fn package_ident(&self, target: PackageTarget) -> Option<PackageIdent> {
        let mut components = self.as_ref().components();

        components.next();
        let hab_folder = components.next().and_then(|c| c.as_os_str().to_str());
        let pkg_folder = components.next().and_then(|c| c.as_os_str().to_str());
        let origin = components.next().and_then(|c| c.as_os_str().to_str());
        let name = components.next().and_then(|c| c.as_os_str().to_str());
        let version = components.next().and_then(|c| c.as_os_str().to_str());
        let release = components.next().and_then(|c| c.as_os_str().to_str());

        if let (Some("hab"), Some("pkgs"), Some(origin), Some(name), Some(version), Some(release)) =
            (hab_folder, pkg_folder, origin, name, version, release)
        {
            if let (Ok(origin), Ok(name), Ok(version), Ok(release)) = (
                PackageOrigin::parse(origin),
                PackageName::parse(name),
                PackageResolvedVersion::parse(version),
                PackageResolvedRelease::parse(release),
            ) {
                Some(PackageIdent {
                    origin,
                    name,
                    version,
                    release,
                    target,
                })
            } else {
                None
            }
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageIdent {
    pub name: PackageName,
    pub origin: PackageOrigin,
    pub version: PackageResolvedVersion,
    pub release: PackageResolvedRelease,
    pub target: PackageTarget,
}

impl PackageIdent {
    pub fn satisfies_resolved_dependency(&self, dep_ident: &PackageResolvedDepIdent) -> bool {
        dep_ident.target == self.target
            && dep_ident.origin == self.origin
            && dep_ident.name == self.name
            && match &dep_ident.version {
                PackageVersion::Resolved(resolved_version) => resolved_version == &self.version,
                PackageVersion::Unresolved => true,
            }
            && match &dep_ident.release {
                PackageRelease::Resolved(resolved_release) => resolved_release == &self.release,
                PackageRelease::Unresolved => true,
            }
    }
    pub fn satisfies_dependency(&self, dep_ident: &PackageDepIdent) -> bool {
        dep_ident.origin == self.origin
            && dep_ident.name == self.name
            && match &dep_ident.version {
                PackageVersion::Resolved(resolved_version) => resolved_version == &self.version,
                PackageVersion::Unresolved => true,
            }
            && match &dep_ident.release {
                PackageRelease::Resolved(resolved_release) => resolved_release == &self.release,
                PackageRelease::Unresolved => true,
            }
    }
    pub fn artifact_name(&self) -> String {
        format!(
            "{}-{}-{}-{}-{}.hart",
            self.origin, self.name, self.version, self.release, self.target
        )
    }
}

impl Display for PackageIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{}/{} ({})",
            self.origin, self.name, self.version, self.release, self.target
        )
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageResolvedDepIdent {
    pub name: PackageName,
    pub origin: PackageOrigin,
    pub version: PackageVersion,
    pub release: PackageRelease,
    pub target: PackageTarget,
}

impl PackageResolvedDepIdent {
    pub fn to_ident(&self) -> Option<PackageIdent> {
        if let (PackageVersion::Resolved(version), PackageRelease::Resolved(release)) =
            (&self.version, &self.release)
        {
            Some(PackageIdent {
                origin: self.origin.to_owned(),
                name: self.name.to_owned(),
                version: version.to_owned(),
                release: release.to_owned(),
                target: self.target.to_owned(),
            })
        } else {
            None
        }
    }
    pub fn satisfies_dependency(&self, dep_ident: &PackageDepIdent) -> bool {
        dep_ident.origin == self.origin
            && dep_ident.name == self.name
            && match &dep_ident.version {
                resolved_version @ PackageVersion::Resolved(_) => resolved_version == &self.version,
                PackageVersion::Unresolved => true,
            }
            && match &dep_ident.release {
                resolved_release @ PackageRelease::Resolved(_) => resolved_release == &self.release,
                PackageRelease::Unresolved => true,
            }
    }
}

impl Display for PackageResolvedDepIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.origin, self.name)?;
        if let PackageVersion::Resolved(version) = &self.version {
            write!(f, "/{}", version)?;
            if let PackageRelease::Resolved(release) = &self.release {
                write!(f, "/{}", release)?;
            }
        }
        write!(f, " ({})", self.target)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageBuildIdent {
    pub target: PackageTarget,
    pub origin: PackageOrigin,
    pub name: PackageName,
    pub version: PackageBuildVersion,
}

impl Display for PackageBuildIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{} ({})",
            self.origin, self.name, self.version, self.target
        )
    }
}

impl PackageBuildIdent {
    pub fn satisfies_dependency(&self, dep_ident: &PackageDepIdent) -> bool {
        self.origin == dep_ident.origin
            && self.name == dep_ident.name
            && match (&dep_ident.version, &self.version) {
                (PackageVersion::Resolved(_version), PackageBuildVersion::Dynamic) => false,
                (PackageVersion::Resolved(version), PackageBuildVersion::Static(build_version)) => {
                    version == build_version
                }
                (PackageVersion::Unresolved, _) => true,
            }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct PackageDepIdent {
    pub name: PackageName,
    pub origin: PackageOrigin,
    pub version: PackageVersion,
    pub release: PackageRelease,
}

impl PackageDepIdent {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageDepIdent> {
        let value = value.as_ref();
        let mut parts = value.split('/');
        let origin = PackageOrigin::parse(
            parts
                .next()
                .ok_or_else(|| eyre!("Package origin missing in {}", value))?,
        )?;
        let name = PackageName::parse(
            parts
                .next()
                .ok_or_else(|| eyre!("Package name missing in {}", value))?,
        )?;
        let version = if let Some(version) = parts.next() {
            PackageVersion::Resolved(PackageResolvedVersion::parse(version)?)
        } else {
            PackageVersion::Unresolved
        };
        let release = if let Some(release) = parts.next() {
            PackageRelease::Resolved(PackageResolvedRelease::parse(release)?)
        } else {
            PackageRelease::Unresolved
        };
        if let Some(tail) = parts.next() {
            return Err(eyre!("Package has extra trailing string: {}", tail));
        }
        Ok(PackageDepIdent {
            origin,
            name,
            version,
            release,
        })
    }
}

impl From<&PackageResolvedDepIdent> for PackageDepIdent {
    fn from(value: &PackageResolvedDepIdent) -> Self {
        PackageDepIdent {
            name: value.name.to_owned(),
            origin: value.origin.to_owned(),
            version: value.version.to_owned(),
            release: value.release.to_owned(),
        }
    }
}

impl From<&PackageIdent> for PackageDepIdent {
    fn from(value: &PackageIdent) -> Self {
        PackageDepIdent {
            name: value.name.to_owned(),
            origin: value.origin.to_owned(),
            version: PackageVersion::Resolved(value.version.to_owned()),
            release: PackageRelease::Resolved(value.release.to_owned()),
        }
    }
}

impl From<&PackageBuildIdent> for PackageDepIdent {
    fn from(value: &PackageBuildIdent) -> Self {
        PackageDepIdent {
            name: value.name.to_owned(),
            origin: value.origin.to_owned(),
            version: match &value.version {
                PackageBuildVersion::Static(version) => PackageVersion::Resolved(version.clone()),
                PackageBuildVersion::Dynamic => PackageVersion::Unresolved,
            },
            release: PackageRelease::Unresolved,
        }
    }
}

impl TryFrom<String> for PackageDepIdent {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageDepIdent::parse(value)
    }
}

impl FromStr for PackageDepIdent {
    type Err = color_eyre::eyre::Error;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        PackageDepIdent::parse(value)
    }
}

impl From<PackageDepIdent> for String {
    fn from(value: PackageDepIdent) -> Self {
        value.to_string()
    }
}

impl Display for PackageDepIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.origin, self.name)?;
        if let PackageVersion::Resolved(version) = &self.version {
            write!(f, "/{}", version)?;
            if let PackageRelease::Resolved(release) = &self.release {
                write!(f, "/{}", release)?;
            }
        }
        Ok(())
    }
}

impl PackageDepIdent {
    pub fn to_resolved_dep_ident(&self, target: PackageTarget) -> PackageResolvedDepIdent {
        PackageResolvedDepIdent {
            name: self.name.to_owned(),
            origin: self.origin.to_owned(),
            version: self.version.to_owned(),
            release: self.release.to_owned(),
            target,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(try_from = "String", into = "String")]
pub enum PackageType {
    Native,
    Standard,
}

impl PackageType {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageType> {
        match value.as_ref() {
            "native" => Ok(PackageType::Native),
            "standard" => Ok(PackageType::Standard),
            _ => Err(eyre!("Unknown package type: {}", value.as_ref())),
        }
    }
}

impl TryFrom<String> for PackageType {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageType::parse(value)
    }
}

impl From<PackageType> for String {
    fn from(value: PackageType) -> Self {
        value.to_string()
    }
}

impl Display for PackageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageType::Native => write!(f, "native"),
            PackageType::Standard => write!(f, "standard"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct PackageDepGlob {
    pub origin: String,
    pub name: String,
    pub version: Option<String>,
    pub release: Option<String>,
}

impl PackageDepGlob {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageDepGlob> {
        let value = value.as_ref();
        let mut parts = value.split('/');
        let origin = parts
            .next()
            .map(String::from)
            .ok_or(eyre!("Invalid package glob pattern"))?;
        if origin.is_empty() {
            return Err(eyre!("Package origin pattern is empty"));
        }
        if let Err(err) = Glob::new(&origin) {
            return Err(eyre!("Package origin pattern is invalid: {}", err));
        }
        let name = parts
            .next()
            .map(String::from)
            .ok_or_else(|| eyre!("Package name missing in '{}'", value))?;
        if name.is_empty() {
            return Err(eyre!("Package name pattern is empty"));
        }
        if let Err(err) = Glob::new(&name) {
            return Err(eyre!("Package name pattern is invalid: {}", err));
        }
        let version = parts.next().map(String::from);
        if let Some(version) = &version {
            if version.is_empty() {
                return Err(eyre!("Package version pattern is empty"));
            }
            if let Err(err) = Glob::new(&version) {
                return Err(eyre!("Package version pattern is invalid: {}", err));
            }
        }
        let release = parts.next().map(String::from);
        if let Some(release) = &release {
            if release.is_empty() {
                return Err(eyre!("Package release pattern is empty"));
            }
            if let Err(err) = Glob::new(&release) {
                return Err(eyre!("Package release pattern is invalid: {}", err));
            }
        }
        if let Some(tail) = parts.next() {
            return Err(eyre!(
                "Package glob pattern has extra trailing string: {}",
                tail
            ));
        }
        Ok(PackageDepGlob {
            origin,
            name,
            version,
            release,
        })
    }
    pub fn matcher(&self) -> PackageDepGlobMatcher {
        PackageDepGlobMatcher {
            origin: Glob::new(self.origin.as_str()).unwrap().compile_matcher(),
            name: Glob::new(self.name.as_str()).unwrap().compile_matcher(),
            version: self
                .version
                .as_ref()
                .map(|v| Glob::new(v.as_str()).unwrap())
                .map(|v| v.compile_matcher()),
            release: self
                .release
                .as_ref()
                .map(|v| Glob::new(v.as_str()).unwrap())
                .map(|v| v.compile_matcher()),
        }
    }
}

impl Display for PackageDepGlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.origin, self.name)?;
        if let Some(version) = self.version.as_ref() {
            write!(f, "/{}", version)?;
            if let Some(release) = self.release.as_ref() {
                write!(f, "/{}", release)?;
            }
        }
        Ok(())
    }
}

impl TryFrom<String> for PackageDepGlob {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageDepGlob::parse(value)
    }
}

impl From<PackageDepGlob> for String {
    fn from(value: PackageDepGlob) -> Self {
        value.to_string()
    }
}

impl FromStr for PackageDepGlob {
    type Err = color_eyre::eyre::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        PackageDepGlob::parse(value)
    }
}

#[derive(Debug)]
pub struct PackageDepGlobMatcher {
    pub origin: GlobMatcher,
    pub name: GlobMatcher,
    pub version: Option<GlobMatcher>,
    pub release: Option<GlobMatcher>,
}

impl PackageDepGlobMatcher {
    pub fn matches_package_ident(&self, ident: &PackageIdent) -> bool {
        self.origin.is_match(&ident.origin.0)
            && self.name.is_match(&ident.name.0)
            && if let Some(version) = self.version.as_ref() {
                version.is_match(&ident.version.0)
            } else {
                true
            }
            && if let Some(release) = self.release.as_ref() {
                release.is_match(&ident.release.0)
            } else {
                true
            }
    }
    pub fn matches_package_resolved_dep_ident(
        &self,
        resolved_dep_ident: &PackageResolvedDepIdent,
    ) -> bool {
        self.origin.is_match(&resolved_dep_ident.origin.0)
            && self.name.is_match(&resolved_dep_ident.name.0)
            && if let Some(version) = self.version.as_ref() {
                match resolved_dep_ident.version {
                    PackageVersion::Resolved(ref resolved_version) => {
                        version.is_match(&resolved_version.0)
                    }
                    PackageVersion::Unresolved => true,
                }
            } else {
                true
            }
            && if let Some(release) = self.release.as_ref() {
                match resolved_dep_ident.release {
                    PackageRelease::Resolved(ref resolved_release) => {
                        release.is_match(&resolved_release.0)
                    }
                    PackageRelease::Unresolved => true,
                }
            } else {
                true
            }
    }
    pub fn matches_package_dep_ident(&self, dep_ident: &PackageDepIdent) -> bool {
        self.origin.is_match(&dep_ident.origin.0)
            && self.name.is_match(&dep_ident.name.0)
            && if let Some(version) = self.version.as_ref() {
                match dep_ident.version {
                    PackageVersion::Resolved(ref resolved_version) => {
                        version.is_match(&resolved_version.0)
                    }
                    PackageVersion::Unresolved => true,
                }
            } else {
                true
            }
            && if let Some(release) = self.release.as_ref() {
                match dep_ident.release {
                    PackageRelease::Resolved(ref resolved_release) => {
                        release.is_match(&resolved_release.0)
                    }
                    PackageRelease::Unresolved => true,
                }
            } else {
                true
            }
    }
    pub fn matches_package_build_ident(&self, build_ident: &PackageBuildIdent) -> bool {
        self.origin.is_match(&build_ident.origin.0)
            && self.name.is_match(&build_ident.name.0)
            && if let Some(version) = self.version.as_ref() {
                match &build_ident.version {
                    PackageBuildVersion::Static(resolved_version) => {
                        version.is_match(&resolved_version.0)
                    }
                    PackageBuildVersion::Dynamic => version.is_match(DYNAMIC_VERSION),
                }
            } else {
                true
            }
            && if let Some(release) = self.release.as_ref() {
                release.is_match(BUILD_RELEASE)
            } else {
                true
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_dep_ident_parsing() {
        let valid_cases = &[
            "core/hab",
            "core/hab/1.0",
            "core/hab/1.0/2032",
            "core/hab/**DYNAMIC**",
        ];
        let invalid_cases = &[
            "core",
            "core\\hab",
            "core/hab/1.0/2034/test",
            "core/",
            "core///",
            "core/hab/",
            "core/hab//",
            "core//1.0/2034",
            "core///2034",
            "core/hab//2034",
            "/hab/1.0/2034",
            "//1.0/2034",
            "///2034",
            "///",
            "core&/hab/1.0",
        ];
        for item in valid_cases {
            assert!(PackageDepIdent::parse(item).is_ok());
        }
        for item in invalid_cases {
            assert!(PackageDepIdent::parse(item).is_err());
        }
    }

    #[test]
    fn package_dep_glob_parsing() {
        let valid_cases = &[
            "core/hab",
            "core/hab/1.0",
            "core/hab/1.0/2032",
            "core/hab/**DYNAMIC**",
            "core&/hab/1.0",
        ];
        let invalid_cases = &[
            "core",
            "core\\hab",
            "core/hab/1.0/2034/test",
            "core/",
            "core///",
            "core/hab/",
            "core/hab//",
            "core//1.0/2034",
            "core///2034",
            "core/hab//2034",
            "/hab/1.0/2034",
            "//1.0/2034",
            "///2034",
            "///",
        ];
        for item in valid_cases {
            assert!(PackageDepGlob::parse(item).is_ok());
        }
        for item in invalid_cases {
            assert!(PackageDepGlob::parse(item).is_err());
        }
    }

    #[test]
    fn dynamic_build_ident_satisfies_package_dep_ident() {
        let dynamic_ident = PackageBuildIdent {
            target: PackageTarget::default(),
            origin: PackageOrigin::parse("core").unwrap(),
            name: PackageName::parse("hab").unwrap(),
            version: PackageBuildVersion::Dynamic,
        };

        let satisfed_dep_idents = &["core/hab"];
        let unsatisfied_dep_idents = &["core/hab/1.0", "core/hab/1.0/2020", "core/hab-studio"];

        for dep_ident in satisfed_dep_idents {
            let dep_ident = PackageDepIdent::parse(dep_ident).unwrap();
            assert_eq!(dynamic_ident.satisfies_dependency(&dep_ident), true);
        }
        for dep_ident in unsatisfied_dep_idents {
            let dep_ident = PackageDepIdent::parse(dep_ident).unwrap();
            assert_eq!(dynamic_ident.satisfies_dependency(&dep_ident), false);
        }
    }

    #[test]
    fn dynamic_build_ident_satisfies_package_dep_glob() {
        let dynamic_ident = PackageBuildIdent {
            target: PackageTarget::default(),
            origin: PackageOrigin::parse("core").unwrap(),
            name: PackageName::parse("hab").unwrap(),
            version: PackageBuildVersion::Dynamic,
        };

        let satisfed_dep_globs = &["core/hab", "core/hab/*", "core/hab/*/*"];
        let unsatisfied_dep_globs = &[
            "core/hab/1.0",
            "core/hab/1.0/2020",
            "core/hab-studio",
            "core/hab/1.0/*",
            "core/hab/*/2020",
        ];

        for dep_glob in satisfed_dep_globs {
            let dep_glob = PackageDepGlob::parse(dep_glob).unwrap().matcher();
            assert_eq!(dep_glob.matches_package_build_ident(&dynamic_ident), true);
        }
        for dep_glob in unsatisfied_dep_globs {
            let dep_glob = PackageDepGlob::parse(dep_glob).unwrap().matcher();
            assert_eq!(dep_glob.matches_package_build_ident(&dynamic_ident), false);
        }
    }
}
