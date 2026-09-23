// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/scoped_observer_registry.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

namespace view_tree {

ScopedRegistry::ScopedRegistry(view_tree::GeometryProvider& geometry_provider)
    : geometry_provider_(geometry_provider) {}

void ScopedRegistry::RegisterScopedViewTreeWatcher(
    RegisterScopedViewTreeWatcherRequestView request,
    RegisterScopedViewTreeWatcherCompleter::Sync& completer) {
  utils::CheckIsOnInputThread();
  geometry_provider_.Register(std::move(request->watcher), request->context_view);
  completer.Reply();
}

void ScopedRegistry::Bind(fidl::ServerEnd<fuchsia_ui_observation_scope::Registry> server_end) {
  utils::CheckIsOnInputThread();
  bindings_.AddBinding(async_get_default_dispatcher(), std::move(server_end), this,
                       fidl::kIgnoreBindingClosure);
}

}  // namespace view_tree
