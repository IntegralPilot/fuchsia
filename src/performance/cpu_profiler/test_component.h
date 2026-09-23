// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_TEST_COMPONENT_H_
#define SRC_PERFORMANCE_CPU_PROFILER_TEST_COMPONENT_H_

#include <fidl/fuchsia.test.manager/cpp/fidl.h>
#include <lib/zx/job.h>
#include <lib/zx/result.h>

#include <string>
#include <vector>

#include "component.h"
#include "component_watcher.h"

namespace profiler {

class TestComponent : public fidl::AsyncEventHandler<fuchsia_test_manager::SuiteController>,
                      public ComponentTarget {
 public:
  using OnStartedHandler = fit::function<void(zx_koid_t job_id, std::string name)>;

  explicit TestComponent(async_dispatcher_t* dispatcher, std::string url,
                         std::optional<fuchsia_test_manager::RunSuiteOptions> options,
                         ComponentWatcher& component_watcher)
      : dispatcher_{dispatcher},
        options_{std::move(options)},
        url_{std::move(url)},
        component_watcher_{component_watcher} {}
  TestComponent(TestComponent&&) = delete;
  TestComponent(const TestComponent&) = delete;

  static zx::result<std::unique_ptr<TestComponent>> Create(
      async_dispatcher_t* dispatcher, std::string url,
      std::optional<fuchsia_test_manager::RunSuiteOptions> options, ComponentWatcher& event_stream);

  zx::result<> Start(fxl::WeakPtr<Sampler> notify) override;
  zx::result<> Stop() override;
  zx::result<> Destroy() override;

 private:
  void OnSuiteEvent(std::vector<fuchsia_test_manager::SuiteEvent>);
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_test_manager::SuiteController> metadata) override;

  async_dispatcher_t* dispatcher_;
  fidl::SyncClient<fuchsia_test_manager::SuiteRunner> suite_runner_;
  fidl::Client<fuchsia_test_manager::SuiteController> suite_controller_;
  std::optional<fuchsia_test_manager::RunSuiteOptions> options_;
  std::string url_;

  // LIFETIME: Reference is to a ComponentWatcher created in main() which lives for the life of the
  // program.
  ComponentWatcher& component_watcher_;
  std::optional<ComponentWatcher::ComponentEventHandler> on_start_;
};
}  // namespace profiler
#endif  // SRC_PERFORMANCE_CPU_PROFILER_TEST_COMPONENT_H_
