// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-reset/cpp/fake-reset.h>

namespace fdf_fake {

fuchsia_hardware_reset::Service::InstanceHandler FakeReset::CreateInstanceHandler() {
  return fuchsia_hardware_reset::Service::InstanceHandler({
      .reset = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
  });
}

bool FakeReset::take_toggled() {
  bool toggled = toggled_;
  toggled_ = false;
  return toggled;
}

bool FakeReset::take_asserted() {
  bool asserted = asserted_;
  asserted_ = false;
  return asserted;
}

bool FakeReset::take_deasserted() {
  bool deasserted = deasserted_;
  deasserted_ = false;
  return deasserted;
}

void FakeReset::Toggle(ToggleCompleter::Sync& completer) {
  toggled_ = true;
  completer.Reply(zx::ok());
}

void FakeReset::ToggleWithTimeout(ToggleWithTimeoutRequestView request,
                                  ToggleWithTimeoutCompleter::Sync& completer) {
  toggled_ = true;
  completer.Reply(zx::ok());
}

void FakeReset::Assert(AssertCompleter::Sync& completer) {
  asserted_ = true;
  completer.Reply(zx::ok());
}

void FakeReset::Deassert(DeassertCompleter::Sync& completer) {
  deasserted_ = true;
  completer.Reply(zx::ok());
}

}  // namespace fdf_fake
