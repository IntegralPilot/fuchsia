// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.accessibility.semantics/cpp/fidl.h>
#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.input.virtualkeyboard/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.input3/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointerinjector/cpp/fidl.h>
#include <fidl/fuchsia.ui.policy/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <fidl/test.accessibility/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <cstring>

#include <src/ui/testing/test_ui_stack/test_ui_stack_config_lib.h>

#include "fidl/fuchsia.input.virtualkeyboard/cpp/markers.h"
#include "src/ui/testing/ui_test_realm/ui_test_realm.h"

namespace {

template <typename T>
void AddPublicService(sys::ComponentContext* context,
                      sys::ServiceDirectory* realm_exposed_services) {
  FX_CHECK(realm_exposed_services);

  context->outgoing()->AddPublicService(
      fidl::InterfaceRequestHandler<T>([realm_exposed_services](fidl::InterfaceRequest<T> request) {
        realm_exposed_services->Connect(std::move(request), fidl::DiscoverableProtocolName<T>);
      }),
      fidl::DiscoverableProtocolName<T>);
}

int run_test_ui_stack(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FX_LOGS(INFO) << "Test UI stack starting";
  auto context = sys::ComponentContext::Create();

  // Read component configuration, and convert to UITestRealm::Config.
  auto test_ui_stack_config = test_ui_stack_config_lib::Config::TakeFromStartupHandle();
  ui_testing::UITestRealm::Config config;

  config.use_scene_owner = true;
  config.accessibility_owner = ui_testing::UITestRealm::AccessibilityOwnerType::FAKE;
  config.display_rotation = test_ui_stack_config.display_rotation();
  config.device_pixel_ratio = std::stof(test_ui_stack_config.device_pixel_ratio());
  config.suspend_enabled = test_ui_stack_config.suspend_enabled();
  config.display_config = {
      .active_width_px = test_ui_stack_config.display_width_px(),
      .active_height_px = test_ui_stack_config.display_height_px(),
      .refresh_rate_millihertz = test_ui_stack_config.display_refresh_rate_millihertz(),
      .max_layer_count = test_ui_stack_config.display_max_layer_count(),
  };

  // Build test realm.
  ui_testing::UITestRealm realm(config);
  realm.Build();
  auto realm_exposed_services = realm.CloneExposedServicesDirectory();

  // Bind incoming service requests to realm's exposed services directory.

  // Base UI services.
  AddPublicService<fuchsia_accessibility_semantics::SemanticsManager>(context.get(),
                                                                      realm_exposed_services.get());
  AddPublicService<fuchsia_element::GraphicalPresenter>(context.get(),
                                                        realm_exposed_services.get());
  AddPublicService<fuchsia_ui_composition::Allocator>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_ui_composition::Flatland>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_ui_composition::FlatlandFactory>(context.get(),
                                                            realm_exposed_services.get());
  AddPublicService<fuchsia_ui_composition::TrustedFlatlandFactory>(context.get(),
                                                                   realm_exposed_services.get());
  AddPublicService<fuchsia_ui_focus::FocusChainListenerRegistry>(context.get(),
                                                                 realm_exposed_services.get());
  AddPublicService<fuchsia_ui_input::ImeService>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_ui_input3::Keyboard>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_ui_input3::KeyEventInjector>(context.get(),
                                                        realm_exposed_services.get());
  AddPublicService<fuchsia_ui_pointerinjector::Registry>(context.get(),
                                                         realm_exposed_services.get());
  AddPublicService<fuchsia_ui_input::DeviceListenerRegistry>(context.get(),
                                                             realm_exposed_services.get());
  AddPublicService<fuchsia_ui_policy::DeviceListenerRegistry>(context.get(),
                                                              realm_exposed_services.get());
  AddPublicService<fuchsia_ui_composition::ScreenCapture>(context.get(),
                                                          realm_exposed_services.get());
  AddPublicService<fuchsia_ui_composition::Screenshot>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_ui_display_singleton::DisplayPower>(context.get(),
                                                               realm_exposed_services.get());
  AddPublicService<fuchsia_ui_display_singleton::Info>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_ui_display_singleton::VsyncSource>(context.get(),
                                                              realm_exposed_services.get());

  // Helper services.
  AddPublicService<fuchsia_ui_test_input::Registry>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_ui_test_scene::Controller>(context.get(), realm_exposed_services.get());
  AddPublicService<fuchsia_input_virtualkeyboard::Manager>(context.get(),
                                                           realm_exposed_services.get());
  AddPublicService<fuchsia_input_virtualkeyboard::ControllerCreator>(context.get(),
                                                                     realm_exposed_services.get());
  if (config.accessibility_owner == ui_testing::UITestRealm::AccessibilityOwnerType::FAKE) {
    AddPublicService<test_accessibility::Magnifier>(context.get(), realm_exposed_services.get());
  }

  context->outgoing()->ServeFromStartupInfo();

  loop.Run();
  return 0;

  FX_LOGS(INFO) << "Test UI stack exiting";
}

}  // namespace

int main(int argc, const char** argv) { return run_test_ui_stack(argc, argv); }
