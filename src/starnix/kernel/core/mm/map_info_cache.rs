// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use memory_pinning::ShadowProcess;
use page_buf::PageBuf;
use starnix_sync::{LockDepMutex, MapInfoCacheBufLock};
use starnix_uapi::errors::Errno;
use starnix_uapi::from_status_like_fdio;
use std::sync::Arc;

/// A singleton cache that can be used to share pinned pages across all map info queries. These
/// queries often generate lock contention in Zircon on Starnix's shared address space when the
/// buffer pages are faulted in. This type holds a long-running allocation in pinned memory to
/// get reduced Zircon VmAspace lock contention in exchange for some extra memory usage and
/// potential lock contention for /proc/pid/status and related files. Luckily it seems that
/// in practice files like /proc/pid/status are not regularly accessed concurrently.
pub struct MapInfoCache {
    buf: LockDepMutex<PageBuf<zx::MapInfo>, MapInfoCacheBufLock>,
}

const ZIRCON_NAME: zx::Name = zx::Name::new_lossy("starnix_zx_map_info_cache");

impl MapInfoCache {
    pub fn get_or_init(current_task: &CurrentTask) -> Result<Arc<Self>, Errno> {
        let kernel = current_task.kernel();
        kernel.expando.get_or_try_init(|| {
            let pinned_shadow_process = kernel.expando.get_or_try_init(|| {
                ShadowProcess::new(ZIRCON_NAME)
                    .map(InfoCacheShadowProcess)
                    .map_err(|e| from_status_like_fdio!(e))
            })?;

            let num_cache_elements = kernel.features.cached_zx_map_info_bytes as usize
                / std::mem::size_of::<zx::MapInfo>();
            Self::new(&pinned_shadow_process.0, num_cache_elements)
        })
    }

    fn new(shadow_process: &ShadowProcess, num_cache_elements: usize) -> Result<Self, Errno> {
        let buf = PageBuf::new_with_extra_vmar(num_cache_elements, shadow_process.vmar())
            .map_err(|e| from_status_like_fdio!(e))?;
        buf.set_name(&ZIRCON_NAME);

        Ok(Self { buf: buf.into() })
    }

    pub fn with_map_infos<R>(
        &self,
        vmar: &zx::Vmar,
        op: impl FnOnce(Result<&[zx::MapInfo], zx::Status>) -> R,
    ) -> R {
        let mut buf = self.buf.lock();
        match vmar.maps(buf.as_mut()) {
            Ok((maps, _, avail)) if maps.len() == avail => return op(Ok(maps)),
            Err(e) => return op(Err(e)),

            // The call succeeded but the buffer wasn't big enough, fall back to a heap allocation.
            Ok(_) => (),
        }
        // No need to hold this lock while we're using the heap instead.
        drop(buf);

        match vmar.maps_vec() {
            Ok(maps) => op(Ok(&maps)),
            Err(e) => op(Err(e)),
        }
    }
}

/// The memory pinning shadow process used for zx::MapInfo buffers.
///
/// Uses its own distinct shadow process so that it doesn't interfere with other uses of memory
/// pinning.
pub struct InfoCacheShadowProcess(ShadowProcess);

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn basic_test() {
        let shadow_process = ShadowProcess::new(zx::Name::new_lossy("testing123")).unwrap();
        let cache = MapInfoCache::new(&shadow_process, 100).unwrap();
        let maps_base_addr = cache.with_map_infos(&shadow_process.vmar(), |maps| {
            let maps = maps.unwrap();
            assert_ne!(maps, &[]);
            maps.as_ptr() as usize
        });
        let cache_base_addr = cache.buf.lock().as_mut().as_ptr() as usize;
        assert_eq!(maps_base_addr, cache_base_addr, "map infos must have been read into cache");
    }

    #[fuchsia::test]
    async fn fall_back_to_heap_over_limit() {
        let shadow_process = ShadowProcess::new(zx::Name::new_lossy("testing123")).unwrap();
        let cache = MapInfoCache::new(&shadow_process, 1).unwrap();

        // Ensure that the test process' root VMAR has more mappings than can fit in the single page
        // of the cache's buffer.
        let number_of_extra_mappings =
            (zx::system_get_page_size() as usize / std::mem::size_of::<zx::MapInfo>()) * 2;
        let vmo_to_map = zx::Vmo::create(4096).unwrap();
        for _ in 0..number_of_extra_mappings {
            fuchsia_runtime::vmar_root_self()
                .map(0, &vmo_to_map, 0, 4096, zx::VmarFlags::PERM_READ)
                .unwrap();
        }

        let maps_base_addr = cache.with_map_infos(&fuchsia_runtime::vmar_root_self(), |maps| {
            let maps = maps.unwrap();
            assert!(!maps.is_empty());
            maps.as_ptr() as usize
        });

        let cache_base_addr = cache.buf.lock().as_mut().as_ptr() as usize;
        assert_ne!(maps_base_addr, cache_base_addr, "map infos must not have been read into cache");
    }
}
