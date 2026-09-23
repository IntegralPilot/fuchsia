// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::frozen_index::{BaseIndex, CacheIndex};
use crate::index::PackageIndex;
use crate::upgradable_packages::UpgradablePackages;
use anyhow::{Context as _, anyhow};
use fidl_fuchsia_pkg_garbagecollector::{
    GcError as SpaceErrorCode, ManagerRequest as SpaceManagerRequest,
    ManagerRequestStream as SpaceManagerRequestStream,
};
use fidl_fuchsia_update::CommitStatusProviderProxy;
use futures::prelude::*;
use log::{error, info};
use std::collections::HashSet;
use std::sync::Arc;

pub async fn serve(
    blobfs: blobfs::Client,
    base_packages: Arc<BaseIndex>,
    cache_packages: Arc<CacheIndex>,
    upgradable_packages: Option<Arc<UpgradablePackages>>,
    package_index: Arc<async_lock::RwLock<PackageIndex>>,
    open_packages: crate::RootDirCache,
    commit_status_provider: CommitStatusProviderProxy,
    mut stream: SpaceManagerRequestStream,
) -> Result<(), anyhow::Error> {
    let event_pair = commit_status_provider
        .is_current_system_committed()
        .await
        .context("while getting event pair")?;

    while let Some(event) = stream.try_next().await? {
        let SpaceManagerRequest::Gc { responder } = event else {
            return Err(anyhow!("Unknown method called on garbage collector."));
        };
        responder.send(
            gc(
                &blobfs,
                base_packages.as_ref(),
                cache_packages.as_ref(),
                upgradable_packages.as_deref(),
                &package_index,
                &open_packages,
                &event_pair,
            )
            .await,
        )?;
    }
    Ok(())
}

async fn gc(
    blobfs: &blobfs::Client,
    base_packages: &BaseIndex,
    cache_packages: &CacheIndex,
    upgradable_packages: Option<&UpgradablePackages>,
    package_index: &Arc<async_lock::RwLock<PackageIndex>>,
    open_packages: &crate::RootDirCache,
    event_pair: &zx::EventPair,
) -> Result<(), SpaceErrorCode> {
    info!("performing gc");

    event_pair.wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST).map_err(|e| {
        match e {
            zx::Status::TIMED_OUT => {
                info!("GC is blocked pending update.");
            }
            zx::Status::CANCELED => {
                info!("Commit handle is closed, likely because we are rebooting.");
            }
            other => {
                error!("Got unexpected status {:?} while waiting on handle.", other);
            }
        }
        SpaceErrorCode::PendingCommit
    })?;

    // The primary purpose of GC is to free space to enable an OTA, so we continue if possible
    // through any errors and delete as many blobs as we can (because OTA'ing may be the only way
    // to actually fix the errors).
    async move {
        // Determine all resident blobs before locking the package index to decrease the amount of
        // time the package index lock is held. Blobs written after this are implicitly protected.
        // This is for speed and not necessary for correctness. It would still be correct if the
        // list of resident blobs was determined any time after the package index lock is taken.
        // This error can not be ignored because we wouldn't know which blobs to delete.
        let mut eligible_blobs = blobfs.list_known_blobs().await?;

        // Lock the package index until we are done deleting blobs. During resolution, required
        // blobs are added to the index before their presence in blobfs is checked, so locking
        // the index until we are done deleting blobs guarantees we will never delete a blob
        // that resolution thinks it can skip fetching.
        let package_index = package_index.read().await;

        let () = package_index.all_blobs().iter().for_each(|blob| {
            eligible_blobs.remove(blob);
        });

        let () = base_packages.list_blobs().iter().for_each(|blob| {
            eligible_blobs.remove(blob);
        });

        let () = cache_packages.list_blobs().iter().for_each(|blob| {
            eligible_blobs.remove(blob);
        });

        if let Some(upgradable_packages) = upgradable_packages {
            let () = upgradable_packages.list_blobs(blobfs).await.iter().for_each(|blob| {
                eligible_blobs.remove(blob);
            });
        }

        let () = protect_open_blobs(&mut eligible_blobs, open_packages).await;

        info!("Garbage collecting {} blobs...", eligible_blobs.len());
        let mut errors = 0;
        for (i, blob) in eligible_blobs.iter().enumerate() {
            let () = blobfs.delete_blob(blob).await.unwrap_or_else(|e| {
                error!(blob:%; "Failed to delete blob: {:#}", anyhow!(e));
                errors += 1;
            });
            if (i + 1).is_multiple_of(100) {
                info!("{} blobs deleted...", i + 1 - errors);
            }
        }
        info!("Garbage collection done. Deleted {} blobs.", eligible_blobs.len() - errors);
        Ok(())
    }
    .await
    .map_err(|e: anyhow::Error| {
        error!("Failed to perform GC operation: {:#}", e);
        SpaceErrorCode::Internal
    })
}

async fn protect_open_blobs(
    eligible_blobs: &mut HashSet<fuchsia_hash::Hash>,
    open_packages: &crate::RootDirCache,
) {
    let mut to_visit = open_packages.list();
    let mut enqueued = to_visit.iter().map(|pkg| *pkg.hash()).collect::<HashSet<_>>();
    while let Some(pkg) = to_visit.pop() {
        eligible_blobs.remove(pkg.hash());
        pkg.external_file_hashes().for_each(|h| {
            eligible_blobs.remove(h);
        });
        let subpackages = match pkg.subpackages().await {
            Ok(subpackages) => subpackages,
            Err(e) => {
                // Blobs necessary for OTA are already protected by the base index.
                error!(
                    "Could not determine subpackages of open package {}. \
                     The subpackages will NOT be protected from GC: {:#}",
                    pkg.hash(),
                    anyhow!(e)
                );
                continue;
            }
        };
        for h in subpackages.into_hashes_undeduplicated() {
            if enqueued.insert(h) {
                let sub = match open_packages.get_or_insert(h, None).await {
                    Ok(sub) => sub,
                    Err(e) => {
                        // Blobs necessary for OTA are already protected by the base index.
                        error!(
                            "Could not determine blobs of {h}, a subpackage of open package {}. \
                             The subpackage will NOT be protected from GC: {:#}",
                            pkg.hash(),
                            anyhow!(e)
                        );
                        continue;
                    }
                };
                to_visit.push(sub);
            }
        }
    }
}
