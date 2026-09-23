// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/ui/composition/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/a11y/lib/view/flatland_accessibility_view.h"
#include "src/ui/a11y/testing/fake_a11y_manager.h"

namespace {

int run_a11y_manager(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto context = sys::ComponentContext::Create();

  fuchsia::ui::composition::FlatlandFactoryPtr flatland_factory =
      context->svc()->Connect<fuchsia::ui::composition::FlatlandFactory>();
  flatland_factory.set_error_handler([&](zx_status_t status) {
    FX_PLOGS(ERROR, status) << "flatland_factory connection closed; exiting";
    loop.Quit();
  });
  auto make_flatland = [&]() {
    fuchsia::ui::composition::FlatlandPtr flatland;
    flatland_factory->CreateFlatland(flatland.NewRequest(), {}, [](auto) {});
    flatland.set_error_handler([&](zx_status_t status) {
      FX_PLOGS(ERROR, status) << "flatland connection closed; exiting";
      loop.Quit();
    });
    return flatland;
  };
  std::unique_ptr<a11y::FlatlandAccessibilityView> a11y_view =
      std::make_unique<a11y::FlatlandAccessibilityView>(
          make_flatland(), make_flatland(),
          context->svc()->Connect<fuchsia::ui::observation::scope::Registry>(),
          context->svc()->Connect<fuchsia::ui::pointer::augment::LocalHit>());
  context->outgoing()->AddPublicService(a11y_view->GetHandler());

  a11y_testing::FakeA11yManager fake_a11y_manager;
  context->outgoing()->AddPublicService(fake_a11y_manager.GetHandler());

  a11y_testing::FakeMagnifier fake_magnifier = a11y_testing::FakeMagnifier(std::move(a11y_view));
  context->outgoing()->AddPublicService(fake_magnifier.GetTestMagnifierHandler());
  context->outgoing()->AddPublicService(fake_magnifier.GetMagnifierHandler());

  context->outgoing()->ServeFromStartupInfo();

  loop.Run();
  return 0;
}

}  // namespace

int main(int argc, const char** argv) { return run_a11y_manager(argc, argv); }
