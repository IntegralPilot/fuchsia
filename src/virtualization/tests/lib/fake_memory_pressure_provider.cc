// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fake_memory_pressure_provider.h"

#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

namespace {
class MemoryPressureComponent : public component_testing::LocalComponentImpl {
 public:
  explicit MemoryPressureComponent(FakeMemoryPressureProvider& provider) : provider_(provider) {}

  void OnStart() override {
    ASSERT_EQ(outgoing()->AddPublicService(provider_.GetHandler()), ZX_OK);
  }

 private:
  FakeMemoryPressureProvider& provider_;
};
}  // namespace

void FakeMemoryPressureProvider::RegisterWatcher(
    ::fidl::InterfaceHandle<::fuchsia::memorypressure::Watcher> watcher) {
  fuchsia::memorypressure::WatcherPtr watcher_proxy = watcher.Bind();
  watcher_proxy->OnLevelChanged(::fuchsia::memorypressure::Level::NORMAL, []() {});
  watchers_.push_back(std::move(watcher_proxy));
}

std::unique_ptr<component_testing::LocalComponentImpl> FakeMemoryPressureProvider::NewComponent() {
  return std::make_unique<MemoryPressureComponent>(*this);
}

void FakeMemoryPressureProvider::OnLevelChanged(::fuchsia::memorypressure::Level level) {
  for (auto& watcher : watchers_) {
    watcher->OnLevelChanged(level, []() {});
  }
}
