// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_VIEW_TREE_VIEW_REF_INSTALLED_IMPL_H_
#define SRC_UI_SCENIC_LIB_VIEW_TREE_VIEW_REF_INSTALLED_IMPL_H_

#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/fidl/cpp/wire/server.h>
#include <zircon/types.h>

#include <unordered_map>
#include <unordered_set>
#include <vector>

#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

namespace view_tree {

// Class that implements the ViewRefInstalled service.
class ViewRefInstalledImpl : public fidl::WireServer<fuchsia_ui_views::ViewRefInstalled> {
 public:
  explicit ViewRefInstalledImpl(
      std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder = nullptr);

  void Bind(fidl::ServerEnd<fuchsia_ui_views::ViewRefInstalled> server_end);

  // |fidl::WireServer<fuchsia_ui_views::ViewRefInstalled>|
  void Watch(WatchRequestView request, WatchCompleter::Sync& completer) override;

  // Called whenever a new snapshot of the ViewTree is generated.
  // When this happens we look through it to check if any of the waited on views have been installed
  // or any installed views have been removed entirely.
  void OnNewViewTreeSnapshot();

 private:
  // Struct to track for when a view ref gets invalidated.
  struct ViewRefInvalidationWaiter {
    explicit ViewRefInvalidationWaiter(zx::eventpair eventpair)
        : waiter(eventpair.get(), ZX_EVENTPAIR_PEER_CLOSED), eventpair(std::move(eventpair)) {}
    ~ViewRefInvalidationWaiter() { waiter.Cancel(); }

    async::WaitOnce waiter;
    zx::eventpair eventpair;  // Keep a reference in case this is the last ViewRef.
  };

  // Tracks uninstalled views with Watch() calls waiting on them.
  struct WatchedView {
    explicit WatchedView(zx::eventpair eventpair) : invalidation_waiter(std::move(eventpair)) {}

    // Waiters that tracks when ViewRefs gets invalidated.
    // We keep a single waiter per watched ViewRef.
    ViewRefInvalidationWaiter invalidation_waiter;

    // All pending completers from Watch() calls for this ViewRef.
    std::vector<WatchCompleter::Async> completers;
  };

  // Called when |view_ref_koid| is observed in the ViewTree for the first time.
  void OnViewRefInstalled(zx_koid_t view_ref_koid);

  // Fired by |invalidation_waiters_| when a ViewRef signals ZX_ERR_PEER_CLOSED.
  void OnViewRefInvalidated(zx_koid_t view_ref_koid, zx_status_t status,
                            const zx_packet_signal* signal);

  fidl::ServerBindingGroup<fuchsia_ui_views::ViewRefInstalled> bindings_;

  // All views currently being Watch()ed.
  std::unordered_map<zx_koid_t, WatchedView> watched_views_;

  // The set of active views (i.e. extant in the latest snapshot, either in view_tree or
  // unconnected_views) that have at some point been installed in the view tree.
  std::unordered_set<zx_koid_t> installed_views_;
  std::shared_ptr<SnapshotHolder> snapshot_holder_;
  uint64_t latest_sequence_number_ = 0;
};

}  // namespace view_tree

#endif  // SRC_UI_SCENIC_LIB_VIEW_TREE_VIEW_REF_INSTALLED_IMPL_H_
