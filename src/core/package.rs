use std::{fmt::Display, path::Path, str::FromStr};

use globset::{GlobMatcher, Glob};
use serde::{Deserialize, Serialize};

use color_eyre::{
    eyre::{eyre, Result},
    Help,
};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref IDENTIFIER_REGEX: Regex = Regex::new("^[A-Za-z0-9_-]+$").unwrap();
}
const DYNAMIC_VERSION: &str = "**DYNAMIC**";

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageName(String);

impl PackageName {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageName> {
        let value = value.as_ref();
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
#[serde(try_from = "String", into = "String")]
pub enum PackageResolvedVersion {
    Static(String),
    Dynamic,
}

impl PackageResolvedVersion {
    pub fn parse(value: impl AsRef<str>) -> Result<PackageResolvedVersion> {
        let value = value.as_ref();
        if value == DYNAMIC_VERSION {
            Ok(PackageResolvedVersion::Dynamic)
        } else {
            Ok(PackageResolvedVersion::Static(value.to_string()))
        }
    }
}

impl Display for PackageResolvedVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageResolvedVersion::Static(version) => write!(f, "{}", version),
            PackageResolvedVersion::Dynamic => write!(f, "{}", DYNAMIC_VERSION),
        }
    }
}

impl TryFrom<String> for PackageResolvedVersion {
    type Error = color_eyre::eyre::Error;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        PackageResolvedVersion::parse(value)
    }
}

impl From<PackageResolvedVersion> for String {
    fn from(value: PackageResolvedVersion) -> Self {
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
            && match (&dep_ident.version, &self.version) {
                (
                    PackageVersion::Resolved(resolved_version),
                    PackageResolvedVersion::Static(self_version),
                ) => match resolved_version {
                    PackageResolvedVersion::Static(resolved_version) => {
                        resolved_version == self_version
                    }
                    PackageResolvedVersion::Dynamic => false,
                },
                (PackageVersion::Resolved(_resolved_version), PackageResolvedVersion::Dynamic) => {
                    panic!("Package ident should not have a dynamic version")
                }
                (PackageVersion::Unresolved, _) => true,
            }
            && match (&dep_ident.release, &self.release) {
                (PackageRelease::Resolved(resolved_release), self_release) => {
                    resolved_release == self_release
                }
                (PackageRelease::Unresolved, _) => true,
            }
    }
    pub fn satisfies_dependency(&self, dep_ident: &PackageDepIdent) -> bool {
        dep_ident.origin == self.origin
            && dep_ident.name == self.name
            && match (&dep_ident.version, &self.version) {
                (
                    PackageVersion::Resolved(resolved_version),
                    PackageResolvedVersion::Static(self_version),
                ) => match resolved_version {
                    PackageResolvedVersion::Static(resolved_version) => {
                        resolved_version == self_version
                    }
                    PackageResolvedVersion::Dynamic => false,
                },
                (PackageVersion::Resolved(_resolved_version), PackageResolvedVersion::Dynamic) => {
                    panic!("Package ident should not have a dynamic version")
                }
                (PackageVersion::Unresolved, _) => true,
            }
            && match (&dep_ident.release, &self.release) {
                (PackageRelease::Resolved(resolved_release), self_release) => {
                    resolved_release == self_release
                }
                (PackageRelease::Unresolved, _) => true,
            }
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
            && match (&dep_ident.version, &self.version) {
                (PackageVersion::Resolved(version), PackageVersion::Resolved(self_version)) => {
                    version == self_version
                }
                (PackageVersion::Resolved(_), PackageVersion::Unresolved) => false,
                (PackageVersion::Unresolved, _) => true,
            }
            && match (&dep_ident.release, &self.release) {
                (PackageRelease::Resolved(release), PackageRelease::Resolved(self_release)) => {
                    release == self_release
                }
                (PackageRelease::Resolved(_), PackageRelease::Unresolved) => false,
                (PackageRelease::Unresolved, _) => true,
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
    pub version: PackageResolvedVersion,
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
            && match &dep_ident.version {
                PackageVersion::Resolved(version) => self.version == *version,
                PackageVersion::Unresolved => true,
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
            version: PackageVersion::Resolved(value.version.to_owned()),
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
    pub name: Option<String>,
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
        let name = parts.next().map(String::from);
        let version = parts.next().map(String::from);
        let release = parts.next().map(String::from);
        Ok(PackageDepGlob {
            origin,
            name,
            version,
            release,
        })
    }
    pub fn matcher(&self) -> Result<PackageDepGlobMatcher> {
        Ok(PackageDepGlobMatcher {
            origin: Glob::new(self.origin.as_str())?.compile_matcher(),
            name: self.name.as_ref().map(|v| Glob::new(v.as_str())).transpose()?.map(|v| v.compile_matcher()),
            version: self.version.as_ref().map(|v| Glob::new(v.as_str())).transpose()?.map(|v| v.compile_matcher()),
            release: self.release.as_ref().map(|v| Glob::new(v.as_str())).transpose()?.map(|v| v.compile_matcher())
        })
    }
}

impl Display for PackageDepGlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.origin)?;
        if let Some(name) = self.name.as_ref() {
            write!(f, "/{}", name)?;
            if let Some(version) = self.version.as_ref() {
                write!(f, "/{}", version)?;
                if let Some(release) = self.release.as_ref() {
                    write!(f, "/{}", release)?;
                }
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
    pub name: Option<GlobMatcher>,
    pub version: Option<GlobMatcher>,
    pub release: Option<GlobMatcher>,
}

impl PackageDepGlobMatcher {
    pub fn is_match(&self, dep_ident: &PackageDepIdent) -> bool {
        self.origin.is_match(&dep_ident.origin.0)
            && if let Some(name) = self.name.as_ref() {
                name.is_match(&dep_ident.name.0)
            } else {
                true
            }
            && if let Some(version) = self.version.as_ref() {
                match dep_ident.version {
                    PackageVersion::Resolved(ref resolved_version) => match resolved_version {
                        PackageResolvedVersion::Static(ref resolved_version) => {
                            version.is_match(resolved_version)
                        }
                        PackageResolvedVersion::Dynamic => true,
                    },
                    PackageVersion::Unresolved => false,
                }
            } else {
                true
            }
            && if let Some(release) = self.release.as_ref() {
                match dep_ident.release {
                    PackageRelease::Resolved(ref resolved_release) => {
                        release.is_match(&resolved_release.0)
                    }
                    PackageRelease::Unresolved => false,
                }
            } else {
                true
            }
    }
}
