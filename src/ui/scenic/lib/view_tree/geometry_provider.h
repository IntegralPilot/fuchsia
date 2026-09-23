// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_VIEW_TREE_GEOMETRY_PROVIDER_H_
#define SRC_UI_SCENIC_LIB_VIEW_TREE_GEOMETRY_PROVIDER_H_

#include <fidl/fuchsia.ui.observation.geometry/cpp/fidl.h>
#include <lib/fidl/cpp/wire/server.h>

#include <deque>
#include <memory>
#include <optional>
#include <unordered_map>

#include "src/lib/fxl/macros.h"
#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

namespace view_tree {
// This class is responsible for registering and maintaining server endpoints for
// fuchsia.ui.observation.geometry.ViewTreeWatcher protocol clients. This class also listens for new
// snapshots generated every frame, and sends a processed version of them to these registered
// clients.
class GeometryProvider {
 public:
  explicit GeometryProvider(std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder);
  // Adds a server side endpoint to |endpoints_| for lifecycle management.
  void Register(fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint,
                zx_koid_t context_view);

  // Adds a server side endpoint provided by
  // fuchsia.ui.observation.test.Registry.RegisterGlobalViewTreeWatcher to |endpoints_|. Endpoints
  // registered by this method get a global access to the view tree.
  void RegisterGlobalViewTreeWatcher(
      fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint);

  void OnNewViewTreeSnapshot();

  // Generates a fuchsia_ui_observation_geometry::ViewTreeSnapshot from the |snapshot| by
  // extracting information about the |context_view| and its descendant views from
  // |snapshot|.
  static std::optional<fuchsia_ui_observation_geometry::ViewTreeSnapshot>
  ExtractObservationSnapshot(std::optional<zx_koid_t> endpoint_context_view,
                             const view_tree::Snapshot& snapshot);

 private:
  using ProviderEndpointId = int64_t;

  // This class implements the server side endpoint for
  // fuchsia.ui.observation.geometry.ViewTreeWatcher clients and manages a deque of snapshot updates
  // to be sent to the client on receiving a Watch() call.
  class ProviderEndpoint
      : public fidl::WireServer<fuchsia_ui_observation_geometry::ViewTreeWatcher> {
   public:
    explicit ProviderEndpoint(
        fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint,
        std::optional<zx_koid_t> context_view, fit::function<void()> destroy_instance_function);

    ~ProviderEndpoint() override;

    // |fidl::WireServer<fuchsia_ui_observation_geometry::ViewTreeWatcher>|
    void Watch(WatchCompleter::Sync& completer) override;

    // Adds the latest snapshot to |view_tree_snapshots_|.
    //
    // If the size of |view_tree_snapshots_| exceeds |fuchsia_ui_observation_geometry::kBufferSize|,
    // it replaces the oldest snapshot with the new one. If there was any pending callback because
    // of a client calling Watch() when there were no pending snapshots, it gets triggered with the
    // latest |view_tree_snapshots_|.
    void AddViewTreeSnapshot(
        std::optional<fuchsia_ui_observation_geometry::ViewTreeSnapshot> view_tree_snapshot);

    bool IsAlive() const { return binding_.has_value(); }

    std::optional<zx_koid_t> context_view() const { return context_view_; }

   private:
    // Checks whether the required conditions for sending the response to the client are met and
    // then sends the response.
    void SendResponseMaybe();

    // Trigger the |pending_completer_| to send the response to the client. If the size of the
    // response exceeds ZX_CHANNEL_MAX_MSG_BYTES, older
    // `fuchsia_ui_observation_geometry::ViewTreeSnapshot`s in the response are dropped.
    void SendResponse();

    // Closes the fidl channel.
    void CloseChannel();

    // Resets the state of an |endpoint_| for subsequent |Watch| calls.
    void Reset();

    // Server-side endpoint binding.
    std::optional<fidl::ServerBinding<fuchsia_ui_observation_geometry::ViewTreeWatcher>> binding_;

    // A deque containing pending snapshot updates for a client. The size of the deque cannot exceed
    // |fuchsia_ui_observation_geometry::kBufferSize|.
    std::deque<fuchsia_ui_observation_geometry::ViewTreeSnapshot> view_tree_snapshots_;

    // If the last |Watch| call did not immediately trigger a callback, it gets stored here and is
    // triggered whenever a new snapshot gets generated.
    std::optional<WatchCompleter::Async> pending_completer_;

    std::optional<const zx_koid_t> context_view_;

    // Errors faced while executing the |pending_completer_|. |error_| must be reset after
    // |pending_completer_| is executed for subsequent |Watch| calls.
    fuchsia_ui_observation_geometry::Error error_{};
  };

  // Common impl for `Register()` and `RegisterGlobalViewTreeWatcher()`.
  void RegisterViewTreeWatcherImpl(
      fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint,
      std::optional<zx_koid_t> context_view);

  // Generates a fuchsia_ui_observation_geometry::ViewDescriptor from the |snapshot|'s view node by
  // extracting information about the |view_ref_koid| from the view node.
  // The view nodes corresponding to views with 0x0 size are *not* reported.
  static fuchsia_ui_observation_geometry::ViewDescriptor ExtractViewDescriptor(
      zx_koid_t view_ref_koid, zx_koid_t context_view, const view_tree::Snapshot& snapshot);

  std::unordered_map<ProviderEndpointId, std::unique_ptr<ProviderEndpoint>> endpoints_;

  // Incremented when Register() is called.
  ProviderEndpointId endpoint_counter_ = 0;

  std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;

  uint64_t latest_sequence_number_ = 0;

  fxl::WeakPtrFactory<GeometryProvider> weak_factory_{this};

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(GeometryProvider);
};

}  // namespace view_tree

#endif  // SRC_UI_SCENIC_LIB_VIEW_TREE_GEOMETRY_PROVIDER_H_
