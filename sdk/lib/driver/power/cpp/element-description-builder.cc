// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/driver/power/cpp/element-description-builder.h>
#include <lib/zx/event.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_power {

ElementDesc ElementDescBuilder::Build() {
  ElementDesc to_return;
  to_return.element_config = element_config;
  to_return.tokens = std::move(tokens_);

  if (this->assertive_token_.has_value()) {
    to_return.assertive_token = std::move(this->assertive_token_.value());
  } else {
    // make an event instead
    zx::event::create(0, &to_return.assertive_token);
  }

  if (this->lessor_.has_value()) {
    to_return.lessor_server = std::move(this->lessor_.value());
  } else {
    // make a channel instead, include it in output
    fidl::Endpoints<fuchsia_power_broker::Lessor> endpoints =
        fidl::CreateEndpoints<fuchsia_power_broker::Lessor>().value();
    to_return.lessor_client = std::move(endpoints.client);
    to_return.lessor_server = std::move(endpoints.server);
  }

  if (this->element_control_.has_value()) {
    to_return.element_control_server = std::move(this->element_control_.value());
  } else {
    // make a channel instead, include it in output
    fidl::Endpoints<fuchsia_power_broker::ElementControl> endpoints =
        fidl::CreateEndpoints<fuchsia_power_broker::ElementControl>().value();
    to_return.element_control_client = std::move(endpoints.client);
    to_return.element_control_server = std::move(endpoints.server);
  }

  if (this->element_runner_.has_value()) {
    to_return.element_runner_client = std::move(this->element_runner_.value());
  } else {
    // make a channel instead, include it in output
    fidl::Endpoints<fuchsia_power_broker::ElementRunner> endpoints =
        fidl::CreateEndpoints<fuchsia_power_broker::ElementRunner>().value();
    to_return.element_runner_client = std::move(endpoints.client);
    to_return.element_runner_server = std::move(endpoints.server);
  }

  return to_return;
}

ElementDescBuilder& ElementDescBuilder::SetAssertiveToken(
    const zx::unowned_event& assertive_token) {
  zx::event dupe;
  assertive_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &dupe);
  assertive_token_ = std::move(dupe);
  return *this;
}

ElementDescBuilder& ElementDescBuilder::SetLessor(
    fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor) {
  lessor_ = std::move(lessor);
  return *this;
}

ElementDescBuilder& ElementDescBuilder::SetElementControl(
    fidl::ServerEnd<fuchsia_power_broker::ElementControl> element_control) {
  element_control_ = std::move(element_control);
  return *this;
}

ElementDescBuilder& ElementDescBuilder::SetElementRunner(
    fidl::ClientEnd<fuchsia_power_broker::ElementRunner> element_runner) {
  element_runner_ = std::move(element_runner);
  return *this;
}

}  // namespace fdf_power

#endif
