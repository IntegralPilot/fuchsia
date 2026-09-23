// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TEST_REALM_REALM_BUILDER_CPP_BUILDER_H_
#define LIB_DRIVER_TEST_REALM_REALM_BUILDER_CPP_BUILDER_H_

#include <fidl/fuchsia.component.test/cpp/fidl.h>
#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>

namespace driver_test_realm {

// Options for the driver test realm being brought up.
struct Options {
  std::optional<bool> using_subpackage;
  std::optional<std::tuple<component_testing::Ref, std::vector<fuchsia_component_test::Capability>>>
      driver_offers;
  std::optional<std::vector<fuchsia_component_test::Capability>> driver_exposes;
  std::vector<std::tuple<fuchsia_component_test::Capability, component_testing::Ref>>
      extra_realm_capabilities;
};

// Builder class to make the |Options| type.
class OptionsBuilder {
 public:
  // Whether the driver test realm is included through a subpackage or as a direct dependency.
  // This is generally false for in-tree gn based tests, and true for bazel based tests.
  OptionsBuilder& using_subpackage(bool using_subpackage);

  // The list of capabilities offered to all drivers brought up in the test realm. These are all
  // provided by the given Ref.
  OptionsBuilder& driver_offers(component_testing::Ref provider,
                                const std::vector<fuchsia_component_test::Capability>& offers);

  // The list of capabilities exposed by drivers in the test realm to the test. To connect to these
  // from the test use the created realm root's exposed directory.
  OptionsBuilder& driver_exposes(const std::vector<fuchsia_component_test::Capability>& exposes);

  // Extra capabilities that are provided to the driver test realm in general. These are
  // optional capabilities that are by default routed from void, unless provided here.
  // For example:
  // fuchsia.tracing.provider.Registry
  OptionsBuilder& add_extra_realm_capability(fuchsia_component_test::Capability capability,
                                             component_testing::Ref provider);

  // Returns the options type.
  Options Build() const { return options_; }

 private:
  Options options_;
};

// Setup the driver test realm pieces in the realm builder. The |dispatcher| should be the same
// dispatcher given later to realm_builder's Build call. This dispatcher should generally be from
// an async::Loop that has had a thread started for it. This is where the dependencies of the
// driver test realm will run after Build is called.
void Setup(component_testing::RealmBuilder& realm_builder, async_dispatcher_t* dispatcher,
           Options options, fuchsia_driver_test::RealmArgs args);

// Waits for the boot-up of the driver test realm and drivers. This must be called after
// the realm_builder's Build call during test setup to ensure the realm is not destroyed
// before setup is complete.
zx::result<> WaitForBootup(component_testing::RealmRoot& realm_root);

// Wait for a node with the given moniker to be present in the driver framework's node topology.
zx::result<fuchsia_driver_development::NodeInfo> WaitForNode(
    component_testing::RealmRoot& realm_root, std::string_view moniker);

// This cleanly shuts down the driver test realm. It should be called at the end of a test or in
// the |TearDown| function. This guarantees nothing tries to reach into local servers that may no
// longer exist after returning from the test.
void ShutdownRealm(component_testing::RealmRoot& realm_root);

}  // namespace driver_test_realm

#endif  // LIB_DRIVER_TEST_REALM_REALM_BUILDER_CPP_BUILDER_H_
