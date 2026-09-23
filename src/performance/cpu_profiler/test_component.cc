// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test_component.h"

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>

// To profile a test, we're going to:
// 1) Request test_manager launch the test for us
// 2) Watch the root job for the url of the test we launched
// 3) Attach to the url when it shows up
//
// There doesn't seem to be a way to query test_manager for a handle/koid of the launched test nor a
// way to get the dynamically assigned moniker of the launched test so we're stuck with a slight
// work around to watch for the url.
zx::result<std::unique_ptr<profiler::TestComponent>> profiler::TestComponent::Create(
    async_dispatcher_t* dispatcher, std::string url,
    std::optional<fuchsia_test_manager::RunSuiteOptions> options, ComponentWatcher& event_stream) {
  std::unique_ptr test =
      std::make_unique<TestComponent>(dispatcher, std::move(url), std::move(options), event_stream);
  zx::result client_end = component::Connect<fuchsia_test_manager::SuiteRunner>();
  if (client_end.is_error()) {
    return client_end.take_error();
  }
  test->suite_runner_ = fidl::SyncClient{std::move(*client_end)};

  return zx::ok(std::move(test));
}

zx::result<> profiler::TestComponent::Start(fxl::WeakPtr<Sampler> notify) {
  if (!suite_runner_.is_valid()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  on_start_ = profiler::MakeOnStartHandler(std::move(notify));

  // Since we don't know the pid or tid or moniker, the only way to deterministically know what to
  // attach to would be to url, since we specify that. Normally, a moniker is preferred to a url
  // since that will be unique and we don't risk conflicting with another instance of the component
  // in a different realm. However, since it's a test component having another instance running
  // somewhere else is much less of an issue.
  if (auto res = component_watcher_.WatchForUrl(
          url_,
          [this](ComponentStartEvent event) {
            zx::result<> watch_for_children =
                TraverseRealm(event.moniker, [this](std::string moniker) {
                  return component_watcher_.WatchForMoniker(
                      std::move(moniker), [this](ComponentStartEvent child_event) {
                        on_start_.value()(std::move(child_event));
                      });
                });
            if (watch_for_children.is_error()) {
              FX_PLOGS(WARNING, watch_for_children.error_value())
                  << "Failed to watch for test's children!";
            }

            on_start_.value()(std::move(event));
          });
      res.is_error()) {
    return res.take_error();
  }

  FX_LOGS(INFO) << "Running test url=" << url_;

  auto [suite_controller_client, suite_controller_server] =
      fidl::Endpoints<fuchsia_test_manager::SuiteController>::Create();
  if (auto status =
          suite_runner_->Run({url_,
                              options_.has_value() ? std::move(options_.value())
                                                   : fuchsia_test_manager::RunSuiteOptions{},
                              std::move(suite_controller_server)});
      status.is_error()) {
    return zx::error(status.error_value().status());
  }
  suite_controller_ = fidl::Client{std::move(suite_controller_client), dispatcher_};

  // Once we launch the test, we need to listen to the events. We don't do anything with them right
  // now, we just need to ensure they don't get backed up.
  suite_controller_->GetEvents().Then(
      [this](fidl::Result<fuchsia_test_manager::SuiteController::GetEvents>& events) {
        if (events.is_ok()) {
          OnSuiteEvent(std::move(events->events()));
        }
      });

  return zx::ok();
}

zx::result<> profiler::TestComponent::Stop() {
  // Forward the stop request to test_manager, then reset our bindings.
  auto d = fit::defer(
      [this]() { suite_controller_ = fidl::Client<fuchsia_test_manager::SuiteController>{}; });

  if (suite_controller_.is_valid()) {
    if (fit::result res = suite_controller_->Kill(); res.is_error()) {
      return zx::error(res.error_value().status());
    }
  }
  return zx::ok();
}

zx::result<> profiler::TestComponent::Destroy() { return zx::ok(); }

void profiler::TestComponent::OnSuiteEvent(std::vector<fuchsia_test_manager::SuiteEvent> events) {
  if (events.empty()) {
    return;
  }
  suite_controller_->GetEvents().Then(
      [this](fidl::Result<fuchsia_test_manager::SuiteController::GetEvents>& events) {
        if (events.is_ok()) {
          this->OnSuiteEvent(std::move(events->events()));
        }
      });
}

void profiler::TestComponent::handle_unknown_event(
    fidl::UnknownEventMetadata<fuchsia_test_manager::SuiteController> metadata) {
  FX_LOGS(WARNING) << "Unknown SuiteController event: " << metadata.event_ordinal;
}
