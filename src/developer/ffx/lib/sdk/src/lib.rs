// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors as _;

use log::warn;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Command;
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SDK directory could not be found.")]
    NotFound,

    #[error("Could not resolve working directory while searching for the Fuchsia SDK: {0}")]
    ResolveCwd(#[source] std::io::Error),

    #[error("FFX was run from an invalid working directory: {0}")]
    InvalidCwd(#[source] std::io::Error),

    #[error("FFX Binary doesn't exist in the file system: {0}")]
    NoBinary(#[source] std::io::Error),

    #[error("SDK path `{0}` was invalid and couldn't be canonicalized: {1}")]
    InvalidPath(PathBuf, #[source] std::io::Error),

    #[error("Failed to open SDK manifest path at `{0}`: {1}")]
    OpenManifest(PathBuf, #[source] std::io::Error),

    #[error("Failed to parse SDK manifest file at `{0}`: {1}")]
    ParseManifest(PathBuf, #[source] serde_json::Error),

    #[error("No path found for {0}")]
    NoPathFound(String),

    #[error("No executable provided for tool '{0}'")]
    NoExecutable(String),

    #[error("SDK File '{0}' has no source in the build directory")]
    NoSource(String),

    #[error("SDK not found. {help}\nOriginal error: {source}")]
    NotFoundWithHelp { help: String, source: Box<SdkError> },

    #[error("Failed to load SDK manifest at `{path}`: {source}")]
    ManifestLoad { path: PathBuf, source: Box<SdkError> },

    #[error("SDK root `{0}` does not contain an SDK manifest.")]
    MissingManifest(PathBuf),

    #[error("Failed to load host tools from `{path}`: {source}")]
    HostToolsLoad { path: PathBuf, source: Box<SdkError> },
}

use metadata::{CpuArchitecture, ElementType, FfxTool, HostTool, Manifest, Part};
pub use sdk_metadata as metadata;

const SDK_MANIFEST_PATH: &str = "meta/manifest.json";

/// Current "F" milestone for Fuchsia (e.g. F38).
const MILESTONE: &'static str = include_str!("../../../../../../integration/MILESTONE");

const SDK_NOT_FOUND_HELP: &str = "\
SDK directory could not be found. Please set with
`ffx sdk set root <PATH_TO_SDK_DIR>`\n
If you are developing in the fuchsia tree, ensure \
that you are running the `ffx` command (in $FUCHSIA_DIR/.jiri_root) or `fx ffx`, not a built binary.
Running the binary directly is not supported in the fuchsia tree.\n\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdkVersion {
    Version(String),
    InTree,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Sdk {
    path_prefix: PathBuf,
    module: Option<String>,
    parts: Vec<Part>,
    real_paths: Option<HashMap<String, String>>,
    version: SdkVersion,
}

#[derive(Debug)]
pub struct FfxToolFiles {
    /// How "specific" this definition is, in terms of how many of the
    /// relevant paths came from arch specific definitions:
    /// - 0: Platform independent, no arch specific paths.
    /// - 1: One of the paths came from an arch specific section.
    /// - 2: Both the paths came from arch specific sections.
    /// This allows for easy sorting of tool files by how specific
    /// they are.
    pub specificity_score: usize,
    /// The actual executable binary to run
    pub executable: PathBuf,
    /// The path to the FHO metadata file
    pub metadata: PathBuf,
}

/// The SDKRoot is the path that is the root directory for the relative paths contained in
/// the SDK manifest. The SDK manifest defines the contents of the SDK.
/// There are two common use cases for the SdkRoot.
///
/// The first is the "out-of-tree" use case, this
/// is where the IDK and optionally additional files, are downloaded as part of a source code project.
/// The IDK includes a manifest file that defines the contents of the IDK.  The manifest file, and
/// the root directory define a specific SdkRoot.
///
/// The other use case is in the Fuchsia.git source code project (aka in-tree). In this case the IDK
/// atom collection is used to locate the host and companion tools. This is done to avoid building
///  the complete IDK and results in dramatically reduced build times for common developer workflows.
///  When using SdkRoot in-tree, the root should be the $root_build_dir.
///
/// TODO(https://fxbug.dev/397989792) tracks removing this hard coded default path for in-tree IDK usage.
///
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SdkRoot {
    /// Full SDK root is actually referring to the root directory of an IDK.  This means it
    /// has the contents of is normally found in meta/manifest.json in the IDK. The paths in the
    ///  manifest are relative to the root directory.

    /// The manifest is optional where None indicates use one of the well known manifests. These are
    ///
    ///  `meta/manifest.json`, which represents the out of tree IDK structure.
    ///
    ///  If the manifest is specified, it must be a relative path to the manifest,
    /// based on the root directory.
    Full { root: PathBuf, manifest: Option<String> },

    /// No SDK root is known. This can happen when running ffx in-tree with an Isolate dir, or a
    ///  directory where the search for ../meta/manifest.json fails.
    ///  This is root is used to find host tools in the same directory as ffx is located. For example,
    /// ffx is in ./host-tools and so is symbolizer.
    HostTools { root: PathBuf },
}

/// A serde-serializable representation of ffx' sdk configuration.
/// Used by Isolate tests.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct FfxSdkConfig {
    pub root: Option<PathBuf>,
    pub manifest: Option<String>,
}

impl SdkRoot {
    /// Gets the basic information about the sdk as configured, without diving deeper into the sdk's own configuration.
    pub fn from_paths(start_path: Option<&Path>) -> Result<Self, SdkError> {
        // All gets in this function should declare that they don't want the build directory searched, because
        // if there is a build directory it *is* generally the sdk.
        let sdk_root = match start_path {
            Some(root) => root.to_owned(),
            _ => {
                let exe_path = find_exe_path()?;

                match Self::find_sdk_root(&Path::new(&exe_path)) {
                    Ok(Some(root)) => root,
                    Ok(None) => {
                        log::debug!(
                            "Could not find an SDK manifest in any parent of ffx's directory. \
                             Using {:?} as HostTools root",
                            exe_path.parent().unwrap()
                        );
                        return Ok(SdkRoot::HostTools {
                            root: exe_path.parent().unwrap().to_path_buf(),
                        });
                    }
                    Err(e) => {
                        return Err(SdkError::NotFoundWithHelp {
                            help: SDK_NOT_FOUND_HELP.to_string(),
                            source: Box::new(e),
                        });
                    }
                }
            }
        };

        log::debug!("Found full Fuchsia SDK at {sdk_root:?}");
        Ok(SdkRoot::Full { root: sdk_root, manifest: None })
    }

    fn find_sdk_root(start_path: &Path) -> Result<Option<PathBuf>, SdkError> {
        let cwd = std::env::current_dir().map_err(SdkError::ResolveCwd)?;
        let mut path = cwd.join(start_path);
        log::debug!("Attempting to find the sdk root from {path:?}");

        loop {
            path = if let Some(parent) = path.parent() {
                parent.to_path_buf()
            } else {
                return Ok(None);
            };

            if SdkRoot::is_sdk_root(&path) {
                log::debug!("Found sdk root through recursive search in {path:?}");
                return Ok(Some(path));
            }
        }
    }

    /// Returns true if the given path appears to be an sdk root.
    fn is_sdk_root(path: &Path) -> bool {
        path.join(SDK_MANIFEST_PATH).exists()
    }

    /// Returns manifest path if it exists.
    pub fn manifest_path(&self) -> Option<PathBuf> {
        match self {
            Self::Full { root, manifest: Some(manifest) } if root.join(manifest).exists() => {
                Some(root.join(manifest))
            }
            Self::Full { root, manifest: None } if root.join(SDK_MANIFEST_PATH).exists() => {
                Some(root.join(SDK_MANIFEST_PATH))
            }
            Self::Full { .. } => None,
            Self::HostTools { .. } => None,
        }
    }

    /// Does a full load of the sdk configuration.
    pub fn get_sdk(self) -> Result<Sdk, SdkError> {
        log::debug!("get_sdk from {self:?}");
        match self {
            Self::Full { root, manifest: Some(manifest_file) } => {
                // If manifest file is specified, use it as an IDK manifest.
                Sdk::from_sdk_dir(&root, &manifest_file).map_err(|e| SdkError::ManifestLoad {
                    path: root.join(manifest_file),
                    source: Box::new(e),
                })
            }
            Self::Full { root, manifest: None } if root.join(SDK_MANIFEST_PATH).exists() => {
                // If the manifest is not specified, but the SDK_MANIFEST exists, read it as the
                // IDK manifest.
                Sdk::from_sdk_dir(&root, SDK_MANIFEST_PATH).map_err(|e| SdkError::ManifestLoad {
                    path: root.join(SDK_MANIFEST_PATH),
                    source: Box::new(e),
                })
            }
            Self::Full { root, manifest: _ } => {
                return Err(SdkError::MissingManifest(root));
            }
            Self::HostTools { root } => {
                // This is not really a SDK, but a collection of host tools.
                Sdk::from_host_tools(root.clone())
                    .map_err(|e| SdkError::HostToolsLoad { path: root, source: Box::new(e) })
            }
        }
    }

    pub fn to_config(&self) -> FfxSdkConfig {
        match self.clone() {
            Self::Full { root, manifest } => FfxSdkConfig { root: Some(root), manifest },
            Self::HostTools { root } => FfxSdkConfig { root: Some(root), manifest: None },
        }
    }
}

/// Finds the executable path of the ffx binary being run, attempting to
/// get the path the user believes it to be at, even if it's symlinked from
/// somewhere else, by using `argv[0]` and [`std::env::current_exe`].
///
/// We do this because sometimes ffx is invoked through an SDK that is symlinked
/// into place from a content addressable store, and we want to make a best
/// effort to search for the sdk in the right place.
fn find_exe_path() -> Result<PathBuf, SdkError> {
    // get the 'real' binary path, which may have symlinks resolved, as well
    // as the command this was run as and the cwd
    let cwd = std::env::current_dir().map_err(SdkError::InvalidCwd)?;
    let binary_path = std::env::var("FFX_BIN")
        .map(PathBuf::from)
        .or_else(|_| std::env::current_exe())
        .and_then(|p| p.canonicalize())
        .map_err(SdkError::NoBinary)?;
    let args_path = match std::env::args_os().next() {
        Some(arg) => PathBuf::from(&arg),
        None => {
            log::trace!("FFX was run without an argv[0] somehow");
            return Ok(binary_path);
        }
    };

    // canonicalize the path from argv0 to try to figure out where it 'really'
    // is to make sure it's actually the right binary through potential
    // symlinks.
    let canonical_args_path = match args_path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            log::trace!(
                "Could not canonicalize the path ffx was run with, \
                which might mean the working directory has changed or the file \
                doesn't exist anymore: {e:?}"
            );
            return Ok(binary_path);
        }
    };

    // check that it's the same file in the end
    if binary_path == canonical_args_path {
        // but return the path it was actually run through instead of the canonical
        // path, but [`Path::join`]-ed to the cwd to make it more or less
        // absolute.
        Ok(cwd.join(args_path))
    } else {
        log::trace!(
            "FFX's argv[0] ({args_path:?}) resolved to {canonical_args_path:?} \
            instead of the binary's path {binary_path:?}, falling back to the \
            binary path."
        );
        Ok(binary_path)
    }
}

impl Sdk {
    pub fn from_sdk_dir(path_prefix: &Path, manifest_file: &str) -> Result<Self, SdkError> {
        let path_prefix = std::fs::canonicalize(path_prefix)
            .map_err(|e| SdkError::InvalidPath(path_prefix.to_owned(), e))?;
        let manifest_path = path_prefix.join(manifest_file);

        let manifest_file = Self::open_manifest(&manifest_path)?;
        let manifest: Manifest = Self::parse_manifest(&manifest_path, manifest_file)?;

        Ok(Sdk {
            path_prefix,
            module: None,
            parts: manifest.parts,
            real_paths: None,
            version: SdkVersion::Version(manifest.id),
        })
    }

    pub(crate) fn from_host_tools(host_tools_dir: PathBuf) -> Result<Self, SdkError> {
        Ok(Sdk {
            path_prefix: host_tools_dir,
            module: None,
            parts: vec![],
            real_paths: None,
            version: SdkVersion::InTree,
        })
    }

    pub fn new() -> Self {
        Sdk {
            path_prefix: PathBuf::new(),
            module: None,
            parts: vec![],
            real_paths: None,
            version: SdkVersion::Unknown,
        }
    }

    pub fn is_host_tools_only(&self) -> bool {
        return self.path_prefix.exists()
            && self.module.is_none()
            && self.parts.is_empty()
            && self.version == SdkVersion::InTree;
    }

    fn open_manifest(path: &Path) -> Result<fs::File, SdkError> {
        fs::File::open(path).map_err(|e| SdkError::OpenManifest(path.to_owned(), e))
    }

    fn parse_manifest<T: DeserializeOwned>(
        manifest_path: &Path,
        manifest_file: fs::File,
    ) -> Result<T, SdkError> {
        serde_json::from_reader(BufReader::new(manifest_file))
            .map_err(|e| SdkError::ParseManifest(manifest_path.to_owned(), e))
    }

    fn metadata_for<'a, M: DeserializeOwned>(
        &'a self,
        kinds: &'a [ElementType],
    ) -> impl Iterator<Item = M> + 'a {
        self.parts
            .iter()
            .filter_map(|part| {
                if kinds.contains(&part.kind) {
                    Some(self.path_prefix.join(&part.meta))
                } else {
                    None
                }
            })
            .filter_map(|path| match fs::File::open(path.clone()) {
                Ok(file) => Some((path, file)),
                Err(err) => {
                    warn!("Failed to open sdk metadata path: {} (error: {err})", path.display());
                    None
                }
            })
            .filter_map(|(path, file)| match serde_json::from_reader(file) {
                Ok(meta) => Some(meta),
                Err(err) => {
                    warn!("Failed to parse sdk metadata file: {} (error: {err})", path.display());
                    None
                }
            })
    }

    fn get_all_ffx_tools(&self) -> impl Iterator<Item = FfxTool> + '_ {
        self.metadata_for(&[ElementType::FfxTool])
    }

    pub fn get_ffx_tools(&self) -> impl Iterator<Item = FfxToolFiles> + '_ {
        self.get_all_ffx_tools().flat_map(|tool| {
            FfxToolFiles::from_metadata(self, tool, CpuArchitecture::current()).ok().flatten()
        })
    }

    pub fn get_ffx_tool(&self, name: &str) -> Option<FfxToolFiles> {
        self.get_all_ffx_tools()
            .filter(|tool| tool.name == name)
            .filter_map(|tool| {
                FfxToolFiles::from_metadata(self, tool, CpuArchitecture::current()).ok().flatten()
            })
            .max_by_key(|tool| tool.specificity_score)
    }

    /// Returns the path to the tool with the given name based on the SDK contents.
    /// A preferred alternative to this method is ffx_config::get_host_tool() which
    /// also considers configured overrides for the tools.
    pub fn get_host_tool(&self, name: &str) -> Result<PathBuf, SdkError> {
        let relative_path = self.get_host_tool_relative_path(name)?;

        let full_path = self.path_prefix.join(relative_path);

        if full_path.exists() {
            log::info!("Path {full_path:?} found for {name}");
            Ok(full_path)
        } else {
            log::info!("No path found for {name}");
            Err(SdkError::NoPathFound(name.to_string()))
        }
    }

    /// Get the metadata for all host tools
    pub fn get_all_host_tools_metadata(&self) -> impl Iterator<Item = HostTool> + '_ {
        self.metadata_for(&[ElementType::HostTool, ElementType::CompanionHostTool])
    }

    fn get_host_tool_relative_path(&self, name: &str) -> Result<PathBuf, SdkError> {
        let found_tool = self
            .get_all_host_tools_metadata()
            .filter(|tool| tool.name == name)
            .map(|tool| match &tool.files.as_deref() {
                Some([tool_path]) => Ok(tool_path.to_owned()),
                Some([tool_path, ..]) => {
                    warn!("Tool '{}' provides multiple files in manifest", name);
                    Ok(tool_path.to_owned())
                }
                Some([]) | None => {
                    // if this is a "host tools" SDK, return the tool name.
                    if self.is_host_tools_only() {
                        Ok(name.to_string())
                    } else {
                        Err(SdkError::NoExecutable(name.to_string()))
                    }
                }
            })
            .collect::<Result<Vec<_>, SdkError>>()?
            .into_iter()
            // Shortest path is the one with no arch specifier, i.e. the default arch, i.e. the current arch (we hope.)
            .min_by_key(|x| x.len());

        if let Some(tool) = found_tool {
            self.get_real_path(tool)
        } else {
            if self.is_host_tools_only() {
                Ok(PathBuf::from(name))
            } else {
                Err(SdkError::NoExecutable(name.to_string()))
            }
        }
    }

    fn get_real_path(&self, path: impl AsRef<str>) -> Result<PathBuf, SdkError> {
        match &self.real_paths {
            Some(map) => map
                .get(path.as_ref())
                .map(PathBuf::from)
                .ok_or_else(|| SdkError::NoSource(path.as_ref().to_string())),
            _ => Ok(PathBuf::from(path.as_ref())),
        }
    }

    /// Returns a command invocation builder for the given host tool, if it
    /// exists in the sdk.
    pub fn get_host_tool_command(&self, name: &str) -> Result<Command, SdkError> {
        let host_tool = self.get_host_tool(name)?;
        let mut command = Command::new(host_tool);
        command.env("FUCHSIA_SDK_ROOT", &self.path_prefix);
        if let Some(module) = self.module.as_deref() {
            command.env("FUCHSIA_SDK_ENV", module);
        }
        Ok(command)
    }

    pub fn get_path_prefix(&self) -> &Path {
        &self.path_prefix
    }

    pub fn get_version(&self) -> &SdkVersion {
        &self.version
    }

    pub fn get_version_string(&self) -> Option<String> {
        match &self.version {
            SdkVersion::Version(version) => Some(version.to_string()),
            SdkVersion::InTree => Some(in_tree_sdk_version()),
            SdkVersion::Unknown => None,
        }
    }

    /// For tests only
    #[doc(hidden)]
    pub fn get_empty_sdk_with_version(version: SdkVersion) -> Sdk {
        Sdk {
            path_prefix: PathBuf::new(),
            module: None,
            parts: Vec::new(),
            real_paths: None,
            version,
        }
    }
}

/// Even though an sdk_version for in-tree is an oxymoron, a value can be
/// generated.
///
/// Returns the current "F" milestone (e.g. F38) and a fixed date.major.minor
/// value of ".99991231.0.1". (e.g. "38.99991231.0.1" altogether).
///
/// The value was chosen because:
/// - it will never conflict with a real sdk build
/// - it will be newest for an sdk build of the same F
/// - it's just weird enough to recognizable and searchable
/// - the major.minor values align with fuchsia.dev guidelines
pub fn in_tree_sdk_version() -> String {
    format!("{}.99991231.0.1", MILESTONE.trim())
}

impl FfxToolFiles {
    fn from_metadata(
        sdk: &Sdk,
        tool: FfxTool,
        arch: CpuArchitecture,
    ) -> Result<Option<Self>, SdkError> {
        let Some(executable) = tool.executable(arch) else {
            return Ok(None);
        };
        let Some(metadata) = tool.executable_metadata(arch) else {
            return Ok(None);
        };

        // Increment the score by zero or one for each of the executable and
        // metadata files, depending on if they're architecture specific or not,
        // for a total score of 0-2 (least specific to most specific).
        let specificity_score = executable.arch.map_or(0, |_| 1) + metadata.arch.map_or(0, |_| 1);
        let executable = sdk.path_prefix.join(&sdk.get_real_path(executable.file)?);
        let metadata = sdk.path_prefix.join(&sdk.get_real_path(metadata.file)?);
        Ok(Some(Self { executable, metadata, specificity_score }))
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use regex::Regex;
    use std::fs;
    use std::io::Write;
    use tempfile::{TempDir, tempdir};

    /// Writes the file to $root, with the path $path, from the source tree prefix $prefix
    /// (relative to this source file)
    macro_rules! put_file {
        ($root:expr, $prefix:literal, $name:literal) => {{
            fs::create_dir_all($root.path().join($name).parent().unwrap()).unwrap();
            fs::File::create($root.path().join($name))
                .unwrap()
                .write_all(include_bytes!(concat!($prefix, "/", $name)))
                .unwrap();
        }};
    }

    fn sdk_test_data_root() -> TempDir {
        let r = tempfile::tempdir().unwrap();
        put_file!(r, "../test_data/release-sdk-root", "fidl/fuchsia.data/meta.json");
        put_file!(r, "../test_data/release-sdk-root", "tools/ffx_tools/ffx-assembly-meta.json");
        put_file!(r, "../test_data/release-sdk-root", "meta/manifest.json");
        put_file!(r, "../test_data/release-sdk-root", "tools/zxdb-meta.json");
        r
    }

    #[test]
    fn test_manifest_exists() {
        let release_root = sdk_test_data_root();

        assert!(
            SdkRoot::Full {
                root: release_root.path().to_owned(),
                manifest: Some(SDK_MANIFEST_PATH.into())
            }
            .manifest_path()
            .is_some()
        );
    }

    #[fuchsia::test]
    fn test_sdk_manifest() {
        let root = sdk_test_data_root();
        let sdk_root = root.path();
        let manifest: Manifest = serde_json::from_reader(BufReader::new(
            fs::File::open(sdk_root.join(SDK_MANIFEST_PATH)).unwrap(),
        ))
        .unwrap();

        assert_eq!("0.20201005.4.1", manifest.id);

        let mut parts = manifest.parts.iter();
        assert!(matches!(parts.next().unwrap(), Part { kind: ElementType::FidlLibrary, .. }));
        assert!(matches!(parts.next().unwrap(), Part { kind: ElementType::HostTool, .. }));
        assert!(matches!(parts.next().unwrap(), Part { kind: ElementType::FfxTool, .. }));
        assert!(parts.next().is_none());
    }

    #[fuchsia::test]
    fn test_sdk_manifest_host_tool() {
        let root = sdk_test_data_root();
        let sdk_root = root.path();
        let manifest: Manifest = serde_json::from_reader(BufReader::new(
            fs::File::open(sdk_root.join(SDK_MANIFEST_PATH)).unwrap(),
        ))
        .unwrap();
        let expected = sdk_root.join("tools/zxdb");
        fs::write(&expected, "#!/bin/bash\n echo hello").expect("fake host tool");
        let sdk = Sdk {
            path_prefix: sdk_root.to_owned(),
            module: None,
            parts: manifest.parts,
            real_paths: None,
            version: SdkVersion::Version(manifest.id.to_owned()),
        };
        let zxdb = sdk.get_host_tool("zxdb").unwrap();

        assert_eq!(expected, zxdb);

        let zxdb_cmd = sdk.get_host_tool_command("zxdb").unwrap();
        assert_eq!(zxdb_cmd.get_program(), sdk_root.join("tools/zxdb"));
    }

    #[fuchsia::test]
    fn test_sdk_manifest_ffx_tool() {
        let root = sdk_test_data_root();
        let sdk_root = root.path();
        let manifest: Manifest = serde_json::from_reader(BufReader::new(
            fs::File::open(sdk_root.join(SDK_MANIFEST_PATH)).unwrap(),
        ))
        .unwrap();

        let sdk = Sdk {
            path_prefix: sdk_root.to_owned(),
            module: None,
            parts: manifest.parts,
            real_paths: None,
            version: SdkVersion::Version(manifest.id.to_owned()),
        };
        let ffx_assembly = sdk.get_ffx_tool("ffx-assembly").unwrap();

        // get_ffx_tool selects with the current architecture, so the executable path will be
        // architecture-dependent.
        let current_arch = CpuArchitecture::current();
        let arch = match current_arch {
            CpuArchitecture::Arm64 => "arm64",
            CpuArchitecture::X64 => "x64",
            CpuArchitecture::Riscv64 => "riscv64",
            _ => panic!("Unsupported host tool architecture {}", current_arch),
        };
        assert_eq!(
            sdk_root.join("tools").join(arch).join("ffx_tools/ffx-assembly"),
            ffx_assembly.executable
        );
        assert_eq!(sdk_root.join("tools/ffx_tools/ffx-assembly.json"), ffx_assembly.metadata);
    }

    #[test]
    fn test_in_tree_sdk_version() {
        let version = in_tree_sdk_version();
        let re = Regex::new(r"^\d+.99991231.0.1$").expect("creating regex");
        assert!(re.is_match(&version));
    }

    #[fuchsia::test]
    fn test_find_sdk_root_finds_root() {
        let temp = tempdir().unwrap();
        let temp_path = std::fs::canonicalize(temp.path()).expect("canonical temp path");

        let start_path = temp_path.join("test1").join("test2");
        std::fs::create_dir_all(start_path.clone()).unwrap();

        let meta_path = temp_path.join("meta");
        std::fs::create_dir(meta_path.clone()).unwrap();

        std::fs::write(meta_path.join("manifest.json"), "").unwrap();

        assert_eq!(SdkRoot::find_sdk_root(&start_path).unwrap().unwrap(), temp_path);
    }

    #[fuchsia::test]
    fn test_find_sdk_root_no_manifest() {
        let temp = tempdir().unwrap();

        let start_path = temp.path().to_path_buf().join("test1").join("test2");
        std::fs::create_dir_all(start_path.clone()).unwrap();

        let meta_path = temp.path().to_path_buf().join("meta");
        std::fs::create_dir(meta_path).unwrap();

        assert!(SdkRoot::find_sdk_root(&start_path).unwrap().is_none());
    }

    #[fuchsia::test]
    fn test_host_tool_root() {
        let temp = tempdir().unwrap();

        // It is difficult to test creating SdkRoot in a unit test since there is code that
        // attempts to detect and navigate the build directory (and it does so well).

        // The HostTool Root is effectively the "SDKRoot of last resort", so the tests should make
        // sure it behaves predictably and fails gracefully if more than just host tools are accessed
        // via this root.
        let start_path = temp.path().to_path_buf().join("test1").join("test2");
        std::fs::create_dir_all(start_path.clone()).unwrap();

        let sdk_root = SdkRoot::HostTools { root: start_path.clone() };

        let manifest = sdk_root.manifest_path();
        assert!(manifest.is_none(), "Expected None manifest, got {manifest:?}");

        let sdk = sdk_root.clone().get_sdk().expect("SDK from sdk_root");

        assert_eq!(sdk.get_path_prefix(), start_path.as_path());

        let config = sdk_root.to_config();

        assert_eq!(config.root, Some(start_path));
        assert_eq!(config.manifest, None);
    }

    #[fuchsia::test]
    fn test_host_tool_sdk() {
        let temp = tempdir().unwrap();

        let start_path = temp.path().to_path_buf().join("some").join("bin");
        std::fs::create_dir_all(start_path.clone()).unwrap();

        // write a test host tool
        fs::write(start_path.join("some-tool"), "contents of host tool").expect("write some-tool");

        let sdk_root = SdkRoot::HostTools { root: start_path.clone() };

        let sdk = sdk_root.clone().get_sdk().expect("SDK from sdk_root");

        assert_eq!(sdk.get_path_prefix(), start_path.as_path());

        let version = sdk.get_version();
        match version {
            SdkVersion::InTree => (),
            _ => panic!("Expected in-tree SDK version, got {version:?}"),
        };

        assert!(sdk.is_host_tools_only());

        let ffx_tools: Vec<_> = sdk.get_ffx_tools().collect();
        assert!(ffx_tools.is_empty());

        let some_tool = sdk.get_ffx_tool("ffx-some");
        assert!(some_tool.is_none());

        let host_tool = sdk.get_host_tool("some-tool").expect("some-tool");
        assert_eq!(host_tool, start_path.join("some-tool"));

        let some_cmd = sdk.get_host_tool_command("some-tool").expect("host tool command");
        assert_eq!(some_cmd.get_program(), start_path.join("some-tool"));
    }
}
