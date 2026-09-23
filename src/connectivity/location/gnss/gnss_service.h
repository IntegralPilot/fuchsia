// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_LOCATION_GNSS_GNSS_SERVICE_H_
#define SRC_CONNECTIVITY_LOCATION_GNSS_GNSS_SERVICE_H_

#include <fidl/fuchsia.location.gnss/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

namespace gnss {

class GnssService final : public fidl::Server<fuchsia_location_gnss::Provider> {
 public:
  GnssService(async_dispatcher_t* dispatcher);
  // fuchsia.location.gnss.Provider APIs.
  void GetSingleShotFix(GetSingleShotFixRequest& request,
                        GetSingleShotFixCompleter::Sync& completer) override;
  void StartTimeBasedLocationTracking(
      StartTimeBasedLocationTrackingRequest& request,
      StartTimeBasedLocationTrackingCompleter::Sync& completer) override;
  void GetCapabilities(GetCapabilitiesCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_location_gnss::Provider> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  void AddBinding(async_dispatcher_t* dispatcher,
                  fidl::ServerEnd<fuchsia_location_gnss::Provider> server_end);
  void OnUnbound(fidl::UnbindInfo info);

  // This function is used to report location to listeners when it is available from driver.
  void SendUpdateToListeners(const fuchsia_location_gnss_types::Location& location);

 private:
  class LocationListenerImpl;
  // std::optional<fidl::ServerBindingRef<fuchsia_location_gnss::Provider>> binding_ref_;
  fidl::ServerBindingGroup<fuchsia_location_gnss::Provider> bindings_;
  std::vector<fidl::ServerBindingRef<fuchsia_location_gnss::LocationListener>> listener_bindings_;
  std::vector<std::shared_ptr<GnssService::LocationListenerImpl>> listeners_;
  std::optional<GetSingleShotFixCompleter::Async> async_single_shot_request_completer_;

  async_dispatcher_t* dispatcher_;
};

}  // namespace gnss

#endif  // SRC_CONNECTIVITY_LOCATION_GNSS_GNSS_SERVICE_H_
