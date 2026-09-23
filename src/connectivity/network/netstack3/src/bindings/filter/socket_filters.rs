// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bindings::Ctx;
use crate::bindings::bpf::{CgroupSkbProgram, EbpfManager, ValidVerifiedProgram};
use fidl::endpoints::Responder;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_table_validation::ValidFidlTable;
use futures::TryStreamExt;
use log::error;
use std::collections::BTreeSet;

// State of a single `SocketControl` connection and detaches attached hook
// when the connection is dropped.
struct SocketControlConnectionState<'a> {
    ebpf_manager: &'a EbpfManager,

    // Set of the hooks that were attached by the current `SocketControl`
    // client.
    attached: BTreeSet<fnet_filter::SocketHook>,
}

impl<'a> SocketControlConnectionState<'a> {
    fn attach_socket_filter(
        &mut self,
        hook: fnet_filter::SocketHook,
        program: ValidVerifiedProgram,
    ) -> Result<(), fnet_filter::SocketControlAttachEbpfProgramError> {
        // If the program is already installed by the same `SocketControl`
        // client then it can be replaced.
        let replace = self.attached.contains(&hook);

        CgroupSkbProgram::new(program, &self.ebpf_manager.maps_cache())
            .map_err(|e| e.into())
            .and_then(|program| match hook {
                fnet_filter::SocketHook::Egress => {
                    self.ebpf_manager.set_egress_hook(Some(program), replace).map_err(|e| e.into())
                }
                fnet_filter::SocketHook::Ingress => {
                    self.ebpf_manager.set_ingress_hook(Some(program), replace).map_err(|e| e.into())
                }
                fnet_filter::SocketHook::__SourceBreaking { unknown_ordinal } => {
                    log::warn!("Unsupported SocketHook ordinal: {}", unknown_ordinal);
                    Err(fnet_filter::SocketControlAttachEbpfProgramError::NotSupported)
                }
            })?;

        if !replace {
            let inserted = self.attached.insert(hook);
            assert!(inserted);
        }

        Ok(())
    }

    fn detach_socket_filter(
        &mut self,
        hook: fnet_filter::SocketHook,
    ) -> Result<(), fnet_filter::SocketControlDetachEbpfProgramError> {
        if !self.attached.remove(&hook) {
            return Err(fnet_filter::SocketControlDetachEbpfProgramError::NotFound);
        }
        self.detach_internal(hook);
        Ok(())
    }

    fn detach_internal(&self, hook: fnet_filter::SocketHook) {
        let result = match hook {
            fnet_filter::SocketHook::Egress => self.ebpf_manager.set_egress_hook(None, false),
            fnet_filter::SocketHook::Ingress => self.ebpf_manager.set_ingress_hook(None, false),
            fnet_filter::SocketHook::__SourceBreaking { .. } => {
                unreachable!()
            }
        };
        result.expect("Failed to detach eBPF program");
    }
}

impl<'a> Drop for SocketControlConnectionState<'a> {
    fn drop(&mut self) {
        for hook in self.attached.iter() {
            self.detach_internal(*hook)
        }
    }
}

#[derive(ValidFidlTable)]
#[fidl_table_src(fnet_filter::AttachEbpfProgramOptions)]
#[fidl_table_strict]
struct ValidAttachEbpfProgramOptions {
    hook: fnet_filter::SocketHook,
    program: ValidVerifiedProgram,
}

pub(crate) async fn serve_socket_control(
    mut stream: fnet_filter::SocketControlRequestStream,
    ctx: Ctx,
) -> Result<(), fidl::Error> {
    use fnet_filter::SocketControlRequest;

    let mut state = SocketControlConnectionState {
        ebpf_manager: &ctx.bindings_ctx().ebpf_manager,
        attached: BTreeSet::new(),
    };

    while let Some(request) = stream.try_next().await? {
        match request {
            SocketControlRequest::AttachEbpfProgram { payload, responder } => {
                let ValidAttachEbpfProgramOptions { hook, program } = match payload.try_into() {
                    Ok(r) => r,
                    Err(e) => {
                        error!("fatal error serving fuchsia.net.filter.SocketControl: {:?}", e);
                        responder.control_handle().shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                        break;
                    }
                };

                responder.send(state.attach_socket_filter(hook, program))?;
            }
            SocketControlRequest::DetachEbpfProgram { hook, responder } => {
                responder.send(state.detach_socket_filter(hook))?;
            }
        }
    }
    Ok(())
}
