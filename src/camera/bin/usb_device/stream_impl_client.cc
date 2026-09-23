// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <sstream>

#include "src/camera/bin/usb_device/messages.h"
#include "src/camera/bin/usb_device/size_util.h"
#include "src/camera/bin/usb_device/stream_impl.h"
#include "src/lib/fsl/handles/object_info.h"

namespace camera {
namespace {

using fpromise::make_error_promise;
using fpromise::make_ok_promise;
using fpromise::make_promise;

using fuchsia::camera3::FrameInfo2;

using fuchsia::sysmem2::BufferCollectionInfo;
using fuchsia::sysmem2::BufferCollectionTokenPtr;

}  // namespace

StreamImpl::Client::Client(StreamImpl& stream, uint64_t id,
                           fidl::InterfaceRequest<fuchsia::camera3::Stream> request)
    : stream_(stream), id_(id), binding_(this, std::move(request)), resolution_(SizeEqual) {
  log_prefix_ = "stream " + stream_.description_ +
                " client (koid = " + std::to_string(GetRelatedKoid(binding_)) + "): ";
  FX_LOGS(INFO) << log_prefix_ << "new stream client, id = " << id_;
  binding_.set_error_handler(fit::bind_member(this, &StreamImpl::Client::OnClientDisconnected));
}

StreamImpl::Client::~Client() = default;

void StreamImpl::Client::AddFrame(FrameInfo2 frame) {
  TRACE_DURATION("camera", "StreamImpl::Client::AddFrame");
  frames_.push(std::move(frame));
  if (!frame_logging_state_.available) {
    FX_LOGS(INFO) << log_prefix_ << "frame available";
    frame_logging_state_.available = true;
  }
  MaybeSendFrame();
}

void StreamImpl::Client::MaybeSendFrame() {
  TRACE_DURATION("camera", "StreamImpl::Client::MaybeSendFrame");
  if (frames_.empty() || !frame_callback_) {
    return;
  }
  auto& frame = frames_.front();
  // This Flow can be connected on the client end to allow tracing the flow of frames
  // into the client.
  TRACE_FLOW_BEGIN("camera", "camera3::Stream::GetNextFrame",
                   fsl::GetKoid(frame.release_fence().get()));
  // Note: this message is logged immediately prior to invoking the frame callback to ensure that no
  // client logs emitted as a result will have earlier timestamps than this message.
  if (!frame_logging_state_.sent) {
    FX_LOGS(INFO) << log_prefix_ << "sent frame";
    frame_logging_state_.sent = true;
  }
  frame_callback_(std::move(frame));
  frames_.pop();
  frame_callback_ = nullptr;
}

void StreamImpl::Client::ReceiveBufferCollection(
    fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token) {
  TRACE_DURATION("camera", "StreamImpl::Client::ReceiveBufferCollection");
  buffers_.Set(std::move(token));
}

void StreamImpl::Client::ReceiveResolution(fuchsia::math::Size coded_size) {
  TRACE_DURATION("camera", "StreamImpl::Client::ReceiveResolution");
  resolution_.Set(coded_size);
}

void StreamImpl::Client::ReceiveCropRegion(std::unique_ptr<fuchsia::math::RectF> region) {
  TRACE_DURATION("camera", "StreamImpl::Client::ReceiveCropRegion");
  crop_region_.Set(std::move(region));
}

bool& StreamImpl::Client::Participant() { return participant_; }

void StreamImpl::Client::ClearFrames() {
  while (!frames_.empty()) {
    frames_.pop();
  }
}

void StreamImpl::Client::OnClientDisconnected(zx_status_t status) {
  FX_PLOGS(INFO, status) << log_prefix_ << "closed connection";
  stream_.Schedule(stream_.RemoveClient(id_));
}

void StreamImpl::Client::CloseConnection(zx_status_t status) { binding_.Close(status); }

void StreamImpl::Client::GetProperties(GetPropertiesCallback callback) {
  FX_LOGS(ERROR) << log_prefix_ << "FIDL call not supported";
}

void StreamImpl::Client::GetProperties2(GetProperties2Callback callback) {
  FX_LOGS(ERROR) << log_prefix_ << "FIDL call not supported";
}

void StreamImpl::Client::SetCropRegion(std::unique_ptr<fuchsia::math::RectF> region) {
  FX_LOGS(ERROR) << log_prefix_ << "FIDL call not supported";
}

void StreamImpl::Client::WatchCropRegion(WatchCropRegionCallback callback) {
  FX_LOGS(ERROR) << log_prefix_ << "FIDL call not supported";
}

void StreamImpl::Client::SetResolution(fuchsia::math::Size coded_size) {
  FX_LOGS(ERROR) << log_prefix_ << "FIDL call not supported";
}

void StreamImpl::Client::WatchResolution(WatchResolutionCallback callback) {
  FX_LOGS(ERROR) << log_prefix_ << "FIDL call not supported";
}

void StreamImpl::Client::SetBufferCollection2(
    fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token) {
  SetBufferCollectionCommon(std::move(token));
}

void StreamImpl::Client::SetBufferCollection(
    fidl::InterfaceHandle<fuchsia::sysmem::BufferCollectionToken> token) {
  SetBufferCollectionCommon(
      fuchsia::sysmem2::BufferCollectionTokenHandle(std::move(token).TakeChannel()));
}

void StreamImpl::Client::SetBufferCollectionCommon(
    fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token) {
  auto nonce = TRACE_NONCE();
  TRACE_ASYNC_BEGIN("camera", "StreamImpl::Client::SetBufferCollection", nonce);
  FX_LOGS(INFO) << log_prefix_ << "called SetBufferCollection(koid = " << GetRelatedKoid(token)
                << ")";
  auto promise =
      stream_.SetClientParticipation(id_, std::move(token))
          .then([this,
                 nonce](fpromise::result<std::optional<BufferCollectionTokenPtr>>& result) mutable
                    -> fpromise::promise<void> {
            // If the result of SetClientParticipation was nullopt, we're done.
            if (!result.value())
              return make_ok_promise();

            return stream_.InitBufferCollections(std::move(*result.value()))
                .then([this](fpromise::result<fuchsia::sysmem::BufferCollectionInfo, zx_status_t>&
                                 result) -> fpromise::promise<void> {
                  if (result.is_ok()) {
                    return stream_.ConnectAndStartStream(std::move(result.value()));
                  } else {
                    stream_.OnError(result.error(), ":failed to initialize buffer collections.");
                    return make_ok_promise();
                  }
                })
                .and_then([nonce]() {
                  TRACE_ASYNC_END("camera", "StreamImpl::Client::SetBufferCollection", nonce);
                });
          });
  stream_.Schedule(std::move(promise));
}

void StreamImpl::Client::WatchBufferCollection2(WatchBufferCollection2Callback callback) {
  WatchBufferCollectionCommon(
      [callback = std::move(callback)](
          fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token) {
        std::move(callback)(std::move(token));
      });
}

void StreamImpl::Client::WatchBufferCollection(WatchBufferCollectionCallback callback) {
  WatchBufferCollectionCommon(
      [callback = std::move(callback)](
          fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token) {
        std::move(callback)(fidl::InterfaceHandle<fuchsia::sysmem::BufferCollectionToken>(
            std::move(token).TakeChannel()));
      });
}

void StreamImpl::Client::WatchBufferCollectionCommon(
    fit::function<void(fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken>)> callback) {
  TRACE_DURATION("camera", "StreamImpl::Client::WatchBufferCollection");
  FX_LOGS(INFO) << log_prefix_ << "called WatchBufferCollection()";
  if (buffers_.Get(std::move(callback))) {
    CloseConnection(ZX_ERR_BAD_STATE);
    stream_.Schedule(stream_.RemoveClient(id_));
  }
}

void StreamImpl::Client::WatchOrientation(WatchOrientationCallback callback) {
  TRACE_DURATION("camera", "StreamImpl::Client::WatchOrientation");
  // It is invalid to have multiple Watch requests in flight.
  if (orientation_callback_) {
    CloseConnection(ZX_ERR_BAD_STATE);
    stream_.Schedule(stream_.RemoveClient(id_));
    return;
  }

  // A watch call subsequent to the first will never be invoked, but the object must still be
  // persisted. Destruction of captured FIDL objects prior to invocation would be interpreted as a
  // server error.
  orientation_callback_ = std::move(callback);
}

void StreamImpl::Client::GetNextFrame(GetNextFrameCallback callback) {
  GetNextFrame2([callback = std::move(callback)](FrameInfo2 frame) {
    callback({.buffer_index = frame.buffer_index(),
              .frame_counter = frame.frame_counter(),
              .timestamp = frame.timestamp(),
              .release_fence = std::move(*frame.mutable_release_fence())});
  });
}

void StreamImpl::Client::GetNextFrame2(GetNextFrame2Callback callback) {
  TRACE_DURATION("camera", "StreamImpl::Client::GetNextFrame2");
  if (!frame_logging_state_.requested) {
    FX_LOGS(INFO) << log_prefix_ << "called GetNextFrame2()";
    frame_logging_state_.requested = true;
  }
  if (frame_callback_) {
    FX_LOGS(INFO) << stream_.description_ << ": " << id_
                  << ": Client called GetNextFrame while a previous call was still pending.";
    CloseConnection(ZX_ERR_BAD_STATE);
    stream_.Schedule(stream_.RemoveClient(id_));
    return;
  }
  frame_callback_ = std::move(callback);
  MaybeSendFrame();
}

void StreamImpl::Client::Rebind(fidl::InterfaceRequest<Stream> request) {
  FX_LOGS(INFO) << log_prefix_ << "called Rebind(koid = " << GetRelatedKoid(request) << ")";
  stream_.OnNewRequest(std::move(request));
}

}  // namespace camera
