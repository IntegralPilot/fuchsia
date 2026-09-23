// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_PIN_CPP_FAKE_PIN_H_
#define LIB_DRIVER_FAKE_PIN_CPP_FAKE_PIN_H_

#include <fidl/fuchsia.hardware.pin/cpp/wire_test_base.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <zircon/errors.h>

#include <optional>

namespace fdf_fake {

class FakePin : public fidl::testing::WireTestBase<fuchsia_hardware_pin::Pin> {
 public:
  explicit FakePin(
      async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher())
      : dispatcher_(dispatcher) {}

  fuchsia_hardware_pin::Service::InstanceHandler CreateInstanceHandler();

  std::optional<fuchsia_hardware_pin::Pull> take_pull();
  std::optional<uint32_t> take_drive_strength_ua();
  std::optional<uint64_t> take_function();
  std::optional<uint64_t> take_slew_rate();
  std::optional<std::string> take_function_name();

 private:
  void Configure(ConfigureRequestView request, ConfigureCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_pin::Pin> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  async_dispatcher_t* const dispatcher_;

  std::optional<fuchsia_hardware_pin::Pull> pull_;
  std::optional<uint32_t> drive_strength_ua_;
  std::optional<uint64_t> function_;
  std::optional<uint64_t> slew_rate_;
  std::optional<std::string> function_name_;
  fidl::ServerBindingGroup<fuchsia_hardware_pin::Pin> bindings_;
};

}  // namespace fdf_fake

#endif  // LIB_DRIVER_FAKE_PIN_CPP_FAKE_PIN_H_
