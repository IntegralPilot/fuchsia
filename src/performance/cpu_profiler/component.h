// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_COMPONENT_H_
#define SRC_PERFORMANCE_CPU_PROFILER_COMPONENT_H_

#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/result.h>

#include <string>

#include "component_watcher.h"
#include "sampler.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace profiler {

// A component moniker as specified in //docs/reference/components/moniker.md.
struct Moniker {
  std::optional<std::string> parent;
  std::optional<std::string> collection;
  std::string name;

  std::string ToString() const {
    std::string moniker_string;
    if (parent.has_value()) {
      moniker_string += *parent;
      moniker_string += "/";
    }
    if (collection.has_value()) {
      moniker_string += *collection;
      moniker_string += ":";
    }
    moniker_string += name;
    return moniker_string;
  }
  static zx::result<Moniker> Parse(std::string_view moniker);
};

profiler::ComponentWatcher::ComponentEventHandler MakeOnStartHandler(
    fxl::WeakPtr<profiler::Sampler> sampler);

// Given a moniker `moniker` and function `f`, call `f` on `moniker` and each child of `moniker`.
//
// The results are `and`ed together, short circuiting and returning early on any failure.
zx::result<> TraverseRealm(const std::string& moniker,
                           const fit::function<zx::result<>(const std::string& moniker)>& f);

class ComponentTarget {
 public:
  // Do any required initialization for the component, and begin watching the component. Add the
  // component to `notify` when the component (and any of its children) are ready to be profiled.
  virtual zx::result<> Start(fxl::WeakPtr<Sampler> notify) = 0;
  // Detach from the component -- stop emitting calls to on start.
  virtual zx::result<> Stop() = 0;
  // Tear down and clean up this component if needed.
  virtual zx::result<> Destroy() = 0;

  virtual ~ComponentTarget() = default;
};

class ControlledComponent : public ComponentTarget {
 public:
  explicit ControlledComponent(std::string url, Moniker moniker,
                               ComponentWatcher& component_watcher)
      : ComponentTarget{},
        url_{std::move(url)},
        moniker_{std::move(moniker)},
        component_watcher_(component_watcher) {}
  static zx::result<std::unique_ptr<ControlledComponent>> Create(
      const std::string& url, const std::string& moniker, ComponentWatcher& component_watcher);

  zx::result<> Start(fxl::WeakPtr<Sampler> notify) override;
  zx::result<> Stop() override;
  zx::result<> Destroy() override;

  ~ControlledComponent() override;

 private:
  std::optional<ComponentWatcher::ComponentEventHandler> on_start_;
  std::string url_;
  Moniker moniker_;

  // LIFETIME: Reference is to a ComponentWatcher created in main() which lives for the life of the
  // program.
  ComponentWatcher& component_watcher_;
  // Ensure that we only destroy this component once -- either explicitly through Destroy, or
  // implicitly in ~ControlledComponent.
  bool needs_destruction_ = true;

  fidl::SyncClient<fuchsia_sys2::LifecycleController> lifecycle_controller_client_;
};

}  // namespace profiler

#endif  // SRC_PERFORMANCE_CPU_PROFILER_COMPONENT_H_
