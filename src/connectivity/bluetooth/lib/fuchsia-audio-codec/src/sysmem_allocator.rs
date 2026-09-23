// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use fidl::client::QueryResponseFut;
use fidl::endpoints::{ClientEnd, Proxy};
use fidl_fuchsia_sysmem2::{
    AllocatorAllocateSharedCollectionRequest, AllocatorBindSharedCollectionRequest, AllocatorProxy,
    AllocatorSetDebugClientInfoRequest, BufferCollectionConstraints, BufferCollectionMarker,
    BufferCollectionProxy, BufferCollectionSetConstraintsRequest,
    BufferCollectionTokenDuplicateRequest, BufferCollectionTokenMarker,
    BufferCollectionWaitForAllBuffersAllocatedResponse,
    BufferCollectionWaitForAllBuffersAllocatedResult, BufferMemorySettings, NodeSetNameRequest,
};
use futures::future::{FusedFuture, Future};
use futures::task::{Context, Poll};
use futures::{FutureExt, ready};
use log::error;
use std::pin::Pin;

/// A set of buffers that have been allocated with the SysmemAllocator.
#[derive(Debug)]
pub struct SysmemAllocatedBuffers {
    buffers: Vec<zx::Vmo>,
    settings: BufferMemorySettings,
    _buffer_collection: BufferCollectionProxy,
}

#[derive(Debug)]
pub struct BufferName<'a> {
    pub name: &'a str,
    pub priority: u32,
}

#[derive(Debug)]
pub struct AllocatorDebugInfo {
    pub name: String,
    pub id: u64,
}

fn default_allocator_name() -> Result<AllocatorDebugInfo, Error> {
    let name = fuchsia_runtime::process_self().get_name()?;
    let koid = fuchsia_runtime::process_self().koid()?;
    Ok(AllocatorDebugInfo { name: name.to_string(), id: koid.raw_koid() })
}

fn set_allocator_name(
    sysmem_client: &AllocatorProxy,
    debug_info: Option<AllocatorDebugInfo>,
) -> Result<(), Error> {
    let unwrapped_debug_info = match debug_info {
        Some(x) => x,
        None => default_allocator_name()?,
    };
    Ok(sysmem_client.set_debug_client_info(&AllocatorSetDebugClientInfoRequest {
        name: Some(unwrapped_debug_info.name),
        id: Some(unwrapped_debug_info.id),
        ..Default::default()
    })?)
}

impl SysmemAllocatedBuffers {
    /// Settings of the buffers that are available through `SysmemAllocator::get`
    /// Returns None if the buffers are not allocated yet.
    pub fn settings(&self) -> &BufferMemorySettings {
        &self.settings
    }

    /// Get a VMO which has been allocated from the
    pub fn get_mut(&mut self, idx: u32) -> Option<&mut zx::Vmo> {
        let idx = idx as usize;
        return self.buffers.get_mut(idx);
    }

    /// Get the number of VMOs that have been allocated.
    /// Returns None if the allocation is not complete yet.
    pub fn len(&self) -> u32 {
        self.buffers.len().try_into().expect("buffers should fit in u32")
    }
}

/// A Future that communicates with the `fuchsia.sysmem2.Allocator` service to allocate shared
/// buffers.
pub enum SysmemAllocation {
    Pending,
    /// Waiting for the Sync response from the Allocator
    WaitingForSync {
        future: Box<QueryResponseFut<()>>,
        token_fn: Option<Box<dyn FnOnce() -> () + Send + Sync>>,
        buffer_collection: BufferCollectionProxy,
    },
    /// Waiting for the buffers to be allocated, which should eventually happen after delivering the token.
    WaitingForAllocation(
        Box<QueryResponseFut<BufferCollectionWaitForAllBuffersAllocatedResult>>,
        BufferCollectionProxy,
    ),
    /// Allocation is completed. The result here represents whether it completed successfully or an
    /// error.
    Done(Result<(), fidl_fuchsia_sysmem2::Error>),
}

impl SysmemAllocation {
    /// A pending allocation which has not been started, and will never finish.
    pub fn pending() -> Self {
        Self::Pending
    }

    /// Allocate a new shared memory collection, using `allocator` to communicate with the Allocator
    /// service. `constraints` will be used to allocate the collection. A shared collection token
    /// client end will be provided to the `token_target_fn` once the request has been synced with
    /// the collection. This token can be used with `SysmemAllocation::shared` to finish allocating
    /// the shared buffers or provided to another service to share allocation, or duplicated to
    /// share this memory with more than one other client.
    pub fn allocate<
        F: FnOnce(ClientEnd<BufferCollectionTokenMarker>) -> () + 'static + Send + Sync,
    >(
        allocator: AllocatorProxy,
        name: BufferName<'_>,
        debug_info: Option<AllocatorDebugInfo>,
        constraints: BufferCollectionConstraints,
        token_target_fn: F,
    ) -> Result<Self, Error> {
        // Ignore errors since only debug information is being sent.
        set_allocator_name(&allocator, debug_info).context("Setting alloocator name")?;
        let (client_token, client_token_request) =
            fidl::endpoints::create_proxy::<BufferCollectionTokenMarker>();
        allocator
            .allocate_shared_collection(AllocatorAllocateSharedCollectionRequest {
                token_request: Some(client_token_request),
                ..Default::default()
            })
            .context("Allocating shared collection")?;

        // Duplicate to get another BufferCollectionToken to the same collection.
        let (token, token_request) = fidl::endpoints::create_endpoints();
        client_token.duplicate(BufferCollectionTokenDuplicateRequest {
            rights_attenuation_mask: Some(fidl::Rights::SAME_RIGHTS),
            token_request: Some(token_request),
            ..Default::default()
        })?;

        client_token
            .set_name(&NodeSetNameRequest {
                priority: Some(name.priority),
                name: Some(name.name.to_string()),
                ..Default::default()
            })
            .context("set_name on BufferCollectionToken")?;

        let client_end_token = client_token.into_client_end().unwrap();

        let mut res = Self::bind(allocator, client_end_token, constraints)?;

        if let Self::WaitingForSync { token_fn, .. } = &mut res {
            *token_fn = Some(Box::new(move || token_target_fn(token)));
        }

        Ok(res)
    }

    /// Bind to a shared memory collection, using `allocator` to communicate with the Allocator
    /// service and a `token` which has already been allocated. `constraints` is set to communicate
    /// the requirements of this client.
    pub fn bind(
        allocator: AllocatorProxy,
        token: ClientEnd<BufferCollectionTokenMarker>,
        constraints: BufferCollectionConstraints,
    ) -> Result<Self, Error> {
        let (buffer_collection, collection_request) =
            fidl::endpoints::create_proxy::<BufferCollectionMarker>();
        allocator.bind_shared_collection(AllocatorBindSharedCollectionRequest {
            token: Some(token),
            buffer_collection_request: Some(collection_request),
            ..Default::default()
        })?;

        buffer_collection
            .set_constraints(BufferCollectionSetConstraintsRequest {
                constraints: Some(constraints),
                ..Default::default()
            })
            .context("sending constraints to sysmem")?;

        Ok(Self::WaitingForSync {
            future: Box::new(buffer_collection.sync()),
            token_fn: None,
            buffer_collection,
        })
    }

    /// Advances a synced collection to wait for the allocation of the buffers, after synced.
    /// Delivers the token to the target as the collection is aware of it now and can reliably
    /// detect when all tokens have been turned in and constraints have been set.
    fn synced(&mut self) -> Result<(), Error> {
        *self = match std::mem::replace(self, Self::Done(Err(fidl_fuchsia_sysmem2::Error::Invalid)))
        {
            Self::WaitingForSync { future: _, token_fn, buffer_collection } => {
                if let Some(deliver_token_fn) = token_fn {
                    deliver_token_fn();
                }
                Self::WaitingForAllocation(
                    Box::new(buffer_collection.wait_for_all_buffers_allocated()),
                    buffer_collection,
                )
            }
            _ => Self::Done(Err(fidl_fuchsia_sysmem2::Error::Invalid)),
        };
        if let Self::Done(_) = self {
            return Err(format_err!("bad state in synced"));
        }
        Ok(())
    }

    /// Finish once the allocation has completed.  Returns the buffers and marks the allocation as
    /// complete.
    fn allocated(
        &mut self,
        response_result: Result<
            BufferCollectionWaitForAllBuffersAllocatedResponse,
            fidl_fuchsia_sysmem2::Error,
        >,
    ) -> Result<SysmemAllocatedBuffers, Error> {
        let done_result = response_result.as_ref().map(|_| ()).map_err(|err| *err);
        match std::mem::replace(self, Self::Done(done_result)) {
            Self::WaitingForAllocation(_, buffer_collection) => {
                let response =
                    response_result.map_err(|err| format_err!("allocation failed: {:?}", err))?;
                let buffer_info = response.buffer_collection_info.unwrap();
                let buffers = buffer_info
                    .buffers
                    .unwrap()
                    .iter_mut()
                    .map(|buffer| buffer.vmo.take().expect("missing buffer"))
                    .collect();
                let settings = buffer_info.settings.unwrap().buffer_settings.unwrap();
                Ok(SysmemAllocatedBuffers {
                    buffers,
                    settings,
                    _buffer_collection: buffer_collection,
                })
            }
            _ => Err(format_err!("allocation complete but not in the right state")),
        }
    }
}

impl FusedFuture for SysmemAllocation {
    fn is_terminated(&self) -> bool {
        match self {
            Self::Done(_) => true,
            _ => false,
        }
    }
}

impl Future for SysmemAllocation {
    type Output = Result<SysmemAllocatedBuffers, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let s = Pin::into_inner(self);
        if let Self::WaitingForSync { future, .. } = s {
            match ready!(future.poll_unpin(cx)) {
                Err(e) => {
                    error!("SysmemAllocator error: {:?}", e);
                    return Poll::Ready(Err(e.into()));
                }
                Ok(()) => {
                    if let Err(e) = s.synced() {
                        return Poll::Ready(Err(e));
                    }
                }
            };
        }
        if let Self::WaitingForAllocation(future, _) = s {
            match ready!(future.poll_unpin(cx)) {
                Ok(response_result) => return Poll::Ready(s.allocated(response_result)),
                Err(e) => {
                    error!("SysmemAllocator waiting error: {:?}", e);
                    Poll::Ready(Err(e.into()))
                }
            }
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fidl_fuchsia_sysmem2::{
        AllocatorMarker, AllocatorRequest, BufferCollectionInfo, BufferCollectionRequest,
        BufferCollectionTokenProxy, BufferCollectionTokenRequest,
        BufferCollectionTokenRequestStream, BufferMemoryConstraints, BufferUsage, CPU_USAGE_READ,
        CoherencyDomain, Heap, SingleBufferSettings, VIDEO_USAGE_HW_DECODER, VmoBuffer,
    };
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::pin::pin;

    use crate::buffer_collection_constraints::buffer_collection_constraints_default;

    fn assert_tokens_connected(
        exec: &mut fasync::TestExecutor,
        proxy: &BufferCollectionTokenProxy,
        requests: &mut BufferCollectionTokenRequestStream,
    ) {
        let mut sync_fut = proxy.sync();

        match exec.run_until_stalled(&mut requests.next()) {
            Poll::Ready(Some(Ok(BufferCollectionTokenRequest::Sync { responder }))) => {
                responder.send().expect("respond to sync")
            }
            x => panic!("Expected vended token to be connected, got {:?}", x),
        };

        // The sync future is ready now.
        assert!(exec.run_until_stalled(&mut sync_fut).is_ready());
    }

    #[fuchsia::test]
    fn allocate_future() {
        let mut exec = fasync::TestExecutor::new();

        let (proxy, mut allocator_requests) =
            fidl::endpoints::create_proxy_and_stream::<AllocatorMarker>();

        let (sender, mut receiver) = futures::channel::oneshot::channel();

        let token_fn = move |token| {
            sender.send(token).expect("should be able to send token");
        };

        let mut allocation = SysmemAllocation::allocate(
            proxy,
            BufferName { name: "audio-codec.allocate_future", priority: 100 },
            None,
            BufferCollectionConstraints {
                usage: Some(BufferUsage {
                    cpu: Some(CPU_USAGE_READ),
                    video: Some(VIDEO_USAGE_HW_DECODER),
                    ..Default::default()
                }),
                min_buffer_count_for_camping: Some(1),
                ..Default::default()
            },
            token_fn,
        )
        .expect("starting should work");
        match exec.run_until_stalled(&mut allocator_requests.next()) {
            Poll::Ready(Some(Ok(AllocatorRequest::SetDebugClientInfo { .. }))) => (),
            x => panic!("Expected debug client info, got {:?}", x),
        };

        let mut token_requests_1 = match exec.run_until_stalled(&mut allocator_requests.next()) {
            Poll::Ready(Some(Ok(AllocatorRequest::AllocateSharedCollection {
                payload, ..
            }))) => payload.token_request.unwrap().into_stream(),
            x => panic!("Expected a shared allocation request, got {:?}", x),
        };

        let mut token_requests_2 = match exec.run_until_stalled(&mut token_requests_1.next()) {
            Poll::Ready(Some(Ok(BufferCollectionTokenRequest::Duplicate { payload, .. }))) => {
                payload.token_request.unwrap().into_stream()
            }
            x => panic!("Expected a duplication request, got {:?}", x),
        };

        let (token_client_1, mut collection_requests_1) = match exec
            .run_until_stalled(&mut allocator_requests.next())
        {
            Poll::Ready(Some(Ok(AllocatorRequest::BindSharedCollection { payload, .. }))) => (
                payload.token.unwrap().into_proxy(),
                payload.buffer_collection_request.unwrap().into_stream(),
            ),
            x => panic!("Expected Bind Shared Collection, got: {:?}", x),
        };

        match exec.run_until_stalled(&mut token_requests_1.next()) {
            Poll::Ready(Some(Ok(BufferCollectionTokenRequest::SetName { .. }))) => {}
            x => panic!("Expected setname {:?}", x),
        };

        // The token turned into the allocator for binding should be connected to the server on allocating.
        assert_tokens_connected(&mut exec, &token_client_1, &mut token_requests_1);

        match exec.run_until_stalled(&mut collection_requests_1.next()) {
            Poll::Ready(Some(Ok(BufferCollectionRequest::SetConstraints { .. }))) => {}
            x => panic!("Expected buffer constraints request, got {:?}", x),
        };

        let sync_responder = match exec.run_until_stalled(&mut collection_requests_1.next()) {
            Poll::Ready(Some(Ok(BufferCollectionRequest::Sync { responder }))) => responder,
            x => panic!("Expected a sync request, got {:?}", x),
        };

        // The sysmem allocator is now waiting for the sync from the collection

        assert!(exec.run_until_stalled(&mut allocation).is_pending());

        // When it gets a response that the collection is synced, it vends the token out
        sync_responder.send().expect("respond to sync request");

        assert!(exec.run_until_stalled(&mut allocation).is_pending());

        let token_client_2 = match receiver.try_recv() {
            Ok(Some(token)) => token.into_proxy(),
            x => panic!("Should have a token sent to the fn, got {:?}", x),
        };

        // token_client_2 should be attached to the token_requests_2 that we handed over to sysmem
        // (in the token duplicate)
        assert_tokens_connected(&mut exec, &token_client_2, &mut token_requests_2);

        // We should have received a wait for the buffers to be allocated in our collection
        const SIZE_BYTES: u64 = 1024;
        let buffer_settings = BufferMemorySettings {
            size_bytes: Some(SIZE_BYTES),
            is_physically_contiguous: Some(true),
            is_secure: Some(false),
            coherency_domain: Some(CoherencyDomain::Ram),
            heap: Some(Heap {
                heap_type: Some(bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM.into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        match exec.run_until_stalled(&mut collection_requests_1.next()) {
            Poll::Ready(Some(Ok(BufferCollectionRequest::WaitForAllBuffersAllocated {
                responder,
            }))) => {
                let single_buffer_settings = SingleBufferSettings {
                    buffer_settings: Some(buffer_settings.clone()),
                    ..Default::default()
                };
                let buffer_collection_info = BufferCollectionInfo {
                    settings: Some(single_buffer_settings),
                    buffers: Some(vec![VmoBuffer {
                        vmo: Some(zx::Vmo::create(SIZE_BYTES.into()).expect("vmo creation")),
                        vmo_usable_start: Some(0),
                        ..Default::default()
                    }]),
                    ..Default::default()
                };
                let response = BufferCollectionWaitForAllBuffersAllocatedResponse {
                    buffer_collection_info: Some(buffer_collection_info),
                    ..Default::default()
                };
                responder.send(Ok(response)).expect("send collection response")
            }
            x => panic!("Expected WaitForBuffersAllocated, got {:?}", x),
        };

        // The allocator should now be finished!
        let mut buffers = match exec.run_until_stalled(&mut allocation) {
            Poll::Pending => panic!("allocation should be done"),
            Poll::Ready(res) => res.expect("successful allocation"),
        };

        assert_eq!(1, buffers.len());
        assert!(buffers.get_mut(0).is_some());
        assert_eq!(buffers.settings(), &buffer_settings);
    }

    #[fuchsia::test]
    fn with_system_allocator() {
        let mut exec = fasync::TestExecutor::new();
        let sysmem_client = fuchsia_component::client::connect_to_protocol::<AllocatorMarker>()
            .expect("connect to allocator");

        let buffer_constraints = BufferCollectionConstraints {
            min_buffer_count: Some(2),
            buffer_memory_constraints: Some(BufferMemoryConstraints {
                min_size_bytes: Some(4096),
                ..Default::default()
            }),
            ..buffer_collection_constraints_default()
        };

        let (sender, mut receiver) = futures::channel::oneshot::channel();
        let token_fn = move |token| {
            sender.send(token).expect("should be able to send token");
        };

        let mut allocation = SysmemAllocation::allocate(
            sysmem_client.clone(),
            BufferName { name: "audio-codec.allocate_future", priority: 100 },
            None,
            buffer_constraints.clone(),
            token_fn,
        )
        .expect("start allocator");

        // Receive the token.  From here on, using the token, the test becomes the other client to
        // the Allocator sharing the memory.
        let token = loop {
            assert!(exec.run_until_stalled(&mut allocation).is_pending());
            if let Poll::Ready(x) = exec.run_until_stalled(&mut receiver) {
                break x;
            }
        };
        let token = token.expect("receive token");

        let (buffer_collection_client, buffer_collection_requests) =
            fidl::endpoints::create_proxy::<BufferCollectionMarker>();
        sysmem_client
            .bind_shared_collection(AllocatorBindSharedCollectionRequest {
                token: Some(token),
                buffer_collection_request: Some(buffer_collection_requests),
                ..Default::default()
            })
            .expect("bind okay");

        buffer_collection_client
            .set_constraints(BufferCollectionSetConstraintsRequest {
                constraints: Some(buffer_constraints),
                ..Default::default()
            })
            .expect("constraints should send okay");

        let mut allocation_fut = pin!(buffer_collection_client.wait_for_all_buffers_allocated());

        let allocation_result =
            exec.run_singlethreaded(&mut allocation_fut).expect("allocation success");

        assert!(allocation_result.is_ok());

        // Allocator should be ready now.
        let allocated_buffers = match exec.run_until_stalled(&mut allocation) {
            Poll::Ready(bufs) => bufs.expect("allocation success"),
            x => panic!("Expected ready, got {:?}", x),
        };

        let _allocator_settings = allocated_buffers.settings();

        let buffers = allocation_result.unwrap().buffer_collection_info.unwrap().buffers.unwrap();

        assert_eq!(buffers.len(), allocated_buffers.len() as usize);
    }
}
