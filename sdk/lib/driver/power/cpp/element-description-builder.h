// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_ELEMENT_DESCRIPTION_BUILDER_H_
#define LIB_DRIVER_POWER_CPP_ELEMENT_DESCRIPTION_BUILDER_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/driver/power/cpp/types.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_power {

using TokenMap = std::unordered_map<ParentElement, zx::event>;

using ElementDependencyMap =
    std::unordered_map<ParentElement, std::vector<fuchsia_power_broker::LevelDependency>>;

struct ElementDesc {
  PowerElementConfiguration element_config;
  TokenMap tokens;
  zx::event assertive_token;
  fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor_server;
  fidl::ServerEnd<fuchsia_power_broker::ElementControl> element_control_server;
  std::optional<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>> element_runner_client;
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> element_runner_server;

  // The below are created if the caller did not supply their corresponding server end
  std::optional<fidl::ClientEnd<fuchsia_power_broker::Lessor>> lessor_client;
  std::optional<fidl::ClientEnd<fuchsia_power_broker::ElementControl>> element_control_client;
};

class ElementDescBuilder {
 public:
  explicit ElementDescBuilder(PowerElementConfiguration config, TokenMap tokens)
      : element_config(std::move(config)), tokens_(std::move(tokens)) {}

  /// Build an `ElementDesc` object based on the information we've been given.
  ///
  /// If the assertive token is not set, a `zx::event` object will be created.
  ///
  /// If the lessor channel is not set, it is created.  The `fidl::ClientEnd` of this channel is
  /// placed in the `lessor_client_` field of the `ElementDesc` object returned.
  /// Similarly, if element runner is not set, the channels will be created and placed in the
  /// `element_runner_` field.
  ElementDesc Build();

  /// Sets the assertive token to associate with this element by duplicating
  /// the token passed in.
  ElementDescBuilder& SetAssertiveToken(const zx::unowned_event& assertive_token);

  /// Sets the channel to use for the Lessor protocol.
  ElementDescBuilder& SetLessor(fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor);

  /// Sets the channel to use for the ElementControl protocol.
  ElementDescBuilder& SetElementControl(
      fidl::ServerEnd<fuchsia_power_broker::ElementControl> element_control);

  /// Sets the channel to use for the ElementRunner protocol.
  ElementDescBuilder& SetElementRunner(
      fidl::ClientEnd<fuchsia_power_broker::ElementRunner> element_runner);

 private:
  PowerElementConfiguration element_config;
  TokenMap tokens_;
  std::optional<zx::event> assertive_token_;
  std::optional<fidl::ServerEnd<fuchsia_power_broker::Lessor>> lessor_;
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementControl>> element_control_;
  std::optional<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>> element_runner_;
};

}  // namespace fdf_power

#endif

#endif  // LIB_DRIVER_POWER_CPP_ELEMENT_DESCRIPTION_BUILDER_H_
