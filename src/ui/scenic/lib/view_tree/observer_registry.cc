// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/observer_registry.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

namespace view_tree {

Registry::Registry(view_tree::GeometryProvider& geometry_provider)
    : geometry_provider_(geometry_provider) {}

void Registry::RegisterGlobalViewTreeWatcher(
    RegisterGlobalViewTreeWatcherRequestView request,
    RegisterGlobalViewTreeWatcherCompleter::Sync& completer) {
  utils::CheckIsOnInputThread();
  geometry_provider_.RegisterGlobalViewTreeWatcher(std::move(request->watcher));
  completer.Reply();
}

void Registry::Bind(fidl::ServerEnd<fuchsia_ui_observation_test::Registry> server_end) {
  utils::CheckIsOnInputThread();
  bindings_.AddBinding(async_get_default_dispatcher(), std::move(server_end), this,
                       fidl::kIgnoreBindingClosure);
}

}  // namespace view_tree
