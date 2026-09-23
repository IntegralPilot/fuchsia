// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-powerdomain/cpp/fake-powerdomain.h>

namespace fdf_fake {

fuchsia_hardware_powerdomain::Service::InstanceHandler FakePowerDomain::CreateInstanceHandler() {
  return fuchsia_hardware_powerdomain::Service::InstanceHandler({
      .domain = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
  });
}

void FakePowerDomain::Enable(EnableCompleter::Sync& completer) {
  enabled_ = true;
  completer.ReplySuccess();
}

void FakePowerDomain::Disable(DisableCompleter::Sync& completer) {
  enabled_ = false;
  completer.ReplySuccess();
}

void FakePowerDomain::IsEnabled(IsEnabledCompleter::Sync& completer) {
  completer.ReplySuccess(enabled_);
}

}  // namespace fdf_fake
