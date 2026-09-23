// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_TESTS_CUSTOM_ENVIRONMENT_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_TESTS_CUSTOM_ENVIRONMENT_H_

#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <lib/driver/outgoing/cpp/handlers.h>
#include <lib/driver/testing/cpp/driver_test.h>

namespace wlan::drivers::wlansoftmac {

// Type that provides an implementation of fdf_testing::Environment to provide the
// a fuchsia.wlan.softmac/Service endpoint for a wlansoftmac driver instance to
// connect to in a unit test.
template <typename WlanSoftmacServer,
          typename = std::enable_if_t<std::is_constructible_v<
              WlanSoftmacServer, fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmac>>>>
class CustomEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) final {
    device_server_.Initialize(component::kDefaultInstance);
    ZX_ASSERT(ZX_OK == device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                            &to_driver_vfs));

    fuchsia_wlan_softmac::Service::InstanceHandler handler;
    ZX_ASSERT(
        handler
            .add_wlan_softmac([this](fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmac> server_end) {
              wlan_softmac_server_.emplace(std::move(server_end));
            })
            .is_ok());

    auto add_service_result =
        to_driver_vfs.AddService<fuchsia_wlan_softmac::Service>(std::move(handler));
    ZX_ASSERT(add_service_result.is_ok());

    return zx::ok();
  }

  // This is obviously super unsafe, but it's good enough for testing.
  WlanSoftmacServer& GetServer() {
    ZX_ASSERT(wlan_softmac_server_);
    return *wlan_softmac_server_;
  }

 private:
  compat::DeviceServer device_server_;
  std::optional<WlanSoftmacServer> wlan_softmac_server_;
};

}  // namespace wlan::drivers::wlansoftmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_TESTS_CUSTOM_ENVIRONMENT_H_
