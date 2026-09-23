// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/usb_device/device_impl.h"

#include <lib/async/cpp/task.h>
#include <lib/fpromise/bridge.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <sstream>
#include <string>

#include "src/camera/bin/usb_device/messages.h"
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

using fuchsia::camera::FrameRate;

using fuchsia::sysmem2::BufferCollectionInfo;

}  // namespace

DeviceImpl::DeviceImpl(async_dispatcher_t* dispatcher, fuchsia::camera::ControlSyncPtr control,
                       fuchsia::sysmem2::AllocatorPtr allocator, zx::event bad_state_event)
    : ActorBase(dispatcher, scope_),
      stream_dispatcher_(dispatcher),
      control_(std::move(control)),
      allocator_(std::move(allocator)),
      bad_state_event_(std::move(bad_state_event)),
      button_listener_binding_(this) {}

DeviceImpl::~DeviceImpl() = default;

// This returns a promise because it will need to be asynchronous once we do something better than
// hard-coded stream properties.
promise<std::unique_ptr<DeviceImpl>, zx_status_t> DeviceImpl::Create(
    async_dispatcher_t* dispatcher, fuchsia::camera::ControlSyncPtr control,
    fuchsia::sysmem2::AllocatorPtr allocator, zx::event bad_state_event) {
  auto device = std::make_unique<DeviceImpl>(dispatcher, std::move(control), std::move(allocator),
                                             std::move(bad_state_event));

  // Construct fake stream properties.
  // TODO(ernesthua) - Should really grab the UVC config list and convert that to stream properties.
  // But not sure if that is really the best design choice for fuchsia.camera3.
  fuchsia::camera3::Configuration2 configuration;
  {
    fuchsia::camera3::StreamProperties2 stream_properties;
    UvcHackGetClientStreamProperties2(&stream_properties);
    configuration.mutable_streams()->push_back(std::move(stream_properties));
  }
  device->configurations_.push_back(std::move(configuration));

  promise<std::unique_ptr<DeviceImpl>, zx_status_t> return_promise = make_promise(
      [device = std::move(device)]() mutable -> result<std::unique_ptr<DeviceImpl>, zx_status_t> {
        return fpromise::ok(std::move(device));
      });
  return return_promise;
}

fidl::InterfaceRequestHandler<fuchsia::camera3::Device> DeviceImpl::GetHandler() {
  return fit::bind_member(this, &DeviceImpl::OnNewRequest);
}

void DeviceImpl::OnNewRequest(fidl::InterfaceRequest<fuchsia::camera3::Device> request) {
  Schedule(Bind(std::move(request)));
}

promise<void> DeviceImpl::Bind(fidl::InterfaceRequest<fuchsia::camera3::Device> request) {
  return make_promise([this, request = std::move(request)]() mutable -> promise<void> {
    bool first_client = clients_.empty();
    auto client = std::make_unique<Client>(*this, client_id_next_, std::move(request));
    auto [it, emplaced] = clients_.emplace(client_id_next_++, std::move(client));
    auto& [id, new_client] = *it;
    if (first_client) {
      return SetConfiguration(0);
    } else {
      new_client->ConfigurationUpdated(current_configuration_index_);
      return make_ok_promise();
    }
  });
}

promise<void> DeviceImpl::RemoveClient(uint64_t id) {
  return make_promise([this, id]() { clients_.erase(id); });
}

promise<void> DeviceImpl::SetConfiguration(uint32_t index) {
  return make_promise([this, index]() {
    std::vector<promise<void, zx_status_t>> deallocation_promises;
    for (auto& event : deallocation_events_) {
      bridge<void, zx_status_t> bridge;
      WaitOnce(event.release(), ZX_EVENTPAIR_PEER_CLOSED,
               [completer = std::move(bridge.completer)](zx_status_t status,
                                                         const zx_packet_signal_t* signal) mutable {
                 FX_LOGS(INFO) << "Client buffer collection deallocated successfully.";
                 if (status != ZX_OK) {
                   completer.complete_error(status);
                   return;
                 }
                 completer.complete_ok();
               });
      deallocation_promises.push_back(bridge.consumer.promise());
    }
    deallocation_events_.clear();
    deallocation_promises_ = std::move(deallocation_promises);

    std::vector<promise<void>> stream_clients_closed_promises;
    for (auto& stream : streams_) {
      if (stream) {
        stream_clients_closed_promises.push_back(stream->CloseAllClients(ZX_OK));
      }
    }

    return join_promise_vector(std::move(stream_clients_closed_promises))
        .and_then([this, index](std::vector<result<void>>& results) {
          streams_.clear();
          streams_.resize(configurations_[index].streams().size());

          FX_LOGS(INFO) << "Configuration set to " << index << ".";
          for (auto& client : clients_) {
            client.second->ConfigurationUpdated(current_configuration_index_);
          }
        });
  });
}

promise<void> DeviceImpl::ConnectToStream(
    uint32_t index, fidl::InterfaceRequest<fuchsia::camera3::Stream> request) {
  return make_promise([this, index, request = std::move(request)]() mutable {
    if (index >= streams_.size()) {
      request.Close(ZX_ERR_INVALID_ARGS);
      return;
    }

    if (streams_[index]) {
      request.Close(ZX_ERR_ALREADY_BOUND);
      return;
    }

    StreamImpl::StreamRequestedCallback on_stream_requested =
        [this, index](fuchsia::sysmem::BufferCollectionInfo buffer_collection_info,
                      FrameRate frame_rate, fidl::InterfaceRequest<fuchsia::camera::Stream> request,
                      zx::eventpair driver_token) {
          bridge<void> bridge;
          Schedule(OnStreamRequested(index, std::move(buffer_collection_info),
                                     std::move(frame_rate), std::move(request),
                                     std::move(driver_token))
                       .and_then([completer = std::move(bridge.completer)]() mutable {
                         completer.complete_ok();
                       }));
          return bridge.consumer.promise();
        };

    // When the last client disconnects destroy the stream.
    StreamImpl::NoClientsCallback on_no_clients = [this, index]() {
      bridge<void> bridge;
      Schedule([this, index, completer = std::move(bridge.completer)]() mutable {
        return streams_[index]->StopStreaming().and_then(
            [this, index, completer = std::move(completer)]() mutable {
              streams_[index] = nullptr;
              completer.complete_ok();
            });
      });
      return bridge.consumer.promise();
    };

    auto description =
        "c" + std::to_string(current_configuration_index_) + "s" + std::to_string(index);

    StreamImpl::AllocatorBindSharedCollectionCallback allocator_bind_shared_collection =
        [this](fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token_handle,
               fidl::InterfaceRequest<fuchsia::sysmem2::BufferCollection> request) {
          bridge<void> bridge;
          Schedule([this, token_handle = std::move(token_handle), request = std::move(request),
                    completer = std::move(bridge.completer)]() mutable {
            fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
            bind_shared_request.set_token(std::move(token_handle));
            bind_shared_request.set_buffer_collection_request(std::move(request));
            allocator_->BindSharedCollection(std::move(bind_shared_request));
            completer.complete_ok();
          });
          return bridge.consumer.promise();
        };

    StreamImpl::RegisterDeallocationEvent register_deallocation_event =
        [this](zx::eventpair deallocation_event) {
          bridge<void> bridge;
          Schedule([this, deallocation_event = std::move(deallocation_event),
                    completer = std::move(bridge.completer)]() mutable {
            deallocation_events_.push_back(std::move(deallocation_event));
            completer.complete_ok();
          });
          return bridge.consumer.promise();
        };

    streams_[index] = std::make_unique<StreamImpl>(
        stream_dispatcher_, configurations_[current_configuration_index_].streams()[index],
        std::move(request), std::move(on_stream_requested),
        std::move(allocator_bind_shared_collection), std::move(register_deallocation_event),
        std::move(on_no_clients), description);
  });
}

static inline bool IsDeallocationComplete(
    const result<std::vector<result<void, zx_status_t>>>& results, const std::string& description) {
  bool deallocation_complete = true;
  if (results.is_error()) {
    FX_LOGS(WARNING) << "aggregate deallocation wait failed prior to connecting to " << description;
    deallocation_complete = false;
  } else {
    auto& wait_results = results.value();
    for (const auto& wait_result : wait_results) {
      if (wait_result.is_error()) {
        FX_PLOGS(WARNING, wait_result.error())
            << "wait failed for previous stream at index " << &wait_result - wait_results.data();
        deallocation_complete = false;
      }
    }
  }

  return deallocation_complete;
}

// This call back function is invoked when the client asks to connect to a camera stream. The call
// back to DeviceImpl is necessary because the control connection is maintained in DeviceImpl.
// Therefore, the call to CreateStream originates here.
promise<void> DeviceImpl::OnStreamRequested(
    uint32_t index, fuchsia::sysmem::BufferCollectionInfo buffer_collection_info,
    FrameRate frame_rate, fidl::InterfaceRequest<fuchsia::camera::Stream> request,
    zx::eventpair driver_token) {
  auto connect = make_promise([this, buffer_collection_info = std::move(buffer_collection_info),
                               frame_rate = std::move(frame_rate), request = std::move(request),
                               driver_token = std::move(driver_token)]() mutable {
    control_->CreateStream(std::move(buffer_collection_info), std::move(frame_rate),
                           std::move(request), std::move(driver_token));
  });

  // Wait for any previous configurations buffers to finish deallocation, then connect
  // to stream. The move and clear are necessary to ensure subsequent accesses to the container
  // produces well-defined results.
  return make_promise([this, index, connect = std::move(connect)]() mutable {
    auto promises = std::move(deallocation_promises_);
    deallocation_promises_.clear();

    FX_LOGS(INFO) << "Waiting on the deallocation of " << promises.size()
                  << " previous client BufferCollection(s).";
    return join_promise_vector(std::move(promises))
        .then([this, index, connect = std::move(connect)](
                  const result<std::vector<result<void, zx_status_t>>>& results) mutable
              -> promise<void> {
          FX_LOGS(INFO) << "Finished waiting for client BufferCollection deallocations.";
          // TODO(ernesthua): Get the proper camera index.
          std::string description = "c0s" + std::to_string(index);
          if (!IsDeallocationComplete(results, description)) {
            constexpr uint32_t kFallbackDelayMsec = 5000;
            FX_LOGS(WARNING) << "deallocation wait failed; falling back to timed wait of "
                             << kFallbackDelayMsec << "ms";
            ScheduleAfterDelay(zx::msec(kFallbackDelayMsec), std::move(connect));
            return make_ok_promise();
          }

          FX_LOGS(INFO) << "connecting to controller stream: " << description;
          return std::move(connect);
        });
  });
}

void DeviceImpl::OnEvent(fuchsia::ui::input::MediaButtonsEvent event,
                         fuchsia::ui::policy::MediaButtonsListener::OnEventCallback callback) {
  callback();
}

}  // namespace camera
