// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect as finspect;
use futures::FutureExt as _;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// `RootDirCache` is a cache of `Arc<RootDir>`s indexed by their hash.
///
/// The cache internally stores `Weak<RootDir>`s and installs a custom `dropper` in its managed
/// `RootDir`s that removes the corresponding entry when dropped, so it is a cache of
/// `Arc<RootDir>`s that are actively in use by its clients. This is useful for deduplicating
/// the `Arc<RootDir>`s used by VFS to serve package directory connections while also keeping
/// track of which connections are open.
///
/// Because of how `RootDir`` is implemented, a package will have alive `Arc`s if there are:
///   1. fuchsia.io.Directory connections to the package's root directory or any sub directory.
///   2. fuchsia.io.File connections to the package's files under meta/.
///   3. fuchsia.io.File connections to the package's content blobs (files not under meta/) *iff*
///      the `crate::NonMetaStorage` impl serves the connections itself (instead of
///      forwarding to a remote server).
///
/// The `NonMetaStorage` impl we use for Fxblob does serve the File connections itself.
/// The impl we use for Blobfs does not, but Blobfs will wait to delete blobs that have
/// open connections until the last connection closes.
/// Similarly, both Blobfs and Fxblob will wait to delete blobs until the last VMO is closed (VMOs
/// obtained from fuchsia.io.File.GetBackingMemory will not keep a package alive), so it is safe to
/// delete packages that RootDirCache says are not open.
///
/// Clients close connections to packages by closing their end of the Zircon channel over which the
/// fuchsia.io.[File|Directory] messages were being sent. Some time after the client end of the
/// channel is closed, the server (usually in a different process) will be notified by the kernel,
/// and the task serving the connection will finish, dropping its `Arc<RootDir>`.
/// When the last `Arc` is dropped, the strong count of the corresponding `std::sync::Weak` in
/// the `RootDirCache` will decrement to zero. At this point the `RootDirCache`
/// will no longer report the package as open.
/// All this is to say that there will be some delay between a package no longer being in use and
/// clients of `RootDirCache` finding out about that.
#[derive(Debug, Clone)]
pub struct RootDirCache<S> {
    non_meta_storage: S,
    dirs: Arc<std::sync::Mutex<HashMap<fuchsia_hash::Hash, Weak<crate::RootDir<S>>>>>,
}

impl<S: crate::NonMetaStorage + Clone> RootDirCache<S> {
    /// Creates a `RootDirCache` that uses `non_meta_storage` as the backing for the
    /// internally managed `crate::RootDir`s.
    pub fn new(non_meta_storage: S) -> Self {
        let dirs = Arc::new(std::sync::Mutex::new(HashMap::new()));
        Self { non_meta_storage, dirs }
    }

    /// Returns an `Arc<RootDir>` corresponding to `hash`.
    /// If there is not already one in the cache, `root_dir` will be used if provided, otherwise
    /// a new one will be created using the `non_meta_storage` provided to `Self::new`.
    ///
    /// If provided, `root_dir` must be backed by the same `NonMetaStorage` that `Self::new` was
    /// called with. The provided `root_dir` must not have a dropper set.
    pub async fn get_or_insert(
        &self,
        hash: fuchsia_hash::Hash,
        root_dir: Option<crate::RootDir<S>>,
    ) -> Result<Arc<crate::RootDir<S>>, crate::Error> {
        Ok(if let Some(root_dir) = self.get(&hash) {
            root_dir
        } else {
            // If this is dropped while the dirs lock is held it will deadlock.
            let dropper = Box::new(Dropper { dirs: Arc::downgrade(&self.dirs), hash });
            let new_root_dir = match root_dir {
                Some(mut root_dir) => match root_dir.set_dropper(dropper) {
                    Ok(()) => Arc::new(root_dir),
                    // Okay to drop the dropper, the lock is not held and the drop impl handles
                    // missing entries.
                    Err(_) => {
                        return Err(crate::Error::DropperAlreadySet);
                    }
                },
                None => {
                    // Do this without the lock held because:
                    // 1. Making a RootDir takes ~100 μs (reading the meta.far)
                    // 2. If new_with_dropper errors it will drop the dropper, which would deadlock
                    // 3. If any other async task runs on this thread and drops a RootDir (e.g. b/c
                    //    a client closed a connection) it will deadlock.
                    crate::RootDir::new_with_dropper(self.non_meta_storage.clone(), hash, dropper)
                        .await?
                }
            };
            use std::collections::hash_map::Entry::*;
            match self.dirs.lock().expect("poisoned mutex").entry(hash) {
                // Raced with another call to serve.
                Occupied(mut o) => {
                    let old_root_dir = o.get_mut();
                    if let Some(old_root_dir) = old_root_dir.upgrade() {
                        old_root_dir
                    } else {
                        *old_root_dir = Arc::downgrade(&new_root_dir);
                        new_root_dir
                    }
                }
                Vacant(v) => {
                    v.insert(Arc::downgrade(&new_root_dir));
                    new_root_dir
                }
            }
        })
    }

    /// Returns the `Arc<RootDir>` with the given `hash`, if one exists in the cache.
    /// Otherwise returns `None`.
    /// Holding on to the returned `Arc` will keep the package open (as reported by
    /// `Self::list`).
    pub fn get(&self, hash: &fuchsia_hash::Hash) -> Option<Arc<crate::RootDir<S>>> {
        self.dirs.lock().expect("poisoned mutex").get(hash)?.upgrade()
    }

    /// Packages with live `Arc<RootDir>`s.
    /// Holding on to the returned `Arc`s will keep the packages open.
    pub fn list(&self) -> Vec<Arc<crate::RootDir<S>>> {
        self.dirs.lock().expect("poisoned mutex").values().filter_map(|v| v.upgrade()).collect()
    }

    /// Returns a callback to be given to `fuchsia_inspect::Node::record_lazy_child`.
    /// Records the package hashes and their corresponding `Arc<RootDir>` strong counts.
    pub fn record_lazy_inspect(
        &self,
    ) -> impl Fn() -> BoxFuture<'static, Result<finspect::Inspector, anyhow::Error>>
    + Send
    + Sync
    + 'static {
        let dirs = Arc::downgrade(&self.dirs);
        move || {
            let dirs = dirs.clone();
            async move {
                let inspector = finspect::Inspector::default();
                if let Some(dirs) = dirs.upgrade() {
                    let package_counts: HashMap<_, _> = {
                        let dirs = dirs.lock().expect("poisoned mutex");
                        dirs.iter().map(|(k, v)| (*k, v.strong_count() as u64)).collect()
                    };
                    let root = inspector.root();
                    let () = package_counts.into_iter().for_each(|(pkg, count)| {
                        root.record_child(pkg.to_string(), |n| n.record_uint("instances", count))
                    });
                }
                Ok(inspector)
            }
            .boxed()
        }
    }
}

/// Removes the corresponding entry from RootDirCache's self.dirs when dropped.
struct Dropper<S> {
    dirs: Weak<std::sync::Mutex<HashMap<fuchsia_hash::Hash, Weak<crate::RootDir<S>>>>>,
    hash: fuchsia_hash::Hash,
}

impl<S> Drop for Dropper<S> {
    fn drop(&mut self) {
        let Some(dirs) = self.dirs.upgrade() else {
            return;
        };
        use std::collections::hash_map::Entry::*;
        match dirs.lock().expect("poisoned mutex").entry(self.hash) {
            Occupied(o) => {
                // In case this raced with a call to serve that added a new one.
                if o.get().strong_count() == 0 {
                    o.remove_entry();
                }
            }
            // Never added because creation failed.
            Vacant(_) => (),
        };
    }
}

impl<S> std::fmt::Debug for Dropper<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dropper").field("dirs", &self.dirs).field("hash", &self.hash).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fidl_fuchsia_io as fio;
    use fuchsia_pkg_testing::PackageBuilder;
    use fuchsia_pkg_testing::blobfs::Fake as FakeBlobfs;

    #[fuchsia::test]
    async fn get_or_insert_new_entry() {
        let pkg = PackageBuilder::new("pkg-name").build().await.unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let server = RootDirCache::new(blobfs_client);

        let dir = server.get_or_insert(metafar_blob.merkle, None).await.unwrap();

        assert_eq!(server.list().len(), 1);

        drop(dir);
        assert_eq!(server.list().len(), 0);
        assert!(server.dirs.lock().expect("poisoned mutex").is_empty());
    }

    #[fuchsia::test]
    async fn closing_package_connection_closes_package() {
        let pkg = PackageBuilder::new("pkg-name").build().await.unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let server = RootDirCache::new(blobfs_client);

        let dir = server.get_or_insert(metafar_blob.merkle, None).await.unwrap();
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let scope = vfs::execution_scope::ExecutionScope::new();
        vfs::directory::serve_on(dir, fio::PERM_READABLE, scope.clone(), server_end);
        let _ = proxy
            .get_attributes(Default::default())
            .await
            .expect("directory succesfully handling requests");
        assert_eq!(server.list().len(), 1);

        drop(proxy);
        let () = scope.wait().await;
        assert_eq!(server.list().len(), 0);
        assert!(server.dirs.lock().expect("poisoned mutex").is_empty());
    }

    #[fuchsia::test]
    async fn get_or_insert_existing_entry() {
        let pkg = PackageBuilder::new("pkg-name").build().await.unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let server = RootDirCache::new(blobfs_client);

        let dir0 = server.get_or_insert(metafar_blob.merkle, None).await.unwrap();

        let dir1 = server.get_or_insert(metafar_blob.merkle, None).await.unwrap();
        assert_eq!(server.list().len(), 1);
        assert_eq!(Arc::strong_count(&server.list()[0]), 3);

        drop(dir0);
        drop(dir1);
        assert_eq!(server.list().len(), 0);
        assert!(server.dirs.lock().expect("poisoned mutex").is_empty());
    }

    #[fuchsia::test]
    async fn get_or_insert_provided_root_dir() {
        let pkg = PackageBuilder::new("pkg-name").build().await.unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let root_dir = crate::RootDir::new_raw(blobfs_client.clone(), metafar_blob.merkle, None)
            .await
            .unwrap();
        blobfs_fake.delete_blob(metafar_blob.merkle);
        let server = RootDirCache::new(blobfs_client);

        let dir = server.get_or_insert(metafar_blob.merkle, Some(root_dir)).await.unwrap();
        assert_eq!(server.list().len(), 1);

        drop(dir);
        assert_eq!(server.list().len(), 0);
        assert!(server.dirs.lock().expect("poisoned mutex").is_empty());
    }

    #[fuchsia::test]
    async fn get_or_insert_provided_root_dir_error_if_already_has_dropper() {
        let pkg = PackageBuilder::new("pkg-name").build().await.unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let root_dir =
            crate::RootDir::new_raw(blobfs_client.clone(), metafar_blob.merkle, Some(Box::new(())))
                .await
                .unwrap();
        let server = RootDirCache::new(blobfs_client);

        assert_matches!(
            server.get_or_insert(metafar_blob.merkle, Some(root_dir)).await,
            Err(crate::Error::DropperAlreadySet)
        );
        assert!(server.dirs.lock().expect("poisoned mutex").is_empty());
    }

    #[fuchsia::test]
    async fn get_or_insert_fails_if_root_dir_creation_fails() {
        let (_blobfs_fake, blobfs_client) = FakeBlobfs::new();
        let server = RootDirCache::new(blobfs_client);

        assert_matches!(
            server.get_or_insert([0; 32].into(), None).await,
            Err(crate::Error::MissingMetaFar)
        );
        assert!(server.dirs.lock().expect("poisoned mutex").is_empty());
    }

    #[fuchsia::test]
    async fn get_or_insert_concurrent_race_to_insert_new_root_dir() {
        let pkg = PackageBuilder::new("pkg-name").build().await.unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let server = RootDirCache::new(blobfs_client);

        let fut0 = server.get_or_insert(metafar_blob.merkle, None);

        let fut1 = server.get_or_insert(metafar_blob.merkle, None);

        let (res0, res1) = futures::future::join(fut0, fut1).await;
        let (dir0, dir1) = (res0.unwrap(), res1.unwrap());

        assert_eq!(server.list().len(), 1);
        assert_eq!(Arc::strong_count(&server.list()[0]), 3);

        drop(dir0);
        drop(dir1);
        assert_eq!(server.list().len(), 0);
        assert!(server.dirs.lock().expect("poisoned mutex").is_empty());
    }

    #[fuchsia::test]
    async fn inspect() {
        let pkg = PackageBuilder::new("pkg-name").build().await.unwrap();
        let (metafar_blob, _) = pkg.contents();
        let (blobfs_fake, blobfs_client) = FakeBlobfs::new();
        blobfs_fake.add_blob(metafar_blob.merkle, metafar_blob.contents);
        let server = RootDirCache::new(blobfs_client);
        let _dir = server.get_or_insert(metafar_blob.merkle, None).await.unwrap();

        let inspector = finspect::Inspector::default();
        inspector.root().record_lazy_child("open-packages", server.record_lazy_inspect());

        assert_data_tree!(inspector, root: {
            "open-packages": {
                pkg.hash().to_string() => {
                    "instances": 1u64,
                },
            }
        });
    }
}
