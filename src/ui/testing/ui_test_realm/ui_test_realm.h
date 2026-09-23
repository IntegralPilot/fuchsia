// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTING_UI_TEST_REALM_UI_TEST_REALM_H_
#define SRC_UI_TESTING_UI_TEST_REALM_UI_TEST_REALM_H_

#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/sys/cpp/service_directory.h>

#include <memory>
#include <optional>

#include "src/lib/fxl/macros.h"

namespace ui_testing {

// DPR constants.
constexpr auto kDefaultDevicePixelRatio = 1.f;
constexpr auto kMediumResolutionDevicePixelRatio = 1.25f;
constexpr auto kHighResolutionDevicePixelRatio = 2.f;

struct DisplayConfig {
  // If zero, uses component's default configuration value.
  // `active_width_px` and `active_height_px` must be either both non-zeroes
  // or both zeroes.
  uint32_t active_width_px = 0;
  uint32_t active_height_px = 0;

  // If zero, uses component's default configuration value.
  uint32_t refresh_rate_millihertz = 0;

  // If zero, uses component's default configuration value.
  uint32_t max_layer_count = 0;
};

// Library class to manage test realm on behalf of UI integration test clients.
class UITestRealm {
 public:
  enum class AccessibilityOwnerType {
    // Use the fake a11y manager. Clients should prefer using the fake a11y
    // manager for tests that require a11y services, but do not test a11y
    // functionality (e.g. tests that run a chromium client).
    FAKE = 1,

    // Use the real a11y manager. Clients should only use the real a11y manager
    // for tests that exercise accessibility-specific functionality.
    REAL = 2,
  };

  struct Config {
    // Specifies whether scene manager owns the root of the scene.
    // If false, then no scene owner will be present in the test realm.
    //
    // UITestManager assumes that scene manager serves input via input pipeline
    // in addition to managing scene ownership. Therefore it exposes the
    // following services out of the test from the top-level realm:
    //   * fuchsia.input.injection.InputDeviceRegistry
    //   * fuchsia.ui.policy.DeviceListenerRegistry
    //   * fuchsia.ui.pointerinjector.configuration.Setup
    //
    // Furthermore, if |use_scene_owner| is true, the client promises to expose
    // fuchsia.ui.app.ViewProvider from its subrealm.
    //
    // If |use_scene_owner| is false, the top-level realm exposes the raw scenic
    // input API:
    //   * fuchsia.ui.pointerinjector.Registry
    bool use_scene_owner = false;

    // Specifies the entity that owns accessibility in the test realm, if any.
    // If std::nullopt, then no a11y services will be present in the test realm.
    std::optional<AccessibilityOwnerType> accessibility_owner;

    // List of ui services required by components in the client subrealm.
    // UITestManager will route these services from the ui layer component to the
    // client subrealm.
    std::vector<std::string> ui_to_client_services;

    // List of capabilities to pass-through from the parent to the client subrealm.
    std::vector<component_testing::Capability> passthrough_capabilities;

    // List of non-ui services the test manager needs to expose to the test fixture.
    // By specifying services here, the client promises to expose them from its subrealm.
    std::vector<std::string> exposed_client_services;

    // List of client realm services to route to the ui layer component.
    //
    // *** Use cases for this field are ~very~ rare.
    // *** This optoin will NOT be available to OOT clients.
    std::vector<std::string> client_to_ui_services;

    // Clockwise display rotation, in degrees. Display rotation MUST be a multiple of 90 degrees.
    int display_rotation = 0;

    // Device pixel ratio for the fake display.
    //
    // Must result in integer logical display dimensions.
    float device_pixel_ratio = kDefaultDevicePixelRatio;

    // Idle threshold milliseconds for the activity service.
    int idle_threshold_ms = 5000;

    // Whether the power framework is treated as available.
    bool suspend_enabled = false;

    // Overrides the default display config.
    DisplayConfig display_config = {};
  };

  explicit UITestRealm(Config config);
  ~UITestRealm() = default;

  // Adds a child to the realm under construction, and returns the new child.
  // Must NOT be called after BuildRealm().
  component_testing::Realm AddSubrealm();

  // Calls realm_builder_.Build();
  void Build();

  // Calls realm_builder_.Teardown();
  void Teardown(component_testing::ScopedChild::TeardownCallback on_teardown_complete);

  // Returns a clone of the realm's exposed services directory.
  // Clients should call this method once, and retain the handle returned.
  //
  // MUST be called AFTER Build().
  std::unique_ptr<sys::ServiceDirectory> CloneExposedServicesDirectory();

  const std::optional<component_testing::RealmRoot>& realm_root() const { return realm_root_; }

  const Config& config() { return config_; }

 private:
  // Helper methods to configure the test realm.
  void ConfigureClientSubrealm();
  void ConfigureAccessibility();
  void RouteConfigData();
  void ConfigureScenic();
  void ConfigureSceneOwner();
  void ConfigureFakeDisplayStackHost();

  // Helper method to route a set of services from the specified source to the
  // spceified targets.
  void RouteServices(std::vector<std::string> services, component_testing::Ref source,
                     std::vector<component_testing::Ref> targets);

  // Helper method to determine the component url used to instantiate the base
  // UI realm.
  std::string CalculateBaseRealmUrl();

  Config config_;
  component_testing::RealmBuilder realm_builder_ =
      component_testing::RealmBuilder::CreateFromRelativeUrl(CalculateBaseRealmUrl());
  std::optional<component_testing::RealmRoot> realm_root_;

  // Some tests may not need a dedicated subrealm. Those clients will not call
  // AddSubrealm(), so UITestManager will crash if it tries to add routes
  // to/from the missing subrealm.
  //
  // NOTE: This piece of state is temporary, and can be removed once the client
  // owns a full RealmBuilder instance, as opposed to a child realm.
  bool has_client_subrealm_ = false;

  // Add state as necessary.

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(UITestRealm);
};

}  // namespace ui_testing

#endif  // SRC_UI_TESTING_UI_TEST_REALM_UI_TEST_REALM_H_
