// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::io::{Directory, DirentKind};
use anyhow::{anyhow, bail, Result};
use std::path::{Component, PathBuf};
use std::str::FromStr;
use thiserror::Error;

use flex_fuchsia_sys2 as fsys;

/// Separator for user input for parsing command line arguments to structs
/// in this crate.
const REMOTE_PATH_SEPARATOR: &'static str = "::";

pub const REMOTE_COMPONENT_STORAGE_PATH_HELP: &'static str = r#"Remote storage paths allow the following formats:
1)  [instance ID]::[path relative to storage]
    Example: "c1a6d0aebbf7c092c53e8e696636af8ec0629ff39b7f2e548430b0034d809da4::/path/to/file"

    `..` is not valid anywhere in the remote path.

    To learn about component instance IDs, see https://fuchsia.dev/go/components/instance-id"#;

pub const REMOTE_DIRECTORY_PATH_HELP: &'static str = r#"Remote directory paths must be:
1)  [moniker]::[path in namespace]
    Example: /foo/bar::/config/data/sample.json

    To learn more about monikers, see https://fuchsia.dev/go/components/moniker#absolute

2)  [moniker]::[dir type]::[path] where [dir type] is one of "in", "out", or "pkg", specifying
    the component's namespace directory, outgoing directory, or package directory (if packaged).
"#;

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteDirectoryPath {
    pub moniker: String,
    pub dir_type: fsys::OpenDirType,
    pub relative_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteComponentStoragePath {
    pub instance_id: String,
    pub relative_path: PathBuf,
}

#[derive(Error, Debug, PartialEq)]
pub enum ParsePathError {
    #[error("Unsupported directory type: {dir_type}. {}", REMOTE_DIRECTORY_PATH_HELP)]
    UnsupportedDirectory { dir_type: String },

    #[error("Disallowed path component: {component}. {}", REMOTE_DIRECTORY_PATH_HELP)]
    DisallowedPathComponent { component: String },

    #[error("Malformatted remote directory path. {}", REMOTE_DIRECTORY_PATH_HELP)]
    InvalidFormat,
}

impl FromStr for RemoteDirectoryPath {
    type Err = ParsePathError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = input.split(REMOTE_PATH_SEPARATOR).collect();
        if parts.len() < 2 || parts.len() > 3 {
            return Err(ParsePathError::InvalidFormat);
        }

        // TODO(https://fxbug.dev/42077346): Use common Moniker parsing logic instead of String.
        let moniker = parts.first().unwrap().to_string();
        let (dir_type, path_str) = if parts.len() == 3 {
            let parsed = parse_dir_type_from_str(parts[1])?;
            (parsed, parts[2])
        } else {
            (fsys::OpenDirType::NamespaceDir, parts[1])
        };
        let path = PathBuf::from(path_str);

        // Perform checks on path that ignore `.`  and disallow `..`, `/` or Windows path prefixes such as C: or \\
        let mut normalized_path = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(c) => normalized_path.push(c),
                Component::RootDir => continue,
                Component::CurDir => continue,
                c => {
                    return Err(ParsePathError::DisallowedPathComponent {
                        component: format!("{:?}", c),
                    })
                }
            }
        }

        Ok(Self { moniker, dir_type, relative_path: normalized_path })
    }
}

fn parse_dir_type_from_str(s: &str) -> Result<fsys::OpenDirType, ParsePathError> {
    // Only match on either the namespace (in), outgoing (out) or package (pkg) directories.
    // The parser could be expected to support others, if the need arises.
    match s {
        "in" | "namespace" => Ok(fsys::OpenDirType::NamespaceDir),
        "out" => Ok(fsys::OpenDirType::OutgoingDir),
        "pkg" => Ok(fsys::OpenDirType::PackageDir),
        _ => Err(ParsePathError::UnsupportedDirectory { dir_type: s.into() }),
    }
}

fn dir_type_to_str(dir_type: &fsys::OpenDirType) -> Result<&str> {
    // Only match on either the namespace (in), outgoing (out) or package (pkg) directories.
    // The parser could be expected to support others, if the need arises.
    match dir_type {
        fsys::OpenDirType::NamespaceDir => Ok("in"),
        fsys::OpenDirType::OutgoingDir => Ok("out"),
        fsys::OpenDirType::PackageDir => Ok("pkg"),
        _ => Err(anyhow!("Unsupported OpenDirType: {:?}", dir_type)),
    }
}

/// Represents a path to a file/directory within a component's storage.
impl RemoteComponentStoragePath {
    pub fn parse(input: &str) -> Result<Self> {
        match input.split_once(REMOTE_PATH_SEPARATOR) {
            Some((first, second)) => {
                if second.contains(REMOTE_PATH_SEPARATOR) {
                    bail!(
                        "Remote storage path must contain exactly one `{}` separator. {}",
                        REMOTE_PATH_SEPARATOR,
                        REMOTE_COMPONENT_STORAGE_PATH_HELP
                    )
                }

                let instance_id = first.to_string();
                let relative_path = PathBuf::from(second);

                // Perform checks on path that ignore `.`  and disallow `..`, `/` or Windows path prefixes such as C: or \\
                let mut normalized_relative_path = PathBuf::new();
                for component in relative_path.components() {
                    match component {
                        Component::Normal(c) => normalized_relative_path.push(c),
                        Component::RootDir => continue,
                        Component::CurDir => continue,
                        c => bail!(
                            "Unsupported path component: {:?}. {}",
                            c,
                            REMOTE_COMPONENT_STORAGE_PATH_HELP
                        ),
                    }
                }

                Ok(Self { instance_id, relative_path: normalized_relative_path })
            }
            None => {
                bail!(
                    "Remote storage path must contain exactly one `{}` separator. {}",
                    REMOTE_PATH_SEPARATOR,
                    REMOTE_COMPONENT_STORAGE_PATH_HELP
                )
            }
        }
    }

    pub fn contains_wildcard(&self) -> bool {
        return self.to_string().contains("*");
    }

    pub fn relative_path_string(&self) -> String {
        return self.relative_path.to_string_lossy().to_string();
    }
}

impl ToString for RemoteComponentStoragePath {
    fn to_string(&self) -> String {
        format!(
            "{}{sep}/{}",
            self.instance_id,
            self.relative_path.to_string_lossy(),
            sep = REMOTE_PATH_SEPARATOR
        )
    }
}

impl ToString for RemoteDirectoryPath {
    fn to_string(&self) -> String {
        format!(
            "{}{sep}{}{sep}/{}",
            self.moniker,
            dir_type_to_str(&self.dir_type).unwrap(),
            self.relative_path.to_string_lossy(),
            sep = REMOTE_PATH_SEPARATOR
        )
    }
}

#[derive(Clone)]
/// Represents either a local path to a file or directory, or the path to a file/directory
/// in a directory associated with a remote component.
pub enum LocalOrRemoteDirectoryPath {
    Local(PathBuf),
    Remote(RemoteDirectoryPath),
}

impl LocalOrRemoteDirectoryPath {
    pub fn parse(path: &str) -> LocalOrRemoteDirectoryPath {
        match RemoteDirectoryPath::from_str(path) {
            Ok(path) => LocalOrRemoteDirectoryPath::Remote(path),
            // If we can't parse a remote path, then it is a host path.
            Err(_) => LocalOrRemoteDirectoryPath::Local(PathBuf::from(path)),
        }
    }
}

#[derive(Clone)]
/// Represents either a local path to a file or directory, or the path to a file/directory
/// in a directory associated with a remote component.
pub enum LocalOrRemoteComponentStoragePath {
    Local(PathBuf),
    Remote(RemoteComponentStoragePath),
}

impl LocalOrRemoteComponentStoragePath {
    pub fn parse(path: &str) -> LocalOrRemoteComponentStoragePath {
        match RemoteComponentStoragePath::parse(path) {
            Ok(path) => LocalOrRemoteComponentStoragePath::Remote(path),
            // If we can't parse a remote path, then it is a host path.
            Err(_) => LocalOrRemoteComponentStoragePath::Local(PathBuf::from(path)),
        }
    }
}

/// Returns a readable `Directory` by opening the parent dir of `path`.
///
/// * `path`: The path from which to derive the parent
/// * `dir`: RemoteDirectory to on which to open a subdir.
pub fn open_parent_subdir_readable<D: Directory>(path: &PathBuf, dir: &D) -> Result<D> {
    if path.components().count() < 2 {
        // The path is something like "/foo" which, as a relative path from `dir`, would make `dir`
        // the parent.
        return dir.clone();
    }

    dir.open_dir_readonly(path.parent().unwrap())
}

/// If `destination_path` in `destination_dir` is itself a directory, returns
/// a path with the filename portion of `source_path` appended. Otherwise, returns
/// a copy of the input `destination_path`.
///
/// The purpose of this function is to help infer a path in cases which an ending file name for the destination path is not provided.
/// For example, the command "ffx component storage copy ~/alarm.wav [instance-id]::/" does not know what name to give the new file copied.
/// [instance-id]::/. Thus it is necessary to infer this new file name and generate the new path "[instance-id]::/alarm.wav".
///
/// # Arguments
///
/// * `destination_dir`: Directory to query for the type of `destination_path`
/// * `source_path`: path from which to read a filename, if needed
/// * `destination_path`: destination path
///
/// # Error Conditions:
///
/// * File name for `source_path` is empty
/// * Communications error talking to remote endpoint
pub async fn add_source_filename_to_path_if_absent<D: Directory>(
    destination_dir: &D,
    source_path: &PathBuf,
    destination_path: &PathBuf,
) -> Result<PathBuf> {
    let source_file = source_path
        .file_name()
        .map_or_else(|| Err(anyhow!("Source path is empty")), |file| Ok(PathBuf::from(file)))?;
    let source_file_str = source_file.display().to_string();

    // If the destination is a directory, append `source_file_str`.
    if let Some(destination_file) = destination_path.file_name() {
        let parent_dir = open_parent_subdir_readable(destination_path, destination_dir)?;
        match parent_dir.entry_type(destination_file.to_string_lossy().as_ref()).await? {
            Some(DirentKind::File) | None => Ok(destination_path.clone()),
            Some(DirentKind::Directory) => Ok(destination_path.join(source_file_str)),
        }
    } else {
        Ok(destination_path.join(source_file_str))
    }
}

#[cfg(test)]
mod tests {
    use crate::path::{dir_type_to_str, parse_dir_type_from_str, RemoteDirectoryPath};
    use flex_fuchsia_sys2 as fsys;
    use std::str::FromStr;

    #[test]
    fn test_parse_dir_type_from_str() {
        assert_eq!(parse_dir_type_from_str("in"), Ok(fsys::OpenDirType::NamespaceDir));
        assert_eq!(parse_dir_type_from_str("namespace"), Ok(fsys::OpenDirType::NamespaceDir));
        assert_eq!(parse_dir_type_from_str("out"), Ok(fsys::OpenDirType::OutgoingDir));
        assert_eq!(parse_dir_type_from_str("pkg"), Ok(fsys::OpenDirType::PackageDir));
        assert!(parse_dir_type_from_str("nonexistent").is_err());
    }

    #[test]
    fn test_dir_type_to_str() {
        assert_eq!(dir_type_to_str(&fsys::OpenDirType::NamespaceDir).unwrap(), "in");
        assert_eq!(dir_type_to_str(&fsys::OpenDirType::OutgoingDir).unwrap(), "out");
        assert_eq!(dir_type_to_str(&fsys::OpenDirType::PackageDir).unwrap(), "pkg");
        assert!(dir_type_to_str(&fsys::OpenDirType::RuntimeDir).is_err());
        assert!(dir_type_to_str(&fsys::OpenDirType::ExposedDir).is_err());
    }

    #[test]
    fn test_remote_directory_path_from_str() {
        assert_eq!(
            RemoteDirectoryPath::from_str("/foo/bar::/path"),
            Ok(RemoteDirectoryPath {
                moniker: "/foo/bar".into(),
                dir_type: fsys::OpenDirType::NamespaceDir,
                relative_path: "path".into(),
            })
        );

        assert_eq!(
            RemoteDirectoryPath::from_str("/foo/bar::out::/path"),
            Ok(RemoteDirectoryPath {
                moniker: "/foo/bar".into(),
                dir_type: fsys::OpenDirType::OutgoingDir,
                relative_path: "path".into(),
            })
        );

        assert_eq!(
            RemoteDirectoryPath::from_str("/foo/bar::pkg::/path"),
            Ok(RemoteDirectoryPath {
                moniker: "/foo/bar".into(),
                dir_type: fsys::OpenDirType::PackageDir,
                relative_path: "path".into(),
            })
        );

        assert!(RemoteDirectoryPath::from_str("/foo/bar").is_err());
        assert!(RemoteDirectoryPath::from_str("/foo/bar::one::two::three").is_err());
        assert!(RemoteDirectoryPath::from_str("/foo/bar::not_a_dir::three").is_err());
    }
}
