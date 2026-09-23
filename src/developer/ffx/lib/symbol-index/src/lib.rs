// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(thiserror::Error, Debug)]
pub enum SymbolIndexError {
    #[error("Cannot find home directory")]
    HomeDirectoryNotFound,

    #[error("Cannot convert OsString into String: {0:?}")]
    OsStringConversionError(std::ffi::OsString),

    #[error("FFX Core Error: {0}")]
    Ffx(#[from] errors::FfxError),

    #[error("FFX Environment Context error: {0}")]
    Context(#[from] ffx_config::environment::ContextError),

    #[error("Serde JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SymbolIndexError>;
use errors::ffx_error;
use ffx_config::EnvironmentContext;
use glob::glob as _glob;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};

pub fn global_symbol_index_path() -> Result<String> {
    let home = home::home_dir().ok_or(SymbolIndexError::HomeDirectoryNotFound)?;
    Ok(pathbuf_to_string(home)? + "/.fuchsia/debug/symbol-index.json")
}

// Ensures that symbols in the root or sdk.root are registered in the global symbol index.
pub fn ensure_symbol_index_registered(context: &EnvironmentContext) -> Result<()> {
    let symbol_index_path = if let Some(build_dir) = context.build_dir() {
        build_dir.join(".symbol-index.json")
    } else {
        context.get_sdk()?.get_path_prefix().join("data/config/symbol_index*/config.json")
    };

    let symbol_index_path = pathbuf_to_string(symbol_index_path)?;

    let global_symbol_index = global_symbol_index_path()?;

    // Note: This clobbers the existing global symbol-index if it is invalid.
    let mut index = SymbolIndex::load(&global_symbol_index).unwrap_or_else(|_| SymbolIndex::new());
    if !index.includes.contains(&symbol_index_path) {
        index.includes.push(symbol_index_path);
        index.save(&global_symbol_index)?;
    }

    return Ok(());
}

#[derive(Serialize, Deserialize, Debug, JsonSchema)]
pub struct BuildIdDir {
    pub path: String,
    pub build_dir: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema)]
pub struct IdsTxt {
    pub path: String,
    pub build_dir: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema)]
pub struct GcsFlat {
    pub url: String,
    #[serde(default)]
    pub require_authentication: bool,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema)]
pub struct DebugInfoD {
    pub url: String,
    #[serde(default)]
    pub require_authentication: bool,
}

#[derive(Serialize, Deserialize, Debug, JsonSchema)]
pub struct SymbolIndex {
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub build_id_dirs: Vec<BuildIdDir>,
    #[serde(default)]
    pub ids_txts: Vec<IdsTxt>,
    #[serde(default)]
    pub gcs_flat: Vec<GcsFlat>,
    #[serde(default)]
    pub debuginfod: Vec<DebugInfoD>,
}

impl SymbolIndex {
    pub fn new() -> Self {
        Self {
            includes: Vec::new(),
            build_id_dirs: Vec::new(),
            ids_txts: Vec::new(),
            gcs_flat: Vec::new(),
            debuginfod: Vec::new(),
        }
    }

    /// Load a file at given path as is.
    pub fn load(path: &str) -> Result<SymbolIndex> {
        let index: SymbolIndex = serde_json::from_reader(
            File::open(path).map_err(|_| ffx_error!("Cannot open {}", path))?,
        )
        .map_err(|_| ffx_error!("Cannot parse {}", path))?;
        Ok(index)
    }

    /// Load a file at given path and resolve relative paths.
    pub fn load_resolve(path: &str) -> Result<SymbolIndex> {
        let mut index = Self::load(path)?;

        // Resolve all paths to absolute.
        let mut base = PathBuf::from(path);
        base.pop(); // Only keeps the directory part.
        index.includes =
            index.includes.into_iter().flat_map(|p| glob(resolve_path(&base, &p))).collect();
        index.build_id_dirs = index
            .build_id_dirs
            .into_iter()
            .flat_map(|build_id_dir| {
                let build_dir = build_id_dir.build_dir.map(|p| resolve_path(&base, &p));
                glob(resolve_path(&base, &build_id_dir.path))
                    .into_iter()
                    .map(move |p| BuildIdDir { path: p, build_dir: build_dir.clone() })
            })
            .collect();
        index.ids_txts = index
            .ids_txts
            .into_iter()
            .flat_map(|ids_txt| {
                let build_dir = ids_txt.build_dir.map(|p| resolve_path(&base, &p));
                glob(resolve_path(&base, &ids_txt.path))
                    .into_iter()
                    .map(move |p| IdsTxt { path: p, build_dir: build_dir.clone() })
            })
            .collect();
        Ok(index)
    }

    /// Load the file, resolve paths and aggregate all the includes.
    pub fn load_aggregate(path: &str) -> Result<SymbolIndex> {
        let mut index = SymbolIndex::new();
        let mut visited: HashSet<String> = HashSet::new();
        index.includes.push(path.to_owned());

        while let Some(include) = index.includes.pop() {
            if visited.contains(&include) {
                continue;
            }
            match Self::load_resolve(&include) {
                Ok(mut sub) => {
                    index.includes.append(&mut sub.includes);
                    index.build_id_dirs.append(&mut sub.build_id_dirs);
                    index.ids_txts.append(&mut sub.ids_txts);
                    index.gcs_flat.append(&mut sub.gcs_flat);
                    index.debuginfod.append(&mut sub.debuginfod);
                }
                Err(err) => {
                    // Only report error for the main entry.
                    if visited.is_empty() {
                        return Err(err);
                    }
                }
            }
            visited.insert(include);
        }
        Ok(index)
    }

    pub fn save(&self, path: &str) -> Result<()> {
        let path = PathBuf::from(path);
        let parent = path.parent().ok_or(ffx_error!("Invalid path {:?}", path))?;
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ffx_error!("Cannot create {:?}: {}", parent, e))?;
        }
        let file =
            File::create(&path).map_err(|e| ffx_error!("Cannot create {:?}: {}", path, e))?;
        serde_json::to_writer_pretty(file, self).map_err(|err| err.into())
    }
}

/// Resolve a relative path against a base directory, with possible ".."s at the beginning.
/// If the relative is actually absolute, return it directly. The base directory could be either
/// an absolute path or a relative path.
///
/// Unlike std::fs::canonicalize, this method does not require paths to exist and it does not
/// resolve symbolic links.
pub fn resolve_path(base: &Path, relative: &str) -> String {
    let mut path = base.to_owned();
    for comp in Path::new(relative).components() {
        match comp {
            std::path::Component::RootDir => {
                path.push(relative);
                break;
            }
            std::path::Component::ParentDir => {
                path.pop();
            }
            std::path::Component::Normal(name) => {
                path.push(name);
            }
            _ => {}
        }
    }
    path.into_os_string().into_string().unwrap()
}

fn glob(path: String) -> Vec<String> {
    // Only glob if "*" is in the string, so nonexistent paths will still be preserved.
    if path.contains('*') {
        _glob(&path)
            .map(|paths| {
                paths
                    .filter_map(|res| {
                        res.ok().map(|p| p.into_os_string().into_string().ok()).flatten()
                    })
                    .collect()
            })
            .unwrap_or_else(|_| vec![path])
    } else {
        vec![path]
    }
}

fn pathbuf_to_string(pathbuf: PathBuf) -> Result<String> {
    pathbuf.into_os_string().into_string().map_err(|s| SymbolIndexError::OsStringConversionError(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DATA_DIR: &str = "../../src/developer/ffx/lib/symbol-index/test_data";

    #[test]
    fn test_load() {
        let index = SymbolIndex::load(&format!("{}/main.json", TEST_DATA_DIR)).unwrap();
        assert_eq!(index.includes.len(), 2);
        assert_eq!(index.includes[0], "./another.*");
    }

    #[test]
    fn test_load_resolve() {
        let index = SymbolIndex::load_resolve(&format!("{}/main.json", TEST_DATA_DIR)).unwrap();
        assert_eq!(index.includes.len(), 2);
        assert_eq!(index.build_id_dirs.len(), 1);
        assert_eq!(index.build_id_dirs[0].path, "/home/someone/.fuchsia/debug/symbol-cache");
        assert_eq!(index.ids_txts.len(), 1);
        assert_eq!(index.ids_txts[0].path, format!("{}/ids.txt", TEST_DATA_DIR));
        assert_eq!(
            index.ids_txts[0].build_dir,
            Some(str::replace(TEST_DATA_DIR, "test_data", "build_dir"))
        );
        assert_eq!(index.gcs_flat.len(), 1);
        assert_eq!(index.gcs_flat[0].require_authentication, false);
        assert_eq!(index.debuginfod.len(), 1);
        assert_eq!(index.debuginfod[0].require_authentication, false);
    }

    #[test]
    fn test_aggregate() {
        let index = SymbolIndex::load_aggregate(&format!("{}/main.json", TEST_DATA_DIR)).unwrap();
        assert_eq!(index.includes.len(), 0);
        assert_eq!(index.gcs_flat.len(), 2);
        assert_eq!(index.gcs_flat[1].require_authentication, true);
        assert_eq!(index.debuginfod.len(), 1);
        assert_eq!(index.debuginfod[0].require_authentication, false);
    }

    #[test]
    fn test_save() {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let mut path = tmpdir.path().to_str().unwrap().to_owned();
        path.push_str("/subdir/symbol-index.json");
        let mut index = SymbolIndex::new();
        index.gcs_flat.push(GcsFlat { url: String::new(), require_authentication: false });
        index.save(&path).unwrap();
        let index = SymbolIndex::load(&path).unwrap();
        assert_eq!(index.gcs_flat.len(), 1);
    }
}
