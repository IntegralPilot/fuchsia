// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_VIEW_TREE_SCOPED_OBSERVER_REGISTRY_H_
#define SRC_UI_SCENIC_LIB_VIEW_TREE_SCOPED_OBSERVER_REGISTRY_H_

#include <fidl/fuchsia.ui.observation.scope/cpp/fidl.h>
#include <lib/fidl/cpp/wire/server.h>

#include "src/ui/scenic/lib/view_tree/geometry_provider.h"

namespace view_tree {

// The Registry class allows a client to receive scoped view geometry updates, in conjunction with
// the |fuchsia.ui.observation.geometry.ViewTreeWatcher| protocol.
class ScopedRegistry : public fidl::WireServer<fuchsia_ui_observation_scope::Registry> {
 public:
  // Sets up forwarding of geometry requests to the geometry provider manager.
  explicit ScopedRegistry(view_tree::GeometryProvider& geometry_provider);

  // |fidl::WireServer<fuchsia_ui_observation_scope::Registry>|
  void RegisterScopedViewTreeWatcher(
      RegisterScopedViewTreeWatcherRequestView request,
      RegisterScopedViewTreeWatcherCompleter::Sync& completer) override;

  void Bind(fidl::ServerEnd<fuchsia_ui_observation_scope::Registry> server_end);

 private:
  fidl::ServerBindingGroup<fuchsia_ui_observation_scope::Registry> bindings_;

  view_tree::GeometryProvider& geometry_provider_;
};

}  // namespace view_tree

#endif  // SRC_UI_SCENIC_LIB_VIEW_TREE_SCOPED_OBSERVER_REGISTRY_H_
