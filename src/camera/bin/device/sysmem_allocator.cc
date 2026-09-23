// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/device/sysmem_allocator.h"

#include <fidl/fuchsia.sysmem2/cpp/hlcpp_conversion.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/defer.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/scope.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/sysmem-version/sysmem-version.h>

namespace camera {
namespace {

constexpr uint32_t kNamePriority = 30;  // Higher than Scenic but below the maximum.

// Returns a promise that completes after calling |WaitForBuffersAllocated| on the provided
// BufferCollection.
//
// The |collection| is consumed by this operation and will be closed upon both success and failure.
fpromise::promise<BufferCollectionWithLifetime, zx_status_t> WaitForAllBuffersAllocated(
    fuchsia::sysmem2::BufferCollectionPtr collection) {
  // Move the bridge completer into a shared_ptr so that we can share the completer between the
  // FIDL error handler and the WaitForBuffersAllocated callback.
  fpromise::bridge<BufferCollectionWithLifetime, zx_status_t> bridge;
  auto completer = std::make_shared<fpromise::completer<BufferCollectionWithLifetime, zx_status_t>>(
      std::move(bridge.completer));
  std::weak_ptr<fpromise::completer<BufferCollectionWithLifetime, zx_status_t>> weak_completer(
      completer);
  collection.set_error_handler([completer](zx_status_t status) {
    // After calling SetConstraints, allocation may fail. This results in WaitForBuffersAllocated
    // returning NO_MEMORY followed by channel closure. Because the client may observe these in
    // either order, treat channel closure as if it were NO_MEMORY.
    FX_CHECK(status != ZX_OK);
    completer->complete_error(status == ZX_ERR_PEER_CLOSED ? ZX_ERR_NO_MEMORY : status);
  });

  zx::eventpair deallocation_complete_client, deallocation_complete_server;
  zx::eventpair::create(/*options=*/0, &deallocation_complete_client,
                        &deallocation_complete_server);
  fuchsia::sysmem2::BufferCollectionAttachLifetimeTrackingRequest attach_lifetime_request;
  attach_lifetime_request.set_server_end(std::move(deallocation_complete_server));
  attach_lifetime_request.set_buffers_remaining(0);
  collection->AttachLifetimeTracking(std::move(attach_lifetime_request));

  collection->WaitForAllBuffersAllocated(
      [weak_completer, deallocation_complete = std::move(deallocation_complete_client)](
          fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result result) mutable {
        auto completer = weak_completer.lock();
        if (!completer) {
          return;
        }
        if (result.is_framework_err()) {
          completer->complete_error(ZX_ERR_INTERNAL);
          return;
        }
        if (result.is_err()) {
          zx_status_t v1_status = sysmem::V1CopyFromV2Error(fidl::HLCPPToNatural(result.err()));
          completer->complete_error(v1_status);
          return;
        }
        BufferCollectionWithLifetime collection_lifetime;
        collection_lifetime.buffers =
            std::move(*result.response().mutable_buffer_collection_info());
        collection_lifetime.deallocation_complete = std::move(deallocation_complete);
        completer->complete_ok(std::move(collection_lifetime));
      });
  return bridge.consumer.promise().inspect(
      [collection = std::move(collection)](
          const fpromise::result<BufferCollectionWithLifetime, zx_status_t>& result) mutable {
        if (collection) {
          collection->Release();
          collection = nullptr;
        }
      });
}

}  // namespace

SysmemAllocator::SysmemAllocator(fuchsia::sysmem2::AllocatorHandle allocator)
    : allocator_(allocator.Bind()) {}

fpromise::promise<BufferCollectionWithLifetime, zx_status_t> SysmemAllocator::BindSharedCollection(
    fuchsia::sysmem2::BufferCollectionTokenHandle token,
    fuchsia::sysmem2::BufferCollectionConstraints constraints, std::string name) {
  TRACE_DURATION("camera", "SysmemAllocator::BindSharedCollection");
  // We expect sysmem to have free space, so bind the provided token now.
  fuchsia::sysmem2::BufferCollectionPtr collection;

  fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
  bind_shared_request.set_token(std::move(token));
  bind_shared_request.set_buffer_collection_request(collection.NewRequest());
  allocator_->BindSharedCollection(std::move(bind_shared_request));

  fuchsia::sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.set_priority(kNamePriority);
  set_name_request.set_name(std::move(name));
  collection->SetName(std::move(set_name_request));

  // We only need to observe the created collection, not take any of its camping buffers.
  fuchsia::sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  auto& observer_constraints = *set_constraints_request.mutable_constraints();
  observer_constraints.mutable_usage()->set_none(fuchsia::sysmem2::NONE_USAGE);
  collection->SetConstraints(std::move(set_constraints_request));

  return WaitForAllBuffersAllocated(std::move(collection)).wrap_with(scope_);
}

}  // namespace camera
