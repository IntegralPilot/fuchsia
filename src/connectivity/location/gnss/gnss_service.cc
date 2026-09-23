// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gnss_service.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

namespace gnss {

class GnssService::LocationListenerImpl
    : public fidl::Server<fuchsia_location_gnss::LocationListener> {
 public:
  explicit LocationListenerImpl() : current_version_(0) {}

  void GetNextLocation(GetNextLocationCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_location_gnss::LocationListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Notify location update to listeners.
  void Notify(fuchsia_location_gnss_types::Location loc);

 private:
  LocationListenerImpl(const LocationListenerImpl&) = delete;
  LocationListenerImpl& operator=(const LocationListenerImpl&) = delete;

  std::mutex mutex_;
  std::optional<fuchsia_location_gnss_types::Location> current_data_;
  uint64_t current_version_;
  std::optional<uint64_t> last_sent_version_;
  std::optional<GetNextLocationCompleter::Async> async_completer_;
};

void GnssService::LocationListenerImpl::GetNextLocation(GetNextLocationCompleter::Sync& completer) {
  std::lock_guard lock(mutex_);

  // Report immediately if already a new location already available.
  if (current_data_.has_value() && last_sent_version_ != current_version_) {
    FX_LOGS(DEBUG) << "GetNextLocation immediately returning available location.";
    last_sent_version_ = current_version_;
    completer.Reply(fit::ok(current_data_.value()));
    return;
  }

  // No new location is available. Save the completer and wait for location update.
  async_completer_ = completer.ToAsync();
}

void GnssService::LocationListenerImpl::Notify(fuchsia_location_gnss_types::Location location) {
  std::lock_guard lock(mutex_);
  uint64_t next_version = current_version_ + 1;

  // Check if Location have actually changed.
  if (current_data_.has_value() && current_data_.value() == location) {
    FX_LOGS(DEBUG) << "Notify: Location unchanged, not updating version or notifying.";
    return;
  }

  FX_LOGS(DEBUG) << "Notify: Updating location, new version: " << next_version;

  // Update the location
  current_data_ = location;
  current_version_ = next_version;

  // Complete any pending GetNextLocation call.
  if (async_completer_) {
    async_completer_->Reply(fit::ok(location));
    last_sent_version_ = current_version_;
    FX_LOGS(DEBUG) << "GetNextLocation returning new location.";
    async_completer_.reset();
  }
}

void GnssService::LocationListenerImpl::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_location_gnss::LocationListener> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Received an unknown method with ordinal " << metadata.method_ordinal;
}

GnssService::GnssService(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

void GnssService::SendUpdateToListeners(const fuchsia_location_gnss_types::Location& location) {
  // Complete any pending singleshot location request.
  if (async_single_shot_request_completer_) {
    async_single_shot_request_completer_->Reply(fit::ok(location));
    async_single_shot_request_completer_.reset();
  }

  // Notify all listeners about location.
  for (const auto& listener : listeners_) {
    listener->Notify(location);
  }
}

void GnssService::GetSingleShotFix(GetSingleShotFixRequest& request,
                                   GetSingleShotFixCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/411471488): Validate request.

  async_single_shot_request_completer_ = completer.ToAsync();
}

void GnssService::StartTimeBasedLocationTracking(
    StartTimeBasedLocationTrackingRequest& request,
    StartTimeBasedLocationTrackingCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/411471488): Validate request.

  fidl::ServerEnd<fuchsia_location_gnss::LocationListener> listener_server_end =
      std::move(request.listener());
  auto listener_impl = std::make_shared<LocationListenerImpl>();
  listener_bindings_.push_back(
      fidl::BindServer(dispatcher_, std::move(listener_server_end), listener_impl));
  listeners_.push_back(listener_impl);
  FX_LOGS(DEBUG) << "StartTimeBasedLocationTracking: Listener connected";
  completer.Reply(fit::ok());
}

void GnssService::GetCapabilities(GetCapabilitiesCompleter::Sync& completer) {
  // TODO(https://fxbug.dev/401262309): Fetch capabilities from driver.
  auto capabilities = fuchsia_location_gnss_types::Capabilities::kCapabilityScheduling |
                      fuchsia_location_gnss_types::Capabilities::kCapabilitySingleShot |
                      fuchsia_location_gnss_types::Capabilities::kCapabilityMsa;
  completer.Reply(capabilities);
}

void GnssService::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_location_gnss::Provider> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Received an unknown method with ordinal " << metadata.method_ordinal;
}

void GnssService::AddBinding(async_dispatcher_t* dispatcher,
                             fidl::ServerEnd<fuchsia_location_gnss::Provider> server_end) {
  // Add the incoming connection to the binding group.
  // The binding group associates the connection with 'this' service instance.
  bindings_.AddBinding(dispatcher, std::move(server_end), this,
                       std::mem_fn(&GnssService::OnUnbound));
  FX_LOGS(DEBUG) << "GnssService::AddBinding: Bound new client. Total bindings: "
                 << bindings_.size();
}

void GnssService::OnUnbound(fidl::UnbindInfo info) {
  if (info.is_user_initiated()) {
    return;
  }
  if (info.is_peer_closed()) {
    FX_LOGS(DEBUG) << "Client disconnected.";
  } else {
    FX_LOGS(ERROR) << "Server error: " << info;
  }
}

}  // namespace gnss
