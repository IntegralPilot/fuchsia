// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-pin/cpp/fake-pin.h>

namespace fdf_fake {

fuchsia_hardware_pin::Service::InstanceHandler FakePin::CreateInstanceHandler() {
  return fuchsia_hardware_pin::Service::InstanceHandler({
      .device = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
  });
}

std::optional<fuchsia_hardware_pin::Pull> FakePin::take_pull() {
  std::optional<fuchsia_hardware_pin::Pull> pull = pull_;
  pull_.reset();
  return pull;
}

std::optional<uint32_t> FakePin::take_drive_strength_ua() {
  std::optional<uint32_t> drive_strength_ua = drive_strength_ua_;
  drive_strength_ua_.reset();
  return drive_strength_ua;
}

std::optional<uint64_t> FakePin::take_function() {
  std::optional<uint64_t> function = function_;
  function_.reset();
  return function;
}

std::optional<uint64_t> FakePin::take_slew_rate() {
  std::optional<uint64_t> slew_rate = slew_rate_;
  slew_rate_.reset();
  return slew_rate;
}

std::optional<std::string> FakePin::take_function_name() {
  std::optional<std::string> function_name = function_name_;
  function_name_.reset();
  return function_name;
}

void FakePin::Configure(ConfigureRequestView request, ConfigureCompleter::Sync& completer) {
  if (request->config.has_pull()) {
    pull_ = request->config.pull();
  }
  if (request->config.has_drive_strength_ua()) {
    drive_strength_ua_ = request->config.drive_strength_ua();
  }
  if (request->config.has_function()) {
    function_ = request->config.function();
  }
  if (request->config.has_slew_rate()) {
    slew_rate_ = request->config.slew_rate();
  }
  if (request->config.has_function_name()) {
    function_name_ = std::string(request->config.function_name().get());
  }
  completer.ReplySuccess(request->config);
}

}  // namespace fdf_fake
