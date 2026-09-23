// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/ui/flatland-frame-scheduling/src/simple_present.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

namespace simple_present {

FlatlandConnection::FlatlandConnection(async_dispatcher_t* dispatcher,
                                       fidl::ClientEnd<fuchsia_ui_composition::Flatland> flatland,
                                       const std::string& debug_name) {
  flatland_ = fidl::Client(std::move(flatland), dispatcher, this);
  auto set_debug_name_res = flatland_->SetDebugName({debug_name});
  if (set_debug_name_res.is_error()) {
    FX_LOGS(ERROR) << "Failed to SetDebugName: "
                   << set_debug_name_res.error_value().status_string();
  }
}

FlatlandConnection::~FlatlandConnection() = default;

// static
std::unique_ptr<FlatlandConnection> FlatlandConnection::Create(async::Loop* loop,
                                                               const std::string& debug_name) {
  auto connect = component::Connect<fuchsia_ui_composition::Flatland>();
  if (connect.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to Flatland protocol: " << connect.status_string();
    return nullptr;
  }

  return std::unique_ptr<FlatlandConnection>(
      new FlatlandConnection(loop->dispatcher(), std::move(connect.value()), debug_name));
}

// static
std::unique_ptr<FlatlandConnection> FlatlandConnection::Create(
    async_dispatcher_t* dispatcher, fidl::ClientEnd<fuchsia_ui_composition::Flatland> flatland,
    const std::string& debug_name) {
  return std::unique_ptr<FlatlandConnection>(
      new FlatlandConnection(dispatcher, std::move(flatland), debug_name));
}

fidl::Client<fuchsia_ui_composition::Flatland>& FlatlandConnection::FlatlandClient() {
  return flatland_;
}

void FlatlandConnection::SetErrorCallback(OnErrorCallback callback) {
  error_callback_ = std::move(callback);
}

void FlatlandConnection::Present() {
  fuchsia_ui_composition::PresentArgs present_args;
  present_args.requested_presentation_time(0);
  present_args.acquire_fences({});
  present_args.release_fences({});
  present_args.unsquashable(false);
  Present(std::move(present_args), [](auto) {});
}

void FlatlandConnection::Present(fuchsia_ui_composition::PresentArgs present_args,
                                 OnFramePresentedCallback callback) {
  if (present_credits_ == 0) {
    pending_presents_.emplace(std::move(present_args), std::move(callback));
    FX_DCHECK(pending_presents_.size() <= 10u) << "Too many pending presents.";
    return;
  }
  --present_credits_;

  // In Flatland, release fences apply to the content of the previous present.
  // Keeping track of the previous frame's release fences and swapping ensure we
  // set the correct ones.
  if (!present_args.release_fences().has_value()) {
    present_args.release_fences(std::vector<zx::event>{});
  }
  present_args.release_fences()->swap(previous_present_release_fences_);

  auto res = flatland_->Present({std::move(present_args)});
  if (res.is_error()) {
    FX_LOGS(ERROR) << "Failed to Present: " << res.error_value().status_string();
  }

  presented_callbacks_.push(std::move(callback));
}

void FlatlandConnection::on_fidl_error(fidl::UnbindInfo error) { error_callback_(); }
void FlatlandConnection::OnError(fidl::Event<fuchsia_ui_composition::Flatland::OnError>& error) {
  error_callback_();
}
void FlatlandConnection::OnNextFrameBegin(
    fidl::Event<fuchsia_ui_composition::Flatland::OnNextFrameBegin>& e) {
  present_credits_ += e.values().additional_present_credits().value();
  if (present_credits_ && !pending_presents_.empty()) {
    // Only iterate over the elements once, because they may be added back to
    // the queue.
    while (present_credits_ && !pending_presents_.empty()) {
      PendingPresent present = std::move(pending_presents_.front());
      pending_presents_.pop();
      Present(std::move(present.present_args), std::move(present.callback));
    }
  }
}
void FlatlandConnection::OnFramePresented(
    fidl::Event<fuchsia_ui_composition::Flatland::OnFramePresented>& e) {
  auto actual_presentation_time = e.frame_presented_info().actual_presentation_time();
  auto num_presentation_infos = e.frame_presented_info().presentation_infos().size();
  for (size_t i = 0; i < num_presentation_infos; ++i) {
    presented_callbacks_.front()(actual_presentation_time);
    presented_callbacks_.pop();
  }
}

FlatlandConnection::PendingPresent::PendingPresent(fuchsia_ui_composition::PresentArgs present_args,
                                                   OnFramePresentedCallback callback)
    : present_args(std::move(present_args)), callback(std::move(callback)) {}
FlatlandConnection::PendingPresent::~PendingPresent() = default;

FlatlandConnection::PendingPresent::PendingPresent(PendingPresent&& other) = default;
FlatlandConnection::PendingPresent& FlatlandConnection::PendingPresent::operator=(
    PendingPresent&&) = default;

}  // namespace simple_present
