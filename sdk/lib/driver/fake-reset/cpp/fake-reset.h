// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_RESET_CPP_FAKE_RESET_H_
#define LIB_DRIVER_FAKE_RESET_CPP_FAKE_RESET_H_

#include <fidl/fuchsia.hardware.reset/cpp/wire_test_base.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <zircon/errors.h>

namespace fdf_fake {

class FakeReset : public fidl::testing::WireTestBase<fuchsia_hardware_reset::Reset> {
 public:
  explicit FakeReset(
      async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher())
      : dispatcher_(dispatcher) {}

  fuchsia_hardware_reset::Service::InstanceHandler CreateInstanceHandler();

  bool take_toggled();
  bool take_asserted();
  bool take_deasserted();

 private:
  void Toggle(ToggleCompleter::Sync& completer) override;
  void ToggleWithTimeout(ToggleWithTimeoutRequestView request,
                         ToggleWithTimeoutCompleter::Sync& completer) override;
  void Assert(AssertCompleter::Sync& completer) override;
  void Deassert(DeassertCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_reset::Reset> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  async_dispatcher_t* const dispatcher_;

  bool toggled_ = false;
  bool asserted_ = false;
  bool deasserted_ = false;
  fidl::ServerBindingGroup<fuchsia_hardware_reset::Reset> bindings_;
};

}  // namespace fdf_fake

#endif  // LIB_DRIVER_FAKE_RESET_CPP_FAKE_RESET_H_
