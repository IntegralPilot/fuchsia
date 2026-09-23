// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include <cstring>

#include "src/intl/intl_services/run.h"
#include "src/ui/a11y/bin/a11y_manager/app.h"
#include "src/ui/a11y/lib/screen_reader/screen_reader_context.h"
#include "src/ui/a11y/lib/semantics/a11y_semantics_event_manager.h"
#include "src/ui/a11y/lib/util/boot_info_manager.h"
#include "src/ui/a11y/lib/view/a11y_view_semantics.h"
#include "src/ui/a11y/lib/view/flatland_accessibility_view.h"
#include "src/ui/a11y/lib/view/view_injector_factory.h"

namespace {

int run_a11y_manager(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  trace::TraceProviderWithFdio trace_provider(loop.dispatcher());

  auto context = sys::ComponentContext::Create();
  auto inspector = inspect::ComponentInspector(loop.dispatcher(), {});
  inspector.Health().StartingUp();
  inspector.Health().Ok();

  std::unique_ptr<a11y::ViewManager> view_manager;
  std::unique_ptr<a11y_manager::App> app;

  a11y::TtsManager tts_manager(context.get());
  a11y::ColorTransformManager color_transform_manager(loop.dispatcher(), context.get());
  a11y::GestureListenerRegistry gesture_listener_registry;
  a11y::BootInfoManager boot_info_manager(context.get());
  a11y::ScreenReaderContextFactory screen_reader_context_factory;

  auto local_hit = context->svc()->Connect<fuchsia::ui::pointer::augment::LocalHit>();
  local_hit.set_error_handler([&loop](zx_status_t status) {
    FX_PLOGS(ERROR, status) << "LocalHit connection closed; exiting";
    loop.Quit();
  });

  auto make_flatland = [&]() {
    fidl::InterfacePtr flatland = context->svc()->Connect<fuchsia::ui::composition::Flatland>();
    flatland.set_error_handler([&](zx_status_t status) {
      FX_PLOGS(ERROR, status) << "flatland connection closed; exiting";
      loop.Quit();
    });
    return flatland;
  };
  auto a11y_view = std::make_shared<a11y::FlatlandAccessibilityView>(
      make_flatland(), make_flatland(),
      context->svc()->Connect<fuchsia::ui::observation::scope::Registry>(),
      context->svc()->Connect<fuchsia::ui::pointer::augment::LocalHit>());
  context->outgoing()->AddPublicService(a11y_view->GetHandler());

  view_manager = std::make_unique<a11y::ViewManager>(
      std::make_unique<a11y::SemanticTreeServiceFactory>(
          inspector.root().CreateChild("semantic_trees")),
      std::make_unique<a11y::A11yViewSemanticsFactory>(),
      std::make_unique<a11y::ViewInjectorFactory>(),
      std::make_unique<a11y::A11ySemanticsEventManager>(), a11y_view, context.get());

  app = std::make_unique<a11y_manager::App>(context.get(), view_manager.get(), &tts_manager,
                                            &color_transform_manager, &gesture_listener_registry,
                                            &boot_info_manager, &screen_reader_context_factory,
                                            inspector.root().CreateChild("a11y_manager_app"));
  context->outgoing()->ServeFromStartupInfo();

  loop.Run();

  FX_LOGS(INFO) << "A11y manager exiting";
  return 0;
}

}  // namespace

int main(int argc, const char** argv) {
  if (strcmp(argv[0], "/pkg/bin/intl_services") == 0) {
    // If the binary was started as intl_services, run only that part of it.
    exit(intl::serve_fuchsia_intl_services(argc, argv));
  }
  return run_a11y_manager(argc, argv);
}
