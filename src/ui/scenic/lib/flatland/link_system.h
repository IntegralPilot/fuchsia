// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_LINK_SYSTEM_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_LINK_SYSTEM_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

#include <functional>
#include <unordered_map>
#include <unordered_set>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_matrix_data.h"
#include "src/ui/scenic/lib/flatland/global_topology_data.h"
#include "src/ui/scenic/lib/flatland/hanging_get_helper.h"
#include "src/ui/scenic/lib/flatland/transform_graph.h"
#include "src/ui/scenic/lib/flatland/transform_handle.h"
#include "src/ui/scenic/lib/flatland/uber_struct.h"
#include "src/ui/scenic/lib/utils/dispatcher_holder.h"
#include "src/ui/scenic/lib/utils/object_linker.h"

#include <glm/glm.hpp>
#include <glm/mat3x3.hpp>

namespace flatland {

// Used by to communicate back to `LinkSystem` callers that a `ParentViewportWatcher` or
// `ChildViewWatcher` client performed an illegal action.  For example, this is used by Flatland
// to close down the associated Flatland session with an error.
using LinkProtocolErrorCallback = std::function<void(const std::string&)>;

// An implementation of the ParentViewportWatcher protocol, consisting of hanging gets for various
// updateable pieces of information.
class ParentViewportWatcherImpl
    : public fidl::Server<fuchsia_ui_composition::ParentViewportWatcher> {
 public:
  ParentViewportWatcherImpl(
      std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
      fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> server_end,
      LinkProtocolErrorCallback error_callback)
      :
#ifndef NDEBUG
        dispatcher_holder_(dispatcher_holder),
#endif
        error_callback_(std::move(error_callback)),
        binding_(dispatcher_holder->dispatcher(), std::move(server_end), this,
                 &ParentViewportWatcherImpl::OnClose) {
  }

  ParentViewportWatcherImpl(ParentViewportWatcherImpl&&) = delete;
  ParentViewportWatcherImpl(const ParentViewportWatcherImpl&) = delete;
  ParentViewportWatcherImpl& operator=(const ParentViewportWatcherImpl&) = delete;

  ~ParentViewportWatcherImpl() override {
#ifndef NDEBUG
    auto dispatcher_holder = dispatcher_holder_.lock();
    FX_CHECK(!dispatcher_holder ||
             (dispatcher_holder->dispatcher() == async_get_default_dispatcher()));
#endif  // NDEBUG
  }

  void UpdateLayoutInfo(fuchsia_ui_composition::LayoutInfo info) {
    last_layout_info_ = info;
    layout_helper_.Update(std::move(info));
  }

  void UpdateDevicePixelRatio(const fuchsia_math::VecF& device_pixel_ratio) {
    auto info = last_layout_info_;
    info.device_pixel_ratio(device_pixel_ratio);
    UpdateLayoutInfo(std::move(info));
  }

  void UpdateLinkStatus(fuchsia_ui_composition::ParentViewportStatus status) {
    status_helper_.Update(status);
  }

  // |fuchsia_ui_composition::ParentViewportWatcher|
  void GetLayout(GetLayoutCompleter::Sync& sync_completer) override {
    if (layout_helper_.HasPendingCallback()) {
      FX_CHECK(error_callback_);
      error_callback_(
          "GetLayout() called when there is a pending GetLayout() call. Flatland connection "
          "will be closed because of broken flow control.");
      sync_completer.Close(ZX_ERR_SHOULD_WAIT);
      return;
    }

    layout_helper_.SetCallback([completer = sync_completer.ToAsync()](
                                   fuchsia_ui_composition::LayoutInfo layout_info) mutable {
      completer.Reply({std::move(layout_info)});
    });
  }

  // |fuchsia_ui_composition::ParentViewportWatcher|
  void GetStatus(GetStatusCompleter::Sync& sync_completer) override {
    if (status_helper_.HasPendingCallback()) {
      FX_CHECK(error_callback_);
      error_callback_(
          "GetStatus() called when there is a pending GetStatus() call. Flatland connection "
          "will be closed because of broken flow control.");
      sync_completer.Close(ZX_ERR_SHOULD_WAIT);
      return;
    }

    status_helper_.SetCallback([completer = sync_completer.ToAsync()](
                                   fuchsia_ui_composition::ParentViewportStatus status) mutable {
      completer.Reply({status});
    });
  }

 private:
  // Called when the connection is torn down.
  static void OnClose(fidl::UnbindInfo info) {
    if (info.is_peer_closed()) {
      FX_LOGS(DEBUG) << "ParentViewportWatcherImpl::OnUnbound()  Client disconnected";
    } else if (!info.is_user_initiated()) {
      FX_LOGS(WARNING) << "ParentViewportWatcherImpl::OnUnbound()  server error: " << info;
    }
  }

#ifndef NDEBUG
  // Only used to verify that destruction occurs on the correct thread.
  std::weak_ptr<utils::DispatcherHolder> dispatcher_holder_;
#endif

  fuchsia_ui_composition::LayoutInfo last_layout_info_;

  LinkProtocolErrorCallback error_callback_;
  HangingGetHelper<fuchsia_ui_composition::LayoutInfo> layout_helper_;
  HangingGetHelper<fuchsia_ui_composition::ParentViewportStatus> status_helper_;

  // Safety: destroy binding first, then destroy hanging-get helpers.
  // This confirms that we are shutting down instead of simply forgot to reply
  // to the completers captured in the hanging-get helpers.
  fidl::ServerBinding<fuchsia_ui_composition::ParentViewportWatcher> binding_;
};

// An implementation of the ChildViewWatcher protocol, consisting of hanging gets for various
// updateable pieces of information.
class ChildViewWatcherImpl : public fidl::Server<fuchsia_ui_composition::ChildViewWatcher> {
 public:
  ChildViewWatcherImpl(std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
                       fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> server_end,
                       LinkProtocolErrorCallback error_callback)
      :
#ifndef NDEBUG
        dispatcher_holder_(dispatcher_holder),
#endif
        error_callback_(std::move(error_callback)),
        binding_(dispatcher_holder->dispatcher(), std::move(server_end), this,
                 &ChildViewWatcherImpl::OnClose) {
  }

  ChildViewWatcherImpl(ChildViewWatcherImpl&&) = delete;
  ChildViewWatcherImpl(const ChildViewWatcherImpl&) = delete;
  ChildViewWatcherImpl& operator=(const ChildViewWatcherImpl&) = delete;

  ~ChildViewWatcherImpl() override {
#ifndef NDEBUG
    auto dispatcher_holder = dispatcher_holder_.lock();
    FX_CHECK(!dispatcher_holder ||
             (dispatcher_holder->dispatcher() == async_get_default_dispatcher()));
#endif  // NDEBUG
  }

  void UpdateLinkStatus(fuchsia_ui_composition::ChildViewStatus status) {
    status_helper_.Update(status);
    if (viewref_) {
      // At the time of writing, CONTENT_HAS_PRESENTED is the only possible value.  DCHECK just in
      // case this changes.
      FX_CHECK(status == fuchsia_ui_composition::ChildViewStatus::kContentHasPresented);
    }
  }

  // If ViewRef hasn't yet been pushed to the hanging get helper, do so.
  void UpdateViewRef() {
    if (viewref_) {
      viewref_helper_.Update(std::move(viewref_.value()));
      viewref_.reset();
    }
  }

  void SetViewRef(fuchsia_ui_views::ViewRef viewref) {
    FX_CHECK(viewref.reference().is_valid());
    viewref_ = std::move(viewref);
  }

  // |fuchsia_ui_composition::ChildViewWatcher|
  void GetStatus(GetStatusCompleter::Sync& sync_completer) override {
    if (status_helper_.HasPendingCallback()) {
      FX_CHECK(error_callback_);
      error_callback_(
          "GetStatus() called when there is a pending GetStatus() call. Flatland connection "
          "will be closed because of broken flow control.");
      sync_completer.Close(ZX_ERR_SHOULD_WAIT);
      return;
    }

    status_helper_.SetCallback(
        [completer = sync_completer.ToAsync()](
            fuchsia_ui_composition::ChildViewStatus status) mutable { completer.Reply({status}); });
  }

  // |fuchsia_ui_composition::ChildViewWatcher|
  void GetViewRef(GetViewRefCompleter::Sync& sync_completer) override {
    if (viewref_helper_.HasPendingCallback()) {
      FX_CHECK(error_callback_);
      error_callback_(
          "GetViewRef() called when there is a pending GetViewRef() call. Flatland connection "
          "will be closed because of broken flow control.");
      sync_completer.Close(ZX_ERR_SHOULD_WAIT);
      return;
    }

    viewref_helper_.SetCallback(
        [completer = sync_completer.ToAsync()](fuchsia_ui_views::ViewRef viewref) mutable {
          completer.Reply({std::move(viewref)});
        });
  }

 private:
  // Called when the connection is torn down.
  static void OnClose(fidl::UnbindInfo info) {
    if (info.is_peer_closed()) {
      FX_LOGS(DEBUG) << "ChildViewWatcherImpl::OnUnbound()  Client disconnected";
    } else if (!info.is_user_initiated()) {
      FX_LOGS(WARNING) << "ChildViewWatcherImpl::OnUnbound()  server error: " << info;
    }
  }

#ifndef NDEBUG
  // Only used to verify that destruction occurs on the correct thread.
  std::weak_ptr<utils::DispatcherHolder> dispatcher_holder_;
#endif
  LinkProtocolErrorCallback error_callback_;
  HangingGetHelper<fuchsia_ui_composition::ChildViewStatus> status_helper_;
  HangingGetHelper<fuchsia_ui_views::ViewRef> viewref_helper_;

  // Safety: destroy binding first, then destroy hanging-get helpers.
  // This confirms that we are shutting down instead of simply forgot to reply
  // to the completers captured in the hanging-get helpers.
  fidl::ServerBinding<fuchsia_ui_composition::ChildViewWatcher> binding_;

  // Temporarily held when SetViewRef() is called.  Instead of immediately notifying any pending
  // hanging get requests, we wait until the child view first appears in the global topology.
  std::optional<fuchsia_ui_views::ViewRef> viewref_;
};

// A system for managing links between Flatland instances. Each Flatland instance creates Links
// using tokens provided by Flatland clients. Each end of a Link consists of:
// - An implementation of the FIDL protocol for communicating with the other end of the link.
// - A TransformHandle which serves as the "attachment point" for that end of the link.
// - The ObjectLinker link which serves as the actual implementation of the link.
//
// The LinkSystem is only responsible for connecting the "attachment point" TransformHandles
// returned in the Link structs. Flatland instances must attach these handles to their own
// transform hierarchy and notify the TopologySystem in order for the link to actually be
// established.
class LinkSystem : public std::enable_shared_from_this<LinkSystem> {
 public:
  explicit LinkSystem(TransformHandle::InstanceId instance_id);

  // Because this object captures its "this" pointer in internal closures, it is unsafe to copy or
  // move it. Disable all copy and move operations.
  LinkSystem(const LinkSystem&) = delete;
  LinkSystem& operator=(const LinkSystem&) = delete;
  LinkSystem(LinkSystem&&) = delete;
  LinkSystem& operator=(LinkSystem&&) = delete;

  // In addition to supplying an interface request via the ObjectLinker, the "ToChild" end of a link
  // also supplies its attachment point so that the LinkSystem can create an edge between the two
  // when the link resolves. This allows creation and destruction logic to be paired within a single
  // ObjectLinker endpoint, instead of being spread out between the two endpoints.
  struct LinkToChildInfo {
    TransformHandle parent_transform_handle;
    TransformHandle internal_link_handle;
  };

  struct LinkToParentInfo {
    TransformHandle child_transform_handle;
    std::shared_ptr<const types::ViewRef> view_ref;
  };

  // Linked Flatland instances only implement a small piece of link functionality. For now, directly
  // sharing link requests is a clean way to implement that functionality. This will become more
  // complicated as the Flatland API evolves.
  using ObjectLinker = utils::ObjectLinker<LinkToParentInfo, LinkToChildInfo>;

  // Destruction of a LinkToChild object will trigger deregistration with the LinkSystem.
  // Deregistration is thread safe, but the user of the Link object should be confident (e.g., by
  // tracking release fences) that no other systems will try to reference the Link.
  struct LinkToChild {
    // The handle on which the ParentViewportWatcherImpl will live.
    TransformHandle parent_transform_handle;
    // The LinkSystem-owned handle that will be a key in the LinkTopologyMap when the link resolves.
    // These handles will never be in calculated global topologies; they are primarily used to
    // signal when to look for a link in GlobalTopologyData::ComputeGlobalTopologyData().
    TransformHandle internal_link_handle;
    ObjectLinker::ImportLink importer;
  };

  // Destruction of a LinkToParent object will trigger deregistration with the LinkSystem.
  // Deregistration is thread safe, but the user of the Link object should be confident (e.g., by
  // tracking release fences) that no other systems will try to reference the Link.
  struct LinkToParent {
    // The handle that the ChildViewWatcherImpl will live on and will be a value in the
    // LinkTopologyMap when the link resolves.
    TransformHandle child_transform_handle;
    ObjectLinker::ExportLink exporter;

    // Tracks the ViewRef for this View and is the reference for the lifetime of the ViewRef by
    // uniquely holding |view_ref_control| until going out of scope.
    std::shared_ptr<const types::ViewRef> view_ref;

    // |view_ref_control| and |view_ref| are set when there is a valid ViewIdentityOnCreation.
    // Otherwise both are kept empty.
    std::optional<fuchsia_ui_views::ViewRefControl> view_ref_control;
  };

  // Creates the parent end of a link. The LinkToChild's |internal_link_handle| serves as the
  // attachment point for the caller's transform hierarchy. |initial_properties| is immediately
  // dispatched to the LinkToParent when the Link is resolved, regardless of whether the parent or
  // the child has called |Flatland::Present()|.
  //
  // Link handles are excluded from global topologies, so the |parent_transform_handle| is
  // provided by the parent as the attachment point for the ChildViewWatcherImpl.
  //
  // |dispatcher_holder| allows hanging-get response-callbacks to be invoked from the appropriate
  // Flatland session thread.
  LinkToChild CreateLinkToChild(
      std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
      fuchsia_ui_views::ViewportCreationToken token,
      fuchsia_ui_composition::ViewportProperties initial_properties,
      fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher,
      TransformHandle parent_transform_handle, LinkProtocolErrorCallback error_callback);

  // Creates the child end of a link. Once both ends of a Link have been created, the LinkSystem
  // will create a local topology that connects the internal Link to the LinkToParent's
  // |child_transform_handle|.
  //
  // |dispatcher_holder| allows hanging-get response-callbacks to be invoked from the appropriate
  // Flatland session thread.
  LinkToParent CreateLinkToParent(
      std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
      fuchsia_ui_views::ViewCreationToken token,
      std::optional<fuchsia_ui_views::ViewIdentityOnCreation> view_identity,
      fidl::ServerEnd<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher,
      TransformHandle child_transform_handle, LinkProtocolErrorCallback error_callback);

  // Returns a snapshot of the current set of links, represented as a map from LinkSystem-owned
  // TransformHandles to TransformHandles in LinkToParents. The LinkSystem generates Keys for this
  // map in CreateLinkToChild() and returns them to callers in a LinkToChild's
  // |internal_link_handle|. The values in this map are arguments to CreateLinkToParent() and become
  // the LinkToParent's |child_transform_handle|. The LinkSystem places entries in the map when a
  // link resolves and removes them when a link is invalidated.
  GlobalTopologyData::LinkTopologyMap GetResolvedTopologyLinks();

  // Returns the instance ID used for LinkSystem-authored handles.
  TransformHandle::InstanceId GetInstanceId() const;

  // For use by the core processing loop, this function consumes global information, processes it,
  // and sends all necessary updates to active ParentViewportWatcher and ChildViewWatcher channels.
  //
  // This data passed into this function is generated by merging information from multiple Flatland
  // instances. |global_topology| is the TopologyVector of all nodes visible from the (currently
  // single) display. |live_handles| is the set of nodes in that vector. |global_matrices| is the
  // list of global matrices, one per handle in |global_topology|. |uber_structs| is the set of
  // UberStructs used to generate the global topology.
  void UpdateLinkWatchers(const GlobalTopologyData::TopologyVector& global_topology,
                          const GlobalMatrixVector& global_matrices,
                          const UberStruct::InstanceMap& uber_structs) const;

  // Returns the mapping from the child_transform_handle of each LinkToParent to the corresponding
  // parent_transform_handle from each LinkToChild.
  std::pair<std::unordered_map<TransformHandle, TransformHandle>, bool> const
  GetLinkChildToParentTransformMap();

  // Updates |device_pixel_ratio_| for the View with parent |handle|. If the value changed it sends
  // updates to all waiting clients, otherwise it does nothing.
  // |handle| should have been previously used as |parent_transform_handle| in CreateLinkToChild().
  void UpdateViewportPropertiesFor(TransformHandle handle,
                                   const fuchsia_ui_composition::ViewportProperties& properties);

  // Updates |device_pixel_ratio_| and, if the value changed, sends updates to all waiting clients.
  void UpdateDevicePixelRatio(const fuchsia_math::VecF& device_pixel_ratio);
  void UpdateDevicePixelRatio(const glm::vec2& initial_device_pixel_ratio) {
    UpdateDevicePixelRatio(
        fuchsia_math::VecF{{.x = initial_device_pixel_ratio.x, .y = initial_device_pixel_ratio.y}});
  }

 private:
  TransformHandle CreateTransformLocked() {
    TransformHandle transform;
    {
      std::scoped_lock lock(mutex_);
      transform = link_graph_.CreateTransform();
    }
    return transform;
  }

  TransformHandle::InstanceId instance_id_;

  // |link_graph_|, an instance of a TransformGraph, is not thread safe, as it is designed to be
  // used by individual Flatland instances. However, this class is shared across all Flatland
  // instances, and therefore different threads. Therefore, access to |link_graph_| should be
  // guarded by |mutex_|.
  TransformGraph link_graph_ FXL_GUARDED_BY(mutex_);

  std::shared_ptr<ObjectLinker> linker_;

  // |mutex_| guards access to |link_graph_| and |link_topologies_|.
  //
  // TODO(https://fxbug.dev/42120738): These maps are modified at Link creation and destruction time
  // (within the ObjectLinker closures) as well as within UpdateLinkWatchers(), which is called by
  // the core render loop. This produces a possible priority inversion between the Flatland instance
  // threads and the (possibly deadline scheduled) render thread.
  mutable std::mutex mutex_;

  // Structs representing the child and parent ends of a link.
  struct ChildEnd {
    std::shared_ptr<ParentViewportWatcherImpl> parent_viewport_watcher;
    TransformHandle child_transform_handle;
  };
  struct ParentEnd {
    std::shared_ptr<ChildViewWatcherImpl> child_view_watcher;
  };

  // Keyed by LinkToChild::parent_transform_handle. Access is managed by |mutex_|.
  std::unordered_map<TransformHandle, ChildEnd> parent_to_child_map_ FXL_GUARDED_BY(mutex_);
  // Keyed by LinkToParent::child_transform_handle. Access is managed by |mutex_|.
  std::unordered_map<TransformHandle, ParentEnd> child_to_parent_map_ FXL_GUARDED_BY(mutex_);
  // The set of current link topologies. Access is managed by |mutex_|.
  GlobalTopologyData::LinkTopologyMap link_topologies_ FXL_GUARDED_BY(mutex_);

  // A map of the most recent LayoutInfo generated for each link that hasn't resolved yet.
  std::unordered_map<TransformHandle, fuchsia_ui_composition::LayoutInfo> initial_layout_infos_
      FXL_GUARDED_BY(mutex_);

  // The starting DPR used by the link system. The actual DPR used on subsequent calls to
  // UpdateLinkWatchers() may be different from this value.
  // TODO(https://fxbug.dev/42059985): This will need to be updated once we have multidisplay setup.
  fuchsia_math::VecF device_pixel_ratio_ FXL_GUARDED_BY(mutex_);

  // Tracks whether a link between sessions has been established/broken since the last time that
  // `GetLinkChildToParentTransformMap()` was called.  This is used as a signal that indicates that
  // the ViewTree needs to be recomputed.  Starting as true guarantees that the ViewTree is always
  // generated the first time (necessary because it is illegal for a subtree generator to say
  // "no diff" the first time).
  bool link_topology_changed_ = true;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_LINK_SYSTEM_H_
