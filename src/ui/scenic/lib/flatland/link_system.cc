// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/link_system.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <array>
#include <cstddef>
#include <memory_resource>

#include "src/ui/scenic/lib/utils/dispatcher_holder.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/task_utils.h"

#include <glm/gtc/matrix_access.hpp>

using fuchsia_ui_composition::ChildViewStatus;
using fuchsia_ui_composition::ChildViewWatcher;
using fuchsia_ui_composition::LayoutInfo;
using fuchsia_ui_composition::ParentViewportStatus;
using fuchsia_ui_composition::ParentViewportWatcher;
using fuchsia_ui_views::ViewCreationToken;
using fuchsia_ui_views::ViewportCreationToken;

namespace flatland {

LinkSystem::LinkSystem(TransformHandle::InstanceId instance_id)
    : instance_id_(instance_id), link_graph_(instance_id_), linker_(ObjectLinker::New()) {
  device_pixel_ratio_ = fuchsia_math::VecF{{.x = 1.f, .y = 1.f}};
}

LinkSystem::LinkToChild LinkSystem::CreateLinkToChild(
    std::shared_ptr<utils::DispatcherHolder> dispatcher_holder, ViewportCreationToken token,
    fuchsia_ui_composition::ViewportProperties initial_properties,
    fidl::ServerEnd<ChildViewWatcher> child_view_watcher, TransformHandle parent_transform_handle,
    LinkProtocolErrorCallback error_callback) {
  FX_DCHECK(token.value().is_valid());
  FX_DCHECK(initial_properties.logical_size().has_value());
  FX_DCHECK(initial_properties.inset().has_value());

  {  // Put the initial layout in a map. This lets us update the layout before the link resolves if
     // necessary.
    std::scoped_lock lock(mutex_);
    LayoutInfo info;
    info.logical_size(initial_properties.logical_size().value());
    info.inset(initial_properties.inset().value());
    info.device_pixel_ratio(device_pixel_ratio_);
    initial_layout_infos_.emplace(parent_transform_handle, std::move(info));
  }

  auto impl = std::make_shared<ChildViewWatcherImpl>(dispatcher_holder,
                                                     std::move(child_view_watcher), error_callback);
  const TransformHandle internal_link_handle = CreateTransformLocked();

  ObjectLinker::ImportLink importer =
      linker_->CreateImport(LinkToChildInfo{.parent_transform_handle = parent_transform_handle,
                                            .internal_link_handle = internal_link_handle},
                            std::move(token.value()),
                            /* error_reporter */ nullptr);

  auto child_transform_handle = std::make_shared<TransformHandle>();  // Uninitialized.
  importer.Initialize(
      /* link_resolved = */
      [ref = shared_from_this(), impl, child_transform_handle](LinkToParentInfo info) mutable {
        if (info.view_ref != nullptr) {
          impl->SetViewRef(fuchsia_ui_views::ViewRef{
              {.reference = utils::CopyZxHandle(info.view_ref->eventpair())}});
        }

        *child_transform_handle = info.child_transform_handle;
        {
          std::scoped_lock lock(ref->mutex_);
          ref->child_to_parent_map_[*child_transform_handle] =
              ParentEnd{.child_view_watcher = impl};
        }
      },
      /* link_invalidated = */
      [ref = shared_from_this(), impl, child_transform_handle, parent_transform_handle,
       weak_dispatcher_holder = std::weak_ptr<utils::DispatcherHolder>(dispatcher_holder)](
          bool on_link_destruction) mutable {
        // We expect |child_transform_handle| to be assigned by the "link_resolved" closure,
        // but this might not happen if the link is being destroyed before it was resolved.
        FX_DCHECK(child_transform_handle || on_link_destruction);

        {
          std::scoped_lock lock(ref->mutex_);
          ref->child_to_parent_map_.erase(*child_transform_handle);
          ref->initial_layout_infos_.erase(parent_transform_handle);
        }

        // Avoid race conditions by destroying ChildViewWatcher on its "own" thread.  For example,
        // if not destroyed on its "own" thread, it might concurrently be handling a FIDL message.
        if (auto dispatcher_holder = weak_dispatcher_holder.lock()) {
          utils::ExecuteOrPostTaskOnDispatcher(
              dispatcher_holder->dispatcher(), [impl = std::move(impl)]() mutable {
                TRACE_DURATION("gfx", "LinkSystem::CreateLinkToChild[destroy impl]");
                impl.reset();
              });

          // The point of moving |impl| into the task above is to destroy it on the correct thread.
          // Verify that we did actually move it (previously, there was a subtle bug where this
          // closure wasn't declared as mutable, so we copied the shared_ptr instead of moving it).
          FX_DCHECK(!impl);
        }
      });

  return LinkToChild({
      .parent_transform_handle = parent_transform_handle,
      .internal_link_handle = internal_link_handle,
      .importer = std::move(importer),
  });
}

LinkSystem::LinkToParent LinkSystem::CreateLinkToParent(
    std::shared_ptr<utils::DispatcherHolder> dispatcher_holder, ViewCreationToken token,
    std::optional<fuchsia_ui_views::ViewIdentityOnCreation> view_identity,
    fidl::ServerEnd<ParentViewportWatcher> parent_viewport_watcher,
    TransformHandle child_transform_handle, LinkProtocolErrorCallback error_callback) {
  FX_DCHECK(token.value().is_valid());

  std::shared_ptr<const types::ViewRef> view_ref;
  std::optional<fuchsia_ui_views::ViewRefControl> view_ref_control;
  if (view_identity.has_value()) {
    view_ref = std::make_shared<const types::ViewRef>(std::move(view_identity->view_ref()));
    view_ref_control = std::move(view_identity->view_ref_control());
  }

  auto impl = std::make_shared<ParentViewportWatcherImpl>(
      dispatcher_holder, std::move(parent_viewport_watcher), error_callback);

  ObjectLinker::ExportLink exporter = linker_->CreateExport(
      LinkToParentInfo{.child_transform_handle = child_transform_handle, .view_ref = view_ref},
      std::move(token.value()),
      /* error_reporter */ nullptr);

  auto parent_transform_handle = std::make_shared<TransformHandle>();  // Uninitialized.
  auto topology_map_key = std::make_shared<TransformHandle>();         // Uninitialized.
  exporter.Initialize(
      /* link_resolved = */
      [ref = shared_from_this(), impl, parent_transform_handle, topology_map_key,
       child_transform_handle](LinkToChildInfo info) {
        *parent_transform_handle = info.parent_transform_handle;
        *topology_map_key = info.internal_link_handle;

        {
          std::scoped_lock lock(ref->mutex_);
          const auto layout_kv = ref->initial_layout_infos_.find(*parent_transform_handle);
          FX_DCHECK(layout_kv != ref->initial_layout_infos_.end())
              << "initial_layout_infos_ for |parent_transform_handle| should exist until the link resolves or is destroyed.";
          impl->UpdateLayoutInfo(std::move(layout_kv->second));
          ref->initial_layout_infos_.erase(layout_kv);

          ref->parent_to_child_map_[*parent_transform_handle] = ChildEnd{
              .parent_viewport_watcher = impl, .child_transform_handle = child_transform_handle};
          // The topology is constructed here, instead of in the link_resolved closure of the
          // LinkToParent object, so that its destruction (which depends on the
          // internal_link_handle) can occur on the same endpoint.
          ref->link_topologies_[*topology_map_key] = child_transform_handle;
          ref->link_topology_changed_ = true;
        }
      },
      /* link_invalidated = */
      [ref = shared_from_this(), impl, parent_transform_handle, topology_map_key,
       weak_dispatcher_holder = std::weak_ptr<utils::DispatcherHolder>(dispatcher_holder)](
          bool on_link_destruction) mutable {
        // We expect |parent_transform_handle| and |topology_map_key| to be assigned by
        // the "link_resolved" closure, but this might not happen if the link is being destroyed
        // before it was resolved.
        FX_DCHECK((parent_transform_handle && topology_map_key) || on_link_destruction);

        {
          std::scoped_lock map_lock(ref->mutex_);
          ref->parent_to_child_map_.erase(*parent_transform_handle);

          ref->link_topologies_.erase(*topology_map_key);
          ref->link_graph_.ReleaseTransform(*topology_map_key);
          ref->link_topology_changed_ = true;
        }

        // Avoid race conditions by destroying ParentViewportWatcher on its "own" thread.  For
        // example, if not destroyed on its "own" thread, it might concurrently be handling a FIDL
        // message.
        if (auto dispatcher_holder = weak_dispatcher_holder.lock()) {
          utils::ExecuteOrPostTaskOnDispatcher(
              dispatcher_holder->dispatcher(), [impl = std::move(impl)]() mutable {
                TRACE_DURATION("gfx", "LinkSystem::CreateLinkToParent[destroy impl]");
                impl.reset();
              });

          // The point of moving |impl| into the task above is to destroy it on the correct thread.
          // Verify that we did actually move it (previously, there was a subtle bug where this
          // closure wasn't declared as mutable, so we copied the shared_ptr instead of moving it).
          FX_DCHECK(!impl);
        }
      });

  return LinkToParent({.child_transform_handle = child_transform_handle,
                       .exporter = std::move(exporter),
                       .view_ref = std::move(view_ref),
                       .view_ref_control = std::move(view_ref_control)});
}

void LinkSystem::UpdateLinkWatchers(const GlobalTopologyData::TopologyVector& global_topology,
                                    const GlobalMatrixVector& global_matrices,
                                    const UberStruct::InstanceMap& uber_structs) const {
  TRACE_DURATION("gfx", "LinkSystem::UpdateLinkWatchers");
  std::scoped_lock lock(mutex_);

  // Use a stack-allocated buffer to back the polymorphic memory resource.
  // 16KB is extremely safe for a tree of 200+ nodes.
  alignas(std::max_align_t) std::array<std::byte, 16384> stack_buffer;
  std::pmr::monotonic_buffer_resource mem_resource(stack_buffer.data(), stack_buffer.size());

  std::pmr::unordered_set<TransformHandle> live_handles(
      global_topology.begin(), global_topology.end(),
      /*bucket_count=*/global_topology.size(), &mem_resource);

  // Since the global topology may not contain every Flatland instance, manually update the
  // ParentViewportStatus of every ParentViewportWatcher.
  for (auto& [_, child_end] : parent_to_child_map_) {
    // The child Flatland instance is connected to the display if it is present in the global
    // topology.
    child_end.parent_viewport_watcher->UpdateLinkStatus(
        live_handles.contains(child_end.child_transform_handle)
            ? ParentViewportStatus::kConnectedToDisplay
            : ParentViewportStatus::kDisconnectedFromDisplay);
  }

  // ChildViewWatcher has two hanging get methods, GetStatus() and GetViewRef(), whose responses are
  // generated in the loop below.
  for (auto& [child_transform_handle, parent_end] : child_to_parent_map_) {
    auto& child_view_watcher = parent_end.child_view_watcher;
    // The ChildViewStatus changes the first time the child presents with a particular parent link.
    // This is indicated by an UberStruct with the |child_transform_handle| as its first
    // TransformHandle in the snapshot.
    //
    // NOTE: This does not mean the child content actually appears on-screen; it simply informs
    //       the parent that the child has content that is available to present on screen.  This is
    //       intentional; for example, the parent might not want to attach the child to the global
    //       scene graph until it knows the child is ready to present content on screen.
    //
    // NOTE: The LinkSystem can technically "miss" updating the ChildViewStatus for a
    //       particular ChildViewWatcher if the child presents two CreateView() calls before
    //       UpdateLinkWatchers() is called, but in that case, the first Link is destroyed,
    //       and therefore its status does not need to be updated anyway.
    auto uber_struct_kv = uber_structs.find(child_transform_handle.GetInstanceId());
    if (uber_struct_kv != uber_structs.end()) {
      const auto& local_topology = uber_struct_kv->second->local_topology;

      // If the local topology doesn't start with the |child_transform_handle|, the child is linked
      // to a different parent now, but the link_invalidated callback to remove this entry has not
      // fired yet.
      if (!local_topology.empty() && local_topology.front().handle == child_transform_handle) {
        child_view_watcher->UpdateLinkStatus(ChildViewStatus::kContentHasPresented);
      }
    }

    // As soon as the child view is part of the global topology, update the watcher to send it along
    // to any caller of GetViewRef().  For example, this means that by the time the watcher receives
    // it, the child view will already exist in the view tree, and therefore an attempt to focus it
    // will succeed.
    if (live_handles.contains(child_transform_handle)) {
      child_view_watcher->UpdateViewRef();
    }
  }
}

void LinkSystem::UpdateDevicePixelRatio(const fuchsia_math::VecF& device_pixel_ratio) {
  TRACE_DURATION("gfx", "LinkSystem::UpdateDevicePixelRatio");
  std::scoped_lock lock(mutex_);

  if (device_pixel_ratio_ == device_pixel_ratio) {
    // The new value is the same as the old.
    return;
  }
  device_pixel_ratio_ = device_pixel_ratio;

  // We update DPR info for every single View in the scene graph.
  // TODO(https://fxbug.dev/42059985): This assumes the same DPR for every client. Need to fix it
  // for multi-display.
  for (const auto& [handle, child_end] : parent_to_child_map_) {
    child_end.parent_viewport_watcher->UpdateDevicePixelRatio(device_pixel_ratio);
  }
  for (auto& [_, layout] : initial_layout_infos_) {
    layout.device_pixel_ratio(device_pixel_ratio);
  }
}

void LinkSystem::UpdateViewportPropertiesFor(
    const TransformHandle handle, const fuchsia_ui_composition::ViewportProperties& properties) {
  // May be called from main thread or from a Flatland thread.
  FX_DCHECK(properties.logical_size().has_value());
  FX_DCHECK(properties.inset().has_value());

  std::scoped_lock lock(mutex_);

  LayoutInfo info;
  info.logical_size(properties.logical_size().value());
  info.inset(properties.inset().value());
  info.device_pixel_ratio(device_pixel_ratio_);

  // |handle| should always be a valid parent TransformHandle. But since the caller may be a
  // Flatland instance thread, which may not know about the destruction of a link, we can be in
  // one of three states:
  // 1. Unresolved link -> Layout stored in |initial_layout_infos_|.
  // 2. Resolved link -> Layout stored in |parent_to_child_map_|.
  // 3. Dead link -> Layout stored nowhere.
  FX_DCHECK((initial_layout_infos_.count(handle) + parent_to_child_map_.count(handle)) <= 1)
      << "Layout should only exist in at most one map at a time.";
  if (auto initial_layout_it = initial_layout_infos_.find(handle);
      initial_layout_it != initial_layout_infos_.end()) {
    // The link hasn't resolved yet; just update the initial layout that will be sent to the child
    // once it resolves.
    initial_layout_it->second = std::move(info);
  } else if (const auto child_it = parent_to_child_map_.find(handle);
             child_it != parent_to_child_map_.end()) {
    child_it->second.parent_viewport_watcher->UpdateLayoutInfo(std::move(info));
  }
}

GlobalTopologyData::LinkTopologyMap LinkSystem::GetResolvedTopologyLinks() {
  TRACE_DURATION("gfx", "LinkSystem::GetResolvedTopologyLinks");
  GlobalTopologyData::LinkTopologyMap copy;

  // Acquire the lock and copy.
  {
    std::scoped_lock lock(mutex_);
    copy = link_topologies_;
  }
  return copy;
}

TransformHandle::InstanceId LinkSystem::GetInstanceId() const { return instance_id_; }

std::pair<std::unordered_map<TransformHandle, TransformHandle>, bool> const
LinkSystem::GetLinkChildToParentTransformMap() {
  TRACE_DURATION("gfx", "LinkSystem::GetLinkChildToParentTransformMap");
  std::pair<std::unordered_map<TransformHandle, TransformHandle>, bool> result;
  auto& child_to_parent_map = result.first;
  auto& link_topology_changed = result.second;

  std::scoped_lock lock(mutex_);
  for (const auto& [parent_transform_handle, child_end] : parent_to_child_map_) {
    child_to_parent_map.try_emplace(child_end.child_transform_handle, parent_transform_handle);
  }

  link_topology_changed = link_topology_changed_;
  link_topology_changed_ = false;

  return result;
}

}  // namespace flatland
