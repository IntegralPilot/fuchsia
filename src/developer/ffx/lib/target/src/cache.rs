// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TargetInfo;

use chrono::prelude::*;
use discovery::query::TargetInfoQuery;
use ffx_config::EnvironmentContext;
use ffx_config::keys::DISCOVERY_CACHE_DIR_CONFIG;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::NamedTempFile;
use thiserror::Error;

const CACHE_FILE_NAME: &str = "ffx-target-info.json";
const CACHE_VERSION: u32 = 2;
const CACHE_TTL_SECONDS: u64 = 60;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

type Result<T> = std::result::Result<T, CacheError>;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("bad location of cache file at {path:?}")]
    BadLocation { path: PathBuf },

    #[error("opening cache file at {path:?}")]
    OpenFile {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("creating cache file at {path:?}")]
    CreateFile {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("deserializing cache from {path:?}")]
    Deserialize {
        path: PathBuf,
        #[source]
        err: serde_json::Error,
    },

    #[error("serializing cache to {path:?}")]
    Serialize {
        path: PathBuf,
        #[source]
        err: serde_json::Error,
    },

    #[error("bad cache version: {0}")]
    BadVersion(u32),

    #[error("cache expired at {0}")]
    Expired(chrono::DateTime<chrono::Utc>),

    #[error("could not rename cache file from {from:?} to {to:?}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("could not get discovery cache file path")]
    NoCacheFile,

    #[error("cannot stat cache file at {path:?}")]
    StatFile {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("cannot remove cache file at {path:?}")]
    RemoveFile {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },
}

/// Create the target discovery cache file. The directory containing the file
/// must already exist. This will list the current targets, but will not try to
/// make an RCS connection to them.
pub async fn create_target_cache(
    context: &EnvironmentContext,
) -> std::result::Result<Vec<TargetInfo>, crate::FfxTargetCrateError> {
    let Some(cache_file) = get_discovery_cache_file(context) else {
        return Err(CacheError::NoCacheFile.into());
    };
    let infos =
        crate::list::list_targets(context, TargetInfoQuery::First, true, true, true).await?;
    // The cache is used to quickly find targets matching a query. With that
    // in mind, we are storing more than we need to (since queries are based
    // only on the name/serial/addrs).  We could just store the subset of the
    // information that we need, but it doesn't save time since we have to do
    // list_targets() regardless. Keeping the extra information could allow us
    // to add other queries in the future, such as by board.
    // Note that while the cache is only used to "resolve" queries (which is a
    // term used to resolve to a _product_), we are storing enough information
    // to match against non-product targets as well. We do not because the
    // assumption is that subtools (e.g. flash) that want to find non-product
    // targets generally avoid the cache, since they want to get the most
    // up-to-date state. But this may change, which is why the cache is not
    // explicitly associated with a "Resolution".
    let cache = Cache::new(infos.clone());
    cache.save(&cache_file)?;
    Ok(infos)
}

/// Remove the target cache file, if it exists.
pub fn remove_target_cache(
    context: &EnvironmentContext,
) -> std::result::Result<(), crate::FfxTargetCrateError> {
    let Some(cache_file) = get_discovery_cache_file(context) else {
        return Ok(());
    };
    if std::fs::exists(&cache_file)
        .map_err(|e| CacheError::StatFile { path: cache_file.clone(), err: e })?
    {
        std::fs::remove_file(&cache_file)
            .map_err(|e| CacheError::RemoveFile { path: cache_file.clone(), err: e })?;
    }
    Ok(())
}

/// Directory containing the discovery cache file
pub fn get_discovery_cache_dir(context: &EnvironmentContext) -> Option<PathBuf> {
    let path_s: Option<String> = match context.get(DISCOVERY_CACHE_DIR_CONFIG) {
        Ok(opath) => opath,
        Err(e) => {
            log::debug!("Could not get {DISCOVERY_CACHE_DIR_CONFIG}: {e}");
            None
        }
    };
    path_s.map(|s| s.into())
}

/// Path to discovery cache file, if any
pub fn get_discovery_cache_file(context: &EnvironmentContext) -> Option<PathBuf> {
    let cache_path = get_discovery_cache_dir(context);
    cache_path.map(|mut d| {
        d.push(CACHE_FILE_NAME);
        d
    })
}

/// Return the time to wait before updating the cache. Because writing the cache itself takes
/// some time (due to the need to wait for discovery to complete), the recheck time is
/// less than the cache TTL.
pub fn get_discovery_cache_recheck_time() -> Duration {
    Duration::from_secs(CACHE_TTL_SECONDS) - DEFAULT_TIMEOUT
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Cache {
    version: u32,
    expires: DateTime<Utc>,
    pub(crate) targets: Vec<TargetInfo>,
}

impl Cache {
    pub(crate) fn new(targets: Vec<TargetInfo>) -> Self {
        Self {
            version: CACHE_VERSION,
            expires: Utc::now() + chrono::Duration::seconds(CACHE_TTL_SECONDS as i64),
            targets,
        }
    }
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let file = fs::File::open(path)
            .map_err(|err| CacheError::OpenFile { path: path.to_path_buf(), err })?;
        let reader = BufReader::new(file);
        let cache: Cache = serde_json::from_reader(reader)
            .map_err(|err| CacheError::Deserialize { path: path.to_path_buf(), err })?;
        if cache.version != CACHE_VERSION {
            return Err(CacheError::BadVersion(cache.version));
        }
        if Utc::now() > cache.expires {
            log::debug!(
                "cache at {path:?} expired at {}",
                cache.expires.format("%Y-%m-%d %H:%M:%S")
            );
            return Err(CacheError::Expired(cache.expires));
        }
        Ok(cache)
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        let Some(dir) = path.parent() else {
            return Err(CacheError::BadLocation { path: path.to_path_buf() });
        };
        let mut temp_file = NamedTempFile::new_in(dir)
            .map_err(|err| CacheError::CreateFile { path: path.to_path_buf(), err })?;
        let writer = BufWriter::new(&mut temp_file);
        serde_json::to_writer(writer, self)
            .map_err(|err| CacheError::Serialize { path: path.to_path_buf(), err })?;
        temp_file
            .as_file_mut()
            .sync_all()
            .map_err(|err| CacheError::CreateFile { path: path.to_path_buf(), err })?;
        temp_file.persist(path).map_err(|e| CacheError::Rename {
            from: e.file.path().to_path_buf(),
            to: path.to_path_buf(),
            err: e.error,
        })?;
        log::debug!("Cache {path:?} saved at {}", self.expires.format("%Y-%m-%d %H:%M:%S"));
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn _set_expires(&mut self, expires: DateTime<Utc>) {
        self.expires = expires;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_config::environment::ExecutableKind;
    use pretty_assertions::assert_eq;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn create_test_cache(targets: Vec<TargetInfo>) -> Cache {
        Cache::new(targets)
    }

    fn create_test_info(name: &str) -> TargetInfo {
        TargetInfo { nodename: Some(name.to_string()), ..TargetInfo::default() }
    }

    #[fuchsia::test]
    fn test_save_and_load_successfully() -> Result<()> {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let info = create_test_info("test-target");
        let cache_to_save = create_test_cache(vec![info.clone()]);

        cache_to_save.save(&cache_path)?;

        let loaded_cache = Cache::load(&cache_path)?;

        assert_eq!(loaded_cache.version, CACHE_VERSION);
        assert_eq!(loaded_cache.targets.len(), 1);
        assert_eq!(loaded_cache.targets[0], info);
        assert!(loaded_cache.expires > Utc::now());

        Ok(())
    }

    #[fuchsia::test]
    fn test_load_expired_cache() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let info = create_test_info("test-target");
        let mut cache = create_test_cache(vec![info]);
        cache.expires = Utc::now() - chrono::Duration::seconds(1);

        cache.save(&cache_path).unwrap();

        let result = Cache::load(&cache_path);
        assert!(matches!(result, Err(CacheError::Expired(_))));
    }

    #[fuchsia::test]
    fn test_load_bad_version() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let info = create_test_info("test-target");
        let mut cache = create_test_cache(vec![info]);
        cache.version = CACHE_VERSION + 1;

        cache.save(&cache_path).unwrap();

        let result = Cache::load(&cache_path);
        assert!(matches!(result, Err(CacheError::BadVersion(_))));
    }

    #[fuchsia::test]
    fn test_load_file_not_found() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("nonexistent.json");
        let result = Cache::load(&cache_path);
        assert!(matches!(result, Err(CacheError::OpenFile { .. })));
    }

    #[fuchsia::test]
    fn test_load_corrupt_json() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("corrupt.json");
        let mut file = fs::File::create(&cache_path).unwrap();
        writeln!(file, "{{{{ not valid json }}").unwrap();

        let result = Cache::load(&cache_path);
        assert!(matches!(result, Err(CacheError::Deserialize { .. })));
    }

    #[fuchsia::test]
    fn test_save_permission_denied() {
        let dir = tempdir().unwrap();
        let readonly_dir = dir.path();
        let mut perms = fs::metadata(readonly_dir).unwrap().permissions();
        perms.set_mode(0o555); // Read and execute, but not write
        fs::set_permissions(readonly_dir, perms).unwrap();

        let cache_path = readonly_dir.join("cache.json");
        let info = create_test_info("test-target");
        let cache_to_save = create_test_cache(vec![info]);

        let result = cache_to_save.save(&cache_path);
        assert!(matches!(result, Err(CacheError::CreateFile { .. })));

        // Restore write permissions so the tempdir can be cleaned up.
        let mut perms = fs::metadata(readonly_dir).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(readonly_dir, perms).unwrap();
    }

    #[fuchsia::test]
    async fn test_get_discovery_cache_dir() {
        let cache_dir = "/tmp/cache";
        let test_env = ffx_config::test_env()
            .user_config(DISCOVERY_CACHE_DIR_CONFIG, cache_dir.to_string())
            .build()
            .unwrap();

        let result = get_discovery_cache_dir(&test_env.context);
        assert_eq!(result, Some(PathBuf::from(cache_dir)));
    }

    #[fuchsia::test]
    async fn test_get_discovery_cache_dir_strict() {
        let context =
            EnvironmentContext::strict(ExecutableKind::Test, ffx_config::ConfigMap::new()).unwrap();
        let result = get_discovery_cache_dir(&context);
        assert!(result.is_none(), "Expected none, got {result:?}");
    }

    #[test]
    fn test_get_discovery_cache_dir_strict_exists() {
        let cache_dir = "/tmp/foo/bar";
        let config_map =
            [("target".to_owned(), serde_json::json!({"discovery_cache_dir": "/tmp/foo/bar"}))]
                .into_iter()
                .collect();
        let context = EnvironmentContext::strict(ExecutableKind::Test, config_map).unwrap();

        let result = get_discovery_cache_dir(&context);
        assert_eq!(result, Some(PathBuf::from(cache_dir)));
    }
}
