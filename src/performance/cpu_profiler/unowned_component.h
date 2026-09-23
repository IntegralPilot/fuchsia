// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_UNOWNED_COMPONENT_H_
#define SRC_PERFORMANCE_CPU_PROFILER_UNOWNED_COMPONENT_H_

#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/result.h>

#include "component.h"
#include "component_watcher.h"

namespace profiler {

// A component that is to be profiled, but who's lifecycle is not controlled by the profiler
class UnownedComponent : public ComponentTarget {
 public:
  // An unowned component needs to be described by at least one of a moniker or url.
  explicit UnownedComponent(std::optional<std::string> url, std::optional<Moniker> moniker,
                            ComponentWatcher& component_watcher)
      : component_watcher_{component_watcher}, moniker_{std::move(moniker)}, url_{std::move(url)} {}
  static zx::result<std::unique_ptr<UnownedComponent>> Create(
      const std::optional<std::string>& moniker, const std::optional<std::string>& url,
      ComponentWatcher& component_watcher);
  zx::result<> Start(fxl::WeakPtr<Sampler> notify) override;
  zx::result<> Stop() override;
  zx::result<> Destroy() override;

 private:
  // Start watching for a component at a `moniker`. Calls on_start_ when a component is created at
  // the requested moniker or as a child of the moniker.
  zx::result<> Attach(const fidl::SyncClient<fuchsia_sys2::RealmQuery>& client,
                      const Moniker& moniker);
  // LIFETIME: Reference is to a ComponentWatcher created in main() which lives for the life of the
  // program.
  ComponentWatcher& component_watcher_;
  std::optional<Moniker> moniker_;
  std::optional<std::string> url_;
  std::optional<ComponentWatcher::ComponentEventHandler> on_start_;
};

}  // namespace profiler

#endif  // SRC_PERFORMANCE_CPU_PROFILER_UNOWNED_COMPONENT_H_
