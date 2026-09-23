// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_EXAMPLES_ESCHER_COMMON_DEMO_HARNESS_FUCHSIA_H_
#define SRC_UI_EXAMPLES_ESCHER_COMMON_DEMO_HARNESS_FUCHSIA_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/trace-provider/provider.h>

#include <memory>

#if defined(FUCHSIA_USE_SCENIC)
#include <fidl/fuchsia.element/cpp/fidl.h>  // nogncheck
#endif

#include "src/ui/examples/escher/common/demo_harness.h"

class DemoHarnessFuchsia : public DemoHarness {
 public:
  DemoHarnessFuchsia(async::Loop* loop, WindowParams window_params);

  sys::ComponentContext* component_context() { return component_context_.get(); }

 private:
  // |DemoHarness|
  std::string GetCacheDirectoryPath() override;

  // |DemoHarness|
  // Called by Init().
  void InitWindowSystem() override;
  vk::SurfaceKHR CreateWindowAndSurface(const WindowParams& window_params) override;

  // |DemoHarness|
  // Called by Init() via CreateInstance().
  void AppendPlatformSpecificInstanceExtensionNames(InstanceParams* params) override;
  void AppendPlatformSpecificDeviceExtensionNames(std::set<std::string>* names) override;

  // |DemoHarness|
  // Called by Shutdown().
  void ShutdownWindowSystem() override;

  // |DemoHarness|
  // Called by Run().
  void RunForPlatform(Demo* demo) override;

  void RenderFrameOrQuit(Demo* demo);

  // DemoHarnessFuchsia can work with a pre-existing message loop, and also
  // create its own if necessary.
  std::unique_ptr<async::Loop> owned_loop_;
  async::Loop* const loop_;
  std::unique_ptr<trace::TraceProviderWithFdio> trace_provider_;

  std::unique_ptr<sys::ComponentContext> component_context_;

#if defined(FUCHSIA_USE_SCENIC)
  fidl::Client<fuchsia_element::GraphicalPresenter> presenter_;
#endif
};

#endif  // SRC_UI_EXAMPLES_ESCHER_COMMON_DEMO_HARNESS_FUCHSIA_H_
