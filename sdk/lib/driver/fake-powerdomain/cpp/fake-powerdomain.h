// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_POWER_DOMAIN_CPP_FAKE_POWER_DOMAIN_H_
#define LIB_DRIVER_FAKE_POWER_DOMAIN_CPP_FAKE_POWER_DOMAIN_H_

#include <fidl/fuchsia.hardware.powerdomain/cpp/wire_test_base.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <zircon/errors.h>

namespace fdf_fake {

class FakePowerDomain : public fidl::testing::WireTestBase<fuchsia_hardware_powerdomain::Domain> {
 public:
  explicit FakePowerDomain(
      async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher())
      : dispatcher_(dispatcher) {}

  fuchsia_hardware_powerdomain::Service::InstanceHandler CreateInstanceHandler();

  bool is_enabled() const { return enabled_; }

 private:
  void Enable(EnableCompleter::Sync& completer) override;
  void Disable(DisableCompleter::Sync& completer) override;
  void IsEnabled(IsEnabledCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_powerdomain::Domain> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  async_dispatcher_t* const dispatcher_;

  bool enabled_ = false;
  fidl::ServerBindingGroup<fuchsia_hardware_powerdomain::Domain> bindings_;
};

}  // namespace fdf_fake

#endif  // LIB_DRIVER_FAKE_POWER_DOMAIN_CPP_FAKE_POWER_DOMAIN_H_
