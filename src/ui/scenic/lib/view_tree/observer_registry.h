// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_VIEW_TREE_OBSERVER_REGISTRY_H_
#define SRC_UI_SCENIC_LIB_VIEW_TREE_OBSERVER_REGISTRY_H_

#include <fidl/fuchsia.ui.observation.test/cpp/fidl.h>
#include <lib/fidl/cpp/wire/server.h>

#include "src/ui/scenic/lib/view_tree/geometry_provider.h"

namespace view_tree {

// The Registry class allows a client to receive global view geometry updates, in conjunction with
// the |fuchsia.ui.observation.geometry.ViewTreeWatcher| protocol.
//
// This is a sensitive protocol, so it should only be used in tests.
class Registry : public fidl::WireServer<fuchsia_ui_observation_test::Registry> {
 public:
  // Sets up forwarding of geometry requests to the geometry provider manager.
  explicit Registry(view_tree::GeometryProvider& geometry_provider);

  // |fidl::WireServer<fuchsia_ui_observation_test::Registry>|
  void RegisterGlobalViewTreeWatcher(
      RegisterGlobalViewTreeWatcherRequestView request,
      RegisterGlobalViewTreeWatcherCompleter::Sync& completer) override;

  void Bind(fidl::ServerEnd<fuchsia_ui_observation_test::Registry> server_end);

 private:
  fidl::ServerBindingGroup<fuchsia_ui_observation_test::Registry> bindings_;

  view_tree::GeometryProvider& geometry_provider_;
};

}  // namespace view_tree

#endif  // SRC_UI_SCENIC_LIB_VIEW_TREE_OBSERVER_REGISTRY_H_
