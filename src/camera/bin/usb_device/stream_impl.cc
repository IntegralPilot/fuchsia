// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/usb_device/stream_impl.h"

#include <fidl/fuchsia.sysmem2/cpp/hlcpp_conversion.h>
#include <fuchsia/images2/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/function.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/trace/event.h>
#include <lib/zx/time.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cerrno>

#include "src/camera/bin/usb_device/messages.h"
#include "src/camera/bin/usb_device/size_util.h"
#include "src/camera/bin/usb_device/uvc_hack.h"
#include "src/lib/fsl/handles/object_info.h"

namespace camera {
namespace {

template <typename RetT_, typename ErrT_ = void>
using promise = fpromise::promise<RetT_, ErrT_>;

template <typename RetT_, typename ErrT_ = void>
using result = fpromise::result<RetT_, ErrT_>;

template <typename RetT_, typename ErrT_ = void>
using bridge = fpromise::bridge<RetT_, ErrT_>;

using fpromise::join_promise_vector;
using fpromise::make_error_promise;
using fpromise::make_ok_promise;
using fpromise::make_promise;
using fpromise::make_result_promise;

using fuchsia::camera::FrameAvailableEvent;
using fuchsia::camera::FrameRate;
using fuchsia::camera::FrameStatus;
using fuchsia::camera::StreamHandle;
using fuchsia::camera::VideoFormat;
using fuchsia::camera3::FrameInfo2;

using fuchsia::sysmem2::BufferCollectionConstraints;
using fuchsia::sysmem2::BufferCollectionPtr;
using fuchsia::sysmem2::BufferCollectionTokenHandle;
using fuchsia::sysmem2::BufferCollectionTokenPtr;

inline size_t ROUNDUP(size_t size, size_t page_size) {
  // Caveat: Only works for power of 2 page sizes.
  return (size + (page_size - 1)) & ~(page_size - 1);
}

// Allocate driver-facing buffer collection.
zx::result<fuchsia::sysmem::BufferCollectionInfo> Gralloc(VideoFormat video_format,
                                                          uint32_t num_buffers) {
  fuchsia::sysmem::BufferCollectionInfo buffer_collection_info;

  size_t buffer_size =
      ROUNDUP(video_format.format.height * video_format.format.planes[0].bytes_per_row,
              zx_system_get_page_size());
  buffer_collection_info.buffer_count = num_buffers;
  buffer_collection_info.vmo_size = buffer_size;
  buffer_collection_info.format.image = video_format.format;
  zx_status_t status;
  for (uint32_t i = 0; i < num_buffers; ++i) {
    status = zx::vmo::create(buffer_size, 0, &buffer_collection_info.vmos[i]);
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to allocate Buffer Collection";
      return zx::error(status);
    }
  }
  return zx::ok(std::move(buffer_collection_info));
}

}  // namespace

StreamImpl::StreamImpl(async_dispatcher_t* dispatcher,
                       const fuchsia::camera3::StreamProperties2& properties,
                       fidl::InterfaceRequest<fuchsia::camera3::Stream> request,
                       StreamRequestedCallback on_stream_requested,
                       AllocatorBindSharedCollectionCallback allocator_bind_shared_collection,
                       RegisterDeallocationEvent register_deallocation_event,
                       NoClientsCallback on_no_clients, std::optional<std::string> description)
    : ActorBase(dispatcher, scope_),
      dispatcher_(dispatcher),
      properties_(properties),
      on_stream_requested_(std::move(on_stream_requested)),
      allocator_bind_shared_collection_(std::move(allocator_bind_shared_collection)),
      register_deallocation_event_(std::move(register_deallocation_event)),
      on_no_clients_(std::move(on_no_clients)),
      description_(description.value_or("<unknown>")) {
  current_resolution_ = ConvertToSize(properties.image_format());
  OnNewRequest(std::move(request));
}

promise<void> StreamImpl::CloseAllClients(zx_status_t status) {
  bridge<void> bridge;
  Schedule([this, status, completer = std::move(bridge.completer)]() mutable -> promise<void> {
    std::vector<promise<void>> clients_removed_promises;
    for (auto& [id, client] : clients_) {
      client->CloseConnection(status);
      clients_removed_promises.push_back(RemoveClient(id));
    }

    return join_promise_vector(std::move(clients_removed_promises))
        .and_then([completer = std::move(completer)](std::vector<result<void>>& results) mutable {
          completer.complete_ok();
        });
  });
  return bridge.consumer.promise();
}

void StreamImpl::OnNewRequest(fidl::InterfaceRequest<fuchsia::camera3::Stream> request) {
  auto nonce = TRACE_NONCE();
  TRACE_ASYNC_BEGIN("camera", "StreamImpl::OnNewRequest", nonce);
  Schedule([this, nonce, request = std::move(request)]() mutable {
    auto client = std::make_unique<Client>(*this, client_id_next_, std::move(request));
    client->ReceiveResolution(current_resolution_);
    client->ReceiveCropRegion(nullptr);
    clients_.emplace(client_id_next_++, std::move(client));
    TRACE_ASYNC_END("camera", "StreamImpl::OnNewRequest", nonce);
  });
}

promise<void> StreamImpl::RemoveClient(uint64_t id) {
  return make_promise([this, id]() -> promise<void> {
    TRACE_DURATION("camera", "StreamImpl::RemoveClient");
    clients_.erase(id);
    if (clients_.empty()) {
      return Cleanup(ZX_OK, "Last client disconnected.");
    }
    return make_ok_promise();
  });
}

// A new incoming buffer (already processed to be client-facing format) is available to be sent.
// Enqueue this buffer into the outbounding queue, creating the necessary fence event pair.
void StreamImpl::OnFrameAvailable(FrameAvailableEvent info) {
  TRACE_DURATION("camera", "StreamImpl::OnFrameAvailable");

  if (info.frame_status != FrameStatus::OK) {
    FX_LOGS(WARNING) << description_
                     << ": Driver reported a bad frame. This will not be reported to clients.";
    return;
  }

  if (frame_waiters_.find(info.buffer_id) != frame_waiters_.end()) {
    FX_LOGS(WARNING) << description_
                     << ": Driver sent a frame that was already in use (ID = " << info.buffer_id
                     << "). This frame will not be sent to clients.";
    return;
  }

  // Faking timestamp here.
  // TODO(ernesthua) - Need to add timestamps in pipeline (maybe down in usb_video) on merge back.
  // Seems inappropriate to make up one here.
  uint64_t capture_timestamp = static_cast<uint64_t>(zx_clock_get_monotonic());

  // The frame is valid and camera is unmuted, so increment the frame counter.
  ++frame_counter_;

  // Discard the frame if there are too many frames outstanding.
  // TODO(https://fxbug.dev/42143447): Recycle LRU frames.
  if (frame_waiters_.size() == max_camping_buffers_) {
    // record_.FrameDropped(cobalt::FrameDropReason::kTooManyFramesInFlight);
    ReleaseClientFrame(info.buffer_id);
    FX_LOGS(WARNING) << description_ << ": Max camping buffers!";
    return;
  }

  // Construct the frame info and create a release fence per client.
  std::vector<zx::eventpair> fences;
  for (auto& [id, client] : clients_) {
    if (!client->Participant()) {
      continue;
    }
    zx::eventpair fence;
    zx::eventpair release_fence;
    ZX_ASSERT(zx::eventpair::create(0u, &fence, &release_fence) == ZX_OK);
    fences.push_back(std::move(fence));
    FrameInfo2 frame;
    frame.set_buffer_index(info.buffer_id);
    frame.set_frame_counter(frame_counter_);
    frame.set_timestamp(capture_timestamp);
    frame.set_capture_timestamp(capture_timestamp);
    frame.set_release_fence(std::move(release_fence));
    client->AddFrame(std::move(frame));
  }

  // No participating clients exist. Release the frame immediately.
  if (fences.empty()) {
    return;
  }

  // Queue a waiter so that when the client end of the fence is released, the frame is released back
  // to the client-facing pool.
  // ZX_ASSERT(frame_waiters_.size() <= max_camping_buffers_);
  frame_waiters_[info.buffer_id] =
      std::make_unique<FrameWaiter>(dispatcher_, std::move(fences), [this, index = info.buffer_id] {
        ReleaseClientFrame(index);
        frame_waiters_.erase(index);
      });
}

promise<BufferCollectionTokenPtr> StreamImpl::TokenSync(BufferCollectionTokenPtr token) {
  bridge<BufferCollectionTokenPtr> bridge;
  token->Sync([token = std::move(token), completer = std::move(bridge.completer)](
                  fuchsia::sysmem2::Node_Sync_Result result) mutable {
    completer.complete_ok(std::move(token));
  });
  return bridge.consumer.promise();
}

promise<void> StreamImpl::BufferCollectionSync(BufferCollectionPtr& collection) {
  bridge<void> bridge;
  collection->Sync(
      [completer = std::move(bridge.completer)](fuchsia::sysmem2::Node_Sync_Result result) mutable {
        completer.complete_ok();
      });
  return bridge.consumer.promise();
}

promise<fuchsia::sysmem2::BufferCollectionInfo, zx_status_t>
StreamImpl::WaitForClientBufferCollectionAllocated() {
  bridge<fuchsia::sysmem2::BufferCollectionInfo, zx_status_t> bridge;
  client_buffer_collection_->WaitForAllBuffersAllocated(
      [completer = std::move(bridge.completer)](
          fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result result) mutable {
        if (result.is_framework_err()) {
          FX_PLOGS(ERROR, fidl::ToUnderlying(result.framework_err()))
              << "Failed to allocate sysmem buffers (framework_err).";
          completer.complete_error(ZX_ERR_INTERNAL);
          return;
        }
        if (result.is_err()) {
          FX_PLOGS(ERROR, static_cast<uint32_t>(result.err()))
              << "Failed to allocate sysmem buffers (sysmem2.Error).";
          zx_status_t v1_status = sysmem::V1CopyFromV2Error(fidl::HLCPPToNatural(result.err()));
          completer.complete_error(v1_status);
          return;
        }
        completer.complete_ok(std::move(*result.response().mutable_buffer_collection_info()));
      });
  return bridge.consumer.promise();
}

promise<std::optional<BufferCollectionTokenPtr>> StreamImpl::SetClientParticipation(
    uint64_t id, fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token_handle) {
  return make_promise([this, id, token_handle = std::move(token_handle)]() mutable
                          -> promise<std::optional<BufferCollectionTokenPtr>> {
    auto it = clients_.find(id);
    if (it == clients_.end()) {
      FX_LOGS(ERROR) << description_ << ": Client " << id << " not found.";
      if (token_handle) {
        token_handle.BindSync()->Release();
      }
      ZX_DEBUG_ASSERT(false);
    }

    // Decide whether we are a parctipant or not.
    auto& client = it->second;
    client->Participant() = !!token_handle;

    if (!token_handle)
      return make_ok_promise<std::optional<BufferCollectionTokenPtr>>(std::nullopt);

    frame_waiters_.clear();

    BufferCollectionTokenPtr token;
    token.Bind(std::move(token_handle));

    return TokenSync(std::move(token))
        .and_then(
            [](BufferCollectionTokenPtr& token) -> result<std::optional<BufferCollectionTokenPtr>> {
              return fpromise::ok(std::move(token));
            });
  });
}

promise<fuchsia::sysmem::BufferCollectionInfo, zx_status_t> StreamImpl::InitBufferCollections(
    BufferCollectionTokenPtr token) {
  return InitializeClientSharedCollection(std::move(token))
      .and_then([this]() { return BufferCollectionSync(client_buffer_collection_); })
      .and_then([this]() { return SetClientBufferCollectionConstraints(); })
      .then([this](result<void>& /*result*/) { return WaitForClientBufferCollectionAllocated(); })
      .and_then([this](fuchsia::sysmem2::BufferCollectionInfo& result) {
        return RegisterClientBufferCollectionDeallocationEvent()
            .and_then([this, result = std::move(result)]() mutable {
              return InitializeClientBuffers(std::move(result));
            })
            .and_then([]() {
              fuchsia::camera::VideoFormat video_format;
              UvcHackGetServerBufferVideoFormat(&video_format);
              return fpromise::ok(std::move(video_format));
            })
            .then([this](fpromise::result<fuchsia::camera::VideoFormat>& result) {
              return AllocateDriverBufferCollection(std::move(result.value()));
            });
      })
      .or_else([](zx_status_t& status) {
        return make_result_promise<fuchsia::sysmem::BufferCollectionInfo, zx_status_t>(
            fpromise::error(status));
      });
}

void StreamImpl::OnError(zx_status_t status, const std::string& message) {
  Schedule(Cleanup(status, message));
}

promise<void> StreamImpl::Cleanup(zx_status_t status, const std::string& message) {
  return make_promise([this, status, message]() {
    if (status == ZX_OK) {
      FX_PLOGS(INFO, status) << description_ << message;
    } else {
      FX_PLOGS(ERROR, status) << description_ << message;
    }

    // Close all clients.
    clients_.clear();

    // Unmap VMOs and close the VMO handles.
    if (client_buffer_collection_info_.has_settings()) {
      auto vmo_size = client_buffer_collection_info_.settings().buffer_settings().size_bytes();
      for (uint32_t buffer_id = 0; buffer_id < client_buffer_collection_info_.buffers().size();
           buffer_id++) {
        uintptr_t vmo_virt_addr = client_buffer_id_to_virt_addr_[buffer_id];
        auto status = zx::vmar::root_self()->unmap(vmo_virt_addr, vmo_size);
        ZX_ASSERT(status == ZX_OK);
        zx::vmo& vmo =
            *client_buffer_collection_info_.mutable_buffers()->at(buffer_id).mutable_vmo();
        vmo.reset();
      }
    }
    client_buffer_id_to_virt_addr_.clear();
    if (client_buffer_collection_) {
      client_buffer_collection_->Release();
    }

    // Cause this stream to be destroyed.
    return on_no_clients_();
  });
}

promise<void> StreamImpl::InitializeClientSharedCollection(BufferCollectionTokenPtr token) {
  return make_promise([this, token = std::move(token)]() mutable -> promise<void> {
    // Duplicate the token for every participating client.
    for (auto& client_i : clients_) {
      if (client_i.second->Participant()) {
        BufferCollectionTokenHandle client_token;
        fuchsia::sysmem2::BufferCollectionTokenDuplicateRequest dup_request;
        dup_request.set_rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS);
        dup_request.set_token_request(client_token.NewRequest());
        token->Duplicate(std::move(dup_request));
        client_i.second->ReceiveBufferCollection(std::move(client_token));
      }
    }

    return allocator_bind_shared_collection_(std::move(token),
                                             client_buffer_collection_.NewRequest())
        .and_then([this]() {
          client_buffer_collection_.set_error_handler([this](zx_status_t status) {
            OnError(status, ":sysmem disconnected unexpectedly.");
          });
          constexpr uint32_t kNamePriority = 30;
          std::string name("fake");
          fuchsia::sysmem2::NodeSetNameRequest set_name_request;
          set_name_request.set_priority(kNamePriority);
          set_name_request.set_name(std::move(name));
          client_buffer_collection_->SetName(std::move(set_name_request));
        });
  });
}

promise<void> StreamImpl::SetClientBufferCollectionConstraints() {
  return make_promise([this]() {
    BufferCollectionConstraints buffer_collection_constraints;
    UvcHackGetClientBufferCollectionConstraints(&buffer_collection_constraints);
    fuchsia::sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.set_constraints(std::move(buffer_collection_constraints));
    client_buffer_collection_->SetConstraints(std::move(set_constraints_request));
  });
}

promise<void> StreamImpl::RegisterClientBufferCollectionDeallocationEvent() {
  return make_promise([this]() {
    zx::eventpair deallocation_event_client, deallocation_event_server;
    zx::eventpair::create(/*options=*/0, &deallocation_event_client, &deallocation_event_server);
    fuchsia::sysmem2::BufferCollectionAttachLifetimeTrackingRequest attach_lifetime_request;
    attach_lifetime_request.set_server_end(std::move(deallocation_event_server));
    attach_lifetime_request.set_buffers_remaining(0);
    client_buffer_collection_->AttachLifetimeTracking(std::move(attach_lifetime_request));
    register_deallocation_event_(std::move(deallocation_event_client));
  });
}

promise<void> StreamImpl::InitializeClientBuffers(
    fuchsia::sysmem2::BufferCollectionInfo buffer_collection_info) {
  return make_promise([this, buffer_collection_info = std::move(buffer_collection_info)]() mutable {
    for (uint32_t buffer_id = 0; buffer_id < buffer_collection_info.buffers().size(); buffer_id++) {
      // TODO(b/204456599) - Use VmoMapper helper class instead.
      const zx::vmo& vmo = buffer_collection_info.buffers()[buffer_id].vmo();
      auto vmo_size = buffer_collection_info.settings().buffer_settings().size_bytes();
      uintptr_t vmo_virt_addr = 0;
      auto status =
          zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0 /* vmar_offset */, vmo,
                                     0 /* vmo_offset */, vmo_size, &vmo_virt_addr);
      ZX_ASSERT(status == ZX_OK);
      client_buffer_id_to_virt_addr_[buffer_id] = vmo_virt_addr;
    }

    // Initially place all buffers into free pool.
    for (uint32_t buffer_id = 0; buffer_id < buffer_collection_info.buffers().size(); buffer_id++) {
      ReleaseClientFrame(buffer_id);
    }

    // Remember this buffer collection - must survive until no longer in use.
    client_buffer_collection_info_ = std::move(buffer_collection_info);
  });
}

promise<fuchsia::sysmem::BufferCollectionInfo, zx_status_t>
StreamImpl::AllocateDriverBufferCollection(fuchsia::camera::VideoFormat video_format) {
  return make_promise([this, video_format = std::move(video_format)]() mutable
                          -> result<fuchsia::sysmem::BufferCollectionInfo, zx_status_t> {
    auto buffer_or = Gralloc(std::move(video_format), 8);
    if (buffer_or.is_error()) {
      FX_LOGS(ERROR) << "Couldn't allocate. status: " << buffer_or.error_value();
      return fpromise::error(buffer_or.error_value());
    }

    fuchsia::sysmem::BufferCollectionInfo buffer_collection_info = std::move(*buffer_or);

    // Map all driver buffers.
    for (uint32_t buffer_id = 0; buffer_id < buffer_collection_info.buffer_count; buffer_id++) {
      // TODO(b/204456599) - Use VmoMapper helper class instead.
      const zx::vmo& vmo = buffer_collection_info.vmos[buffer_id];
      auto vmo_size = buffer_collection_info.vmo_size;
      uintptr_t vmo_virt_addr = 0;
      auto status =
          zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0 /* vmar_offset */, vmo,
                                     0 /* vmo_offset */, vmo_size, &vmo_virt_addr);
      ZX_ASSERT(status == ZX_OK);
      driver_buffer_id_to_virt_addr_[buffer_id] = vmo_virt_addr;
      uint64_t offset = 0;
      status = vmo.op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, offset, vmo_size, nullptr, 0);
      if (status != ZX_OK) {
        FX_LOGS(ERROR) << "vmo.op_range() failed";
      }
    }

    return fpromise::ok(std::move(buffer_collection_info));
  });
}

promise<void> StreamImpl::ConnectAndStartStream(
    fuchsia::sysmem::BufferCollectionInfo buffer_collection_info) {
  return make_promise([this, buffer_collection_info = std::move(buffer_collection_info)]() mutable {
    return make_promise([]() {
             FrameRate frame_rate;
             UvcHackGetServerFrameRate(&frame_rate);
             return fpromise::ok(std::move(frame_rate));
           })
        .and_then([this, buffer_collection_info =
                             std::move(buffer_collection_info)](FrameRate& frame_rate) mutable {
          return ConnectToStream(std::move(buffer_collection_info), std::move(frame_rate));
        })
        .and_then([this]() { return StartStreaming(); });
  });
}

promise<void> StreamImpl::ConnectToStream(
    fuchsia::sysmem::BufferCollectionInfo buffer_collection_info, FrameRate frame_rate) {
  return make_promise([this, buffer_collection_info = std::move(buffer_collection_info),
                       frame_rate = std::move(frame_rate)]() mutable -> promise<void> {
    // Create event pair to know if we or the driver disconnects.
    zx::eventpair driver_token;
    auto status = zx::eventpair::create(0, &stream_token_, &driver_token);
    ZX_ASSERT_MSG(status == ZX_OK, "Couldn't create driver token. status: %d", status);

    // Ask device side to connect to stream using given BufferCollectionInfo.
    StreamHandle stream_handle;
    return on_stream_requested_(std::move(buffer_collection_info), std::move(frame_rate),
                                stream_handle.NewRequest(), std::move(driver_token))
        .and_then([this, stream_handle = std::move(stream_handle)]() mutable {
          stream_ = stream_handle.Bind();

          // Install frame call back.
          stream_.events().OnFrameAvailable = [this](FrameAvailableEvent frame) {
            if (frame.frame_status != FrameStatus::OK) {
              FX_LOGS(ERROR) << "Error set on incoming frame. Error: "
                             << static_cast<int>(frame.frame_status);
              return;
            }

            // Convert the frame for sending to the client.
            ProcessFrameForSend(frame.buffer_id);

            // Immediately return driver-facing buffer to free pool since it has been processed
            // already.
            stream_->ReleaseFrame(frame.buffer_id);
          };
        });
  });
}

promise<void> StreamImpl::StartStreaming() {
  return make_promise([this]() {
    if (streaming_) {
      FX_LOGS(WARNING) << "Already started streaming!";
      return;
    }
    streaming_ = true;
    WaitOnce(stream_token_.get(), ZX_EVENTPAIR_PEER_CLOSED,
             [this](zx_status_t /*status*/, const zx_packet_signal_t* /*signal*/) {
               // If we get this while we're supposed to be streaming, the driver has encountered
               // and error and we should shutdown gracefully.
               if (streaming_) {
                 OnError(ZX_ERR_INTERNAL,
                         ":saw peer closed event from driver while still streaming.");
                 return;
               }
             });
    stream_->Start();
  });
}

promise<void> StreamImpl::StopStreaming() {
  bridge<void> bridge;
  Schedule([this, completer = std::move(bridge.completer)]() mutable {
    if (!streaming_) {
      FX_LOGS(WARNING) << "Already stopped streaming!";
      completer.complete_ok();
      return;
    }
    streaming_ = false;
    stream_->Stop();

    stream_ = nullptr;

    // Wait until Stop request completed/frames drain (PEER_CLOSED is signalled on the
    // stream_token_).
    zx_signals_t peer_closed_signal = ZX_EVENTPAIR_PEER_CLOSED;
    zx_signals_t observed;
    auto status = zx_object_wait_one(stream_token_.get(), peer_closed_signal,
                                     zx_deadline_after(ZX_MSEC(3000)), &observed);
    if (status == ZX_OK) {
      ZX_ASSERT(observed == ZX_EVENTPAIR_PEER_CLOSED);
    } else if (status == ZX_ERR_TIMED_OUT) {
      FX_LOGS(ERROR) << "Timed out waiting for stream Stop request to complete!";
    } else {
      FX_LOGS(ERROR) << "Saw unexpected error while waiting for stream Stop request to complete: "
                     << status;
    }
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

// Convert the incoming frame to the proper outgoing format. Caller is responsible for releasing the
// buffer back to the driver-facing buffer pool.
void StreamImpl::ProcessFrameForSend(uint32_t driver_buffer_id) {
  // Make sure driver_buffer_id valid.
  auto driver_it = driver_buffer_id_to_virt_addr_.find(driver_buffer_id);
  ZX_ASSERT(driver_it != driver_buffer_id_to_virt_addr_.end());

  // Grab a free client-facing buffer.
  uint32_t client_buffer_id = client_buffer_free_queue_.front();
  client_buffer_free_queue_.pop();

  // Make sure client_buffer_id valid.
  auto client_it = client_buffer_id_to_virt_addr_.find(client_buffer_id);
  ZX_ASSERT(client_it != client_buffer_id_to_virt_addr_.end());

  // Warning! Grotesquely hard coded for YUY2 server side & NV12 client side.
  uintptr_t client_frame_ptr = client_buffer_id_to_virt_addr_[client_buffer_id];
  uint8_t* client_frame = reinterpret_cast<uint8_t*>(client_frame_ptr);
  uintptr_t driver_frame_ptr = driver_buffer_id_to_virt_addr_[driver_buffer_id];
  uint8_t* driver_frame = reinterpret_cast<uint8_t*>(driver_frame_ptr);
  UvcHackConvertYUY2ToNV12(client_frame, driver_frame);

  // Construct frame metadata.
  FrameAvailableEvent info;
  info.frame_status = FrameStatus::OK;
  info.buffer_id = client_buffer_id;
  info.metadata.timestamp = static_cast<uint64_t>(zx_clock_get_monotonic());
  OnFrameAvailable(std::move(info));
}

void StreamImpl::ReleaseClientFrame(uint32_t buffer_id) {
  client_buffer_free_queue_.push(buffer_id);
}

}  // namespace camera
