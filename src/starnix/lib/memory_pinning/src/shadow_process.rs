// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_scheduler::{
    RoleManagerMarker, RoleManagerSetRoleRequest, RoleManagerSynchronousProxy, RoleName, RoleTarget,
};
use starnix_logging::log_warn;
use starnix_uapi::errors::Errno;
use starnix_uapi::from_status_like_fdio;
use std::sync::{Arc, Weak};

/// A high-priority memory profile that (as of writing) disables reclamation for any mappings it
/// contains.
const MEMORY_ROLE: &str = "fuchsia.starnix.pinned_memory";

/// Provides a hack for truly pinning memory in the absence of partial VMAR profiles or an
/// analogous feature (https://fxbug.dev/446265172). The pins produced by this type will stay
/// pinned even under critical memory pressure levels and should be used with extreme care.
///
/// Requires access to the `fuchsia.scheduler.RoleManager` protocol capability to actually pin
/// memory.
#[derive(Debug)]
pub struct ShadowProcess {
    // Keep the process alive but we're never going to start it.
    process: zx::Process,
    vmar: Arc<zx::Vmar>,
}

impl ShadowProcess {
    /// Create a new shadow process for pinning memory. Connects to `fuchsia.scheduler.RoleManager`
    /// in the process' namespace.
    pub fn new(name: zx::Name) -> Result<Self, zx::Status> {
        let role_manager =
            fuchsia_component::client::connect_to_protocol_sync::<RoleManagerMarker>()
                .expect("this can only fail if a process' namespace is broken");
        Self::from_role_manager(name, role_manager)
    }

    fn from_role_manager(
        name: zx::Name,
        role_manager: RoleManagerSynchronousProxy,
    ) -> Result<Self, zx::Status> {
        let (process, vmar) =
            zx::Process::create(&fuchsia_runtime::job_default(), name, Default::default())?;
        let vmar_dupe = vmar.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        if let Err(e) = role_manager.set_role(
            RoleManagerSetRoleRequest {
                target: Some(RoleTarget::Vmar(vmar_dupe)),
                role: Some(RoleName { role: MEMORY_ROLE.to_string() }),
                ..Default::default()
            },
            zx::MonotonicInstant::INFINITE,
        ) {
            log_warn!(e:%, name:%; "Unable to set role for memory pin shadow process' vmar.");
        }

        Ok(Self { process, vmar: Arc::new(vmar) })
    }

    /// Pin the provided range of the provided VMO to ensure those pages stay resident under
    /// memory pressure.
    pub fn pin_pages(
        &self,
        vmo: &zx::Vmo,
        offset: u64,
        length: usize,
    ) -> Result<Arc<PinnedMapping>, Errno> {
        let base = self
            .vmar
            .map(0, vmo, offset, length, zx::VmarFlags::PERM_READ)
            .map_err(|e| from_status_like_fdio!(e))?;
        Ok(Arc::new(PinnedMapping { vmar: Arc::downgrade(&self.vmar), base, length }))
    }

    /// Return a handle to the VMAR where all mappings are pinned.
    pub fn vmar(&self) -> Arc<zx::Vmar> {
        self.vmar.clone()
    }
}

impl Drop for ShadowProcess {
    fn drop(&mut self) {
        use zx::Task;

        // Ensure the process exits so that it doesn't confuse the kernel's shutdown logic.
        self.process.kill().expect("must be able to kill process we created");
    }
}

/// A token for a region of pinned memory. Will unpin the memory when dropped.
#[derive(Clone, Debug)]
pub struct PinnedMapping {
    vmar: Weak<zx::Vmar>,
    base: usize,
    length: usize,
}

impl Drop for PinnedMapping {
    fn drop(&mut self) {
        if let Some(vmar) = self.vmar.upgrade() {
            // SAFETY: this address is not observable outside this module and it is just a key into
            // the high priority VMAR for this module's purposes. No pointers or references have
            // been created pointing into this mapping which makes it sound to unmap.
            if let Err(e) = unsafe { vmar.unmap(self.base, self.length) } {
                log_warn!(e:%; "Failed to unmap mlock() pin mapping.");
            }
        }
    }
}

impl std::cmp::PartialEq for PinnedMapping {
    fn eq(&self, rhs: &Self) -> bool {
        Weak::ptr_eq(&self.vmar, &rhs.vmar) && self.base == rhs.base && self.length == rhs.length
    }
}
impl std::cmp::Eq for PinnedMapping {}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_scheduler::{RoleManagerRequest, RoleManagerSetRoleResponse};
    use futures::StreamExt;

    #[fuchsia::test]
    fn create_without_role_manager_succeeds() {
        // There's no RoleManager available in the unit test environment.
        let _shadow_process = ShadowProcess::new(zx::Name::new_lossy("noop")).unwrap();
    }

    #[fuchsia::test]
    async fn creation_sets_role() {
        let (role_manager_client, mut role_manager_server) =
            fidl::endpoints::create_sync_proxy_and_stream::<RoleManagerMarker>();

        // Creating a ShadowProcess blocks the calling thread until the role manager replies, spawn
        // a separate thread.
        let (send_vmar_koid, recv_vmar_koid) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let shadow_process = ShadowProcess::from_role_manager(
                zx::Name::new_lossy("role_manager_test"),
                role_manager_client,
            )
            .unwrap();
            send_vmar_koid.send(shadow_process.vmar.koid().unwrap()).unwrap();
        });

        match role_manager_server.next().await.unwrap().unwrap() {
            RoleManagerRequest::SetRole { payload, responder } => {
                responder.send(Ok(RoleManagerSetRoleResponse::default())).unwrap();
                let shadow_vmar_koid: zx::Koid = recv_vmar_koid.await.unwrap();

                let received_vmar_koid = match &payload.target {
                    Some(RoleTarget::Vmar(vmar)) => vmar.koid().unwrap(),
                    other => panic!("unexpected SetRole target {other:#?}"),
                };
                assert_eq!(shadow_vmar_koid, received_vmar_koid);
                assert_eq!(payload.role, Some(RoleName { role: MEMORY_ROLE.to_string() }),);
            }
            other => panic!("unexpected SetRole request {other:?}"),
        }
    }

    #[fuchsia::test]
    fn vmo_is_mapped_in_shadow_vmar() {
        let shadow_process = ShadowProcess::new(zx::Name::new_lossy("vmo_mapping_test")).unwrap();

        let get_shadow_mappings = || {
            shadow_process
                .vmar
                .maps_vec()
                .unwrap()
                .into_iter()
                .filter_map(|info| info.details().as_mapping().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        };
        assert_eq!(get_shadow_mappings(), &[], "VMAR should be empty before any pinning");

        // Initialize a VMO and populate its pages
        let to_map = zx::Vmo::create(8192).unwrap();
        to_map.write(&[1u8; 8192][..], 0).unwrap();

        // Pin the VMO pages
        let pinned_mapping = shadow_process.pin_pages(&to_map, 0, 8192).unwrap();
        let mappings_after_pinning = get_shadow_mappings();
        assert_eq!(mappings_after_pinning.len(), 1, "there should only be one mapping in VMAR");

        // Check the mapping in the shadow process' VMAR. It should be a read-only mapping and
        // be fully committed & populated.
        let pinned_mapping_info = &mappings_after_pinning[0];
        assert_eq!(pinned_mapping_info.mmu_flags, zx::VmarFlagsExtended::PERM_READ);
        assert_eq!(pinned_mapping_info.vmo_koid, to_map.koid().unwrap());
        assert_eq!(pinned_mapping_info.vmo_offset, 0);
        assert_eq!(pinned_mapping_info.committed_bytes, 8192);
        assert_eq!(pinned_mapping_info.populated_bytes, 8192);

        drop(pinned_mapping);
        assert_eq!(get_shadow_mappings(), &[], "dropping PinnedMap must clean up");
    }
}
