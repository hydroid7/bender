// Copyright (c) 2017-2018 ETH Zurich
// Fabian Schuiki <fschuiki@iis.ee.ethz.ch>

//! Package manifest and configuration files.
//!
//! This module provides reading and writing of package manifests and
//! configuration files.

#![deny(missing_docs)]

use std;
use std::str::FromStr;
use std::path::PathBuf;
use std::hash::Hash;
use std::collections::{HashMap, HashSet};
use semver;
use error::*;
use util::*;

/// A package manifest.
///
/// This is usually called `Bender.yml` in the root directory of the package.
#[derive(Debug)]
pub struct Manifest {
    /// The package definition.
    pub package: Package,
    /// The dependencies.
    pub dependencies: HashMap<String, Dependency>,
}

/// A package definition.
///
/// Contains the metadata for an individual package.
#[derive(Serialize, Deserialize, Debug)]
pub struct Package {
    /// The name of the package.
    pub name: String,
    /// A list of package authors. Each author should be of the form `John Doe
    /// <john@doe.com>`.
    pub authors: Option<Vec<String>>,
}

/// A dependency.
///
/// The name of the dependency is given implicitly by the key in the hash map
/// that this `Dependency` is accessible through.
#[derive(Debug)]
pub enum Dependency {
    /// A dependency that can be found in one of the package repositories.
    Version(semver::VersionReq),
    /// A local path dependency. The exact version of the dependency found at
    /// the given path will be used, regardless of any actual versioning
    /// constraints.
    Path(PathBuf),
    /// A git dependency specified by a revision.
    GitRevision(String, String),
    /// A git dependency specified by a version requirement. Works similarly to
    /// the `GitRevision`, but extracts all tags of the form `v.*` from the
    /// repository and matches the version against that.
    GitVersion(String, semver::VersionReq),
}

/// Converts partial configuration into a validated full configuration.
pub trait Validate {
    /// The output type produced by validation.
    type Output;
    /// The error type produced by validation.
    type Error;
    /// Validate self and convert into the non-partial version.
    fn validate(self) -> std::result::Result<Self::Output, Self::Error>;
}

// Implement `Validate` for hash maps of validatable values.
impl<K,V> Validate for HashMap<K,V> where K: Hash + Eq, V: Validate<Error=Error> {
    type Output = HashMap<K, V::Output>;
    type Error = (K,Error);
    fn validate(self) -> std::result::Result<Self::Output, Self::Error> {
        self.into_iter().map(|(k,v)| match v.validate() {
            Ok(v) => Ok((k,v)),
            Err(e) => Err((k,e)),
        }).collect()
    }
}

// Implement `Validate` for `StringOrStruct` wrapped validatable values.
impl<T> Validate for StringOrStruct<T> where T: Validate {
    type Output = T::Output;
    type Error = T::Error;
    fn validate(self) -> std::result::Result<T::Output, T::Error> {
        self.0.validate()
    }
}

/// A partial manifest.
///
/// Validation turns this into a `Manifest`.
#[derive(Serialize, Deserialize, Debug)]
pub struct PartialManifest {
    /// The package definition.
    pub package: Option<Package>,
    /// The dependencies.
    pub dependencies: Option<HashMap<String, StringOrStruct<PartialDependency>>>,
}

impl Validate for PartialManifest {
    type Output = Manifest;
    type Error = Error;
    fn validate(self) -> Result<Manifest> {
        let pkg = match self.package {
            Some(p) => p,
            None => return Err(Error::new("Missing package information."))
        };
        let deps = match self.dependencies {
            Some(d) => d.validate().map_err(|(key,cause)| Error::chain(
                format!("In dependency `{}` of package `{}`:", key, pkg.name),
                cause
            ))?,
            None => HashMap::new(),
        };
        Ok(Manifest {
            package: pkg,
            dependencies: deps,
        })
    }
}

/// A partial dependency.
///
/// Contains all the necessary information to resolve and find a dependency.
/// The following combinations of fields are valid:
///
/// - `version`
/// - `path`
/// - `git,rev`
/// - `git,version`
///
/// Can be validated into a `Dependency`.
#[derive(Serialize, Deserialize, Debug)]
pub struct PartialDependency {
    /// The path to the package.
    path: Option<String>,
    /// The git URL to the package.
    git: Option<String>,
    /// The git revision of the package to use. Can be a commit hash, branch,
    /// tag, or similar.
    rev: Option<String>,
    /// The version requirement of the package. This will be parsed into a
    /// semantic versioning requirement.
    version: Option<String>,
}

impl FromStr for PartialDependency {
    type Err = Void;
    fn from_str(s: &str) -> std::result::Result<Self, Void> {
        Ok(PartialDependency {
            path: None,
            git: None,
            rev: None,
            version: Some(s.into()),
        })
    }
}

impl Validate for PartialDependency {
    type Output = Dependency;
    type Error = Error;
    fn validate(self) -> Result<Dependency> {
        let version = match self.version {
            Some(v) => Some(semver::VersionReq::parse(&v).map_err(|cause| Error::chain(
                format!("\"{}\" is not a valid semantic version requirement.", v),
                cause
            ))?),
            None => None,
        };
        if self.rev.is_some() && version.is_some() {
            return Err(Error::new("A dependency cannot specify `version` and `rev` at the same time."));
        }
        if let Some(path) = self.path {
            if let Some(list) = string_list(
                self.git.map(|_| "`git`").iter()
                .chain(self.rev.map(|_| "`rev`").iter())
                .chain(version.map(|_| "`version`").iter()),
                ",", "or"
            ) {
                Err(Error::new(format!("A `path` dependency cannot have a {} field.", list)))
            } else {
                Ok(Dependency::Path(path.into()))
            }
        } else if let Some(git) = self.git {
            if let Some(rev) = self.rev {
                Ok(Dependency::GitRevision(git, rev))
            } else if let Some(version) = version {
                Ok(Dependency::GitVersion(git, version))
            } else {
                Err(Error::new("A `git` dependency must have either a `rev` or `version` field."))
            }
        } else if let Some(version) = version {
            Ok(Dependency::Version(version))
        } else {
            Err(Error::new("A dependency must specify `version`, `path`, or `git`."))
        }
    }
}

/// Merges missing information from another struct.
pub trait Merge {
    /// Populate missing fields from `other`.
    fn merge(self, other: Self) -> Self;
}

/// A configuration.
///
/// This struct encapsulates every setting of the tool that can be changed by
/// the user by some means. It is constructed from a partial configuration.
#[derive(Debug)]
pub struct Config {
    /// The path to the database directory.
    pub database: PathBuf,
    /// The git command or path to the binary.
    pub git: String,
}

/// A partial configuration.
#[derive(Serialize, Deserialize, Debug)]
pub struct PartialConfig {
    /// The path to the database directory.
    pub database: Option<PathBuf>,
    /// The git command or path to the binary.
    pub git: Option<String>,
}

impl PartialConfig {
    /// Create a new empty configuration.
    pub fn new() -> PartialConfig {
        PartialConfig {
            database: None,
            git: None,
        }
    }
}

impl Merge for PartialConfig {
    fn merge(self, other: PartialConfig) -> PartialConfig {
        PartialConfig {
            database: self.database.or(other.database),
            git: self.git.or(other.git),
        }
    }
}

impl Validate for PartialConfig {
    type Output = Config;
    type Error = Error;
    fn validate(self) -> Result<Config> {
        Ok(Config {
            database: match self.database {
                Some(db) => db,
                None => return Err(Error::new("Database directory not configured")),
            },
            git: match self.git {
                Some(git) => git,
                None => return Err(Error::new("Git command or path to binary not configured")),
            },
        })
    }
}

/// A lock file.
///
/// This struct encapsulates the result of dependency resolution. For every
/// dependency in the package it lists the exact source and version.
#[derive(Serialize, Deserialize, Debug)]
pub struct Locked {
    /// The locked package versions.
    pub packages: HashMap<String, LockedPackage>,
}

/// A locked dependency.
///
/// Encapsualtes the exact source and version of a dependency.
#[derive(Serialize, Deserialize, Debug)]
pub struct LockedPackage {
    /// The revision hash of the dependency.
    pub revision: Option<String>,
    /// The version of the dependency.
    pub version: Option<String>,
    /// The source of the dependency.
    pub source: LockedSource,
    /// Other packages this package depends on.
    pub dependencies: HashSet<String>,
}

/// A source description for a locked dependency.
#[derive(Serialize, Deserialize, Debug)]
pub enum LockedSource {
    /// A path on the system.
    Path(PathBuf),
    /// A git URL.
    Git(String),
    /// A registry.
    Registry(String),
}
