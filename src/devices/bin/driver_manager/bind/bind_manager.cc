// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/bind/bind_manager.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>

#include "src/devices/bin/driver_manager/node_property_conversion.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/lib/log/log.h"

namespace fdd = fuchsia_driver_development;
namespace fdi = fuchsia_driver_index;

namespace driver_manager {

BindManager::BindManager(BindManagerBridge* bridge, NodeManager* node_manager,
                         async_dispatcher_t* dispatcher)
    : bridge_(bridge) {
  bind_resource_set_.set_on_bind_state_changed([bridge]() { bridge->OnBindingStateChanged(); });
}

void BindManager::TryBindAllAvailable(NodeBindingInfoResultCallback result_callback) {
  // If there's an ongoing process to bind all orphans, queue up this callback. Once
  // the process is complete, it'll make another attempt to bind all orphans and invoke
  // all callbacks in the list.
  if (bind_resource_set_.is_bind_ongoing()) {
    pending_orphan_rebind_callbacks_.push_back(std::move(result_callback));
    return;
  }

  if (bind_resource_set_.NumOfAvailableResources() == 0) {
    result_callback(fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo>());
    TryResolvePendingNodes(/* force= */ true);
    return;
  }

  bind_resource_set_.StartNextBindProcess();

  // In case there is a pending call to TryBindAllAvailable() after this one, we automatically
  // restart the process and call all queued up callbacks upon completion.
  auto next_attempt =
      [this, result_callback = std::move(result_callback)](
          fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> results) mutable {
        result_callback(results);
        ProcessPendingBindRequests();
      };
  std::shared_ptr<BindResultTracker> tracker = std::make_shared<BindResultTracker>(
      bind_resource_set_.NumOfAvailableResources(), std::move(next_attempt));
  TryBindAllAvailableInternal(tracker);
  TryResolvePendingNodes(/* force= */ true);
}

void BindManager::Bind(Resource& resource, std::string_view driver_url_suffix,
                       std::shared_ptr<BindResultTracker> result_tracker) {
  std::shared_ptr node = resource.owner().lock();
  ZX_ASSERT_MSG(node, "Resource must have an owner to bind");
  BindRequest request = {
      .node_moniker = node->MakeComponentMoniker(),
      .resource = resource.weak_from_this(),
      .driver_url_suffix = std::string(driver_url_suffix),
      .tracker = result_tracker,
      .composite_only = false,
  };
  if (bind_resource_set_.is_bind_ongoing()) {
    pending_bind_requests_.push_back(std::move(request));
    if (!resource.is_self_resource()) {
      has_provided_resource_change_ = true;
    }
    TryResolvePendingNodes(/* force= */ false);
    return;
  }

  // Remove the resource from the orphaned resources to avoid collision.
  bind_resource_set_.RemoveOrphanedResource(resource.id());
  bind_resource_set_.StartNextBindProcess();

  auto next_attempt = [this]() mutable { ProcessPendingBindRequests(); };
  BindInternal(std::move(request), next_attempt);
  if (!resource.is_self_resource()) {
    has_provided_resource_change_ = true;
  }
  TryResolvePendingNodes(/* force= */ false);
}

void BindManager::TryBindAllAvailableInternal(std::shared_ptr<BindResultTracker> tracker) {
  ZX_ASSERT(bind_resource_set_.is_bind_ongoing());
  if (bind_resource_set_.NumOfAvailableResources() == 0) {
    return;
  }

  auto multibind_nodes = bind_resource_set_.CurrentMultibindResources();
  for (auto& [id, resource_weak] : multibind_nodes) {
    std::shared_ptr resource = resource_weak.lock();
    if (!resource) {
      tracker->ReportNoBind();
      continue;
    }
    std::shared_ptr node = resource->owner().lock();
    ZX_ASSERT(node);
    std::string moniker = node->MakeComponentMoniker();

    BindInternal(BindRequest{
        .node_moniker = std::move(moniker),
        .resource = resource_weak,
        .tracker = tracker,
        .composite_only = true,
    });
  }

  auto orphaned_nodes = bind_resource_set_.CurrentOrphanedResources();
  for (auto& [id, resource_weak] : orphaned_nodes) {
    std::shared_ptr resource = resource_weak.lock();
    if (!resource) {
      tracker->ReportNoBind();
      continue;
    }
    std::shared_ptr node = resource->owner().lock();
    ZX_ASSERT(node);
    std::string moniker = node->MakeComponentMoniker();

    BindInternal(BindRequest{
        .node_moniker = std::move(moniker),
        .resource = resource_weak,
        .tracker = tracker,
        .composite_only = false,
    });
  }
}

void BindManager::BindInternal(BindRequest request,
                               BindMatchCompleteCallback match_complete_callback) {
  ZX_ASSERT(bind_resource_set_.is_bind_ongoing());
  std::shared_ptr resource = request.resource.lock();
  if (!resource) {
    fdf_log::warn("Resource from node '{}' was freed before bind request is processed.",
                  request.node_moniker);
    if (request.tracker) {
      request.tracker->ReportNoBind();
    }
    match_complete_callback();
    return;
  }

  std::string driver_url_suffix = request.driver_url_suffix;
  auto match_callback =
      [this, request = std::move(request),
       match_complete_callback = std::move(match_complete_callback)](
          fidl::WireUnownedResult<fdi::DriverIndex::MatchDriver>& result) mutable {
        OnMatchDriverCallback(std::move(request), result, std::move(match_complete_callback));
      };
  fidl::Arena arena;
  auto builder = fuchsia_driver_index::wire::MatchDriverArgs::Builder(arena).name(resource->name());

  if (!resource->is_self_resource()) {
    builder.properties(fidl::ToWire(arena, resource->properties()));
  } else {
    std::shared_ptr node = resource->owner().lock();
    if (!node) {
      fdf_log::warn("Node was freed before bind request is processed. {}", request.node_moniker);
      if (request.tracker) {
        request.tracker->ReportNoBind();
      }
      match_complete_callback();
      return;
    }

    // Composite node's "default" node properties are its primary parent's node properties which
    // should not be used.
    if (node->type() == NodeType::kNormal) {
      if (std::optional props = node->GetNodeProperties(); props.has_value()) {
        builder.properties(fidl::ToWire(arena, props.value()));
      }
    }
  }

  if (!driver_url_suffix.empty()) {
    builder.driver_url_suffix(driver_url_suffix);
  }
  bridge_->RequestMatchFromDriverIndex(builder.Build(), std::move(match_callback));
}

void BindManager::OnMatchDriverCallback(
    BindRequest request, fidl::WireUnownedResult<fdi::DriverIndex::MatchDriver>& result,
    BindMatchCompleteCallback match_complete_callback) {
  auto report_no_bind = fit::defer([&request, &match_complete_callback]() mutable {
    if (request.tracker) {
      request.tracker->ReportNoBind();
    }
    match_complete_callback();
  });

  std::shared_ptr resource = request.resource.lock();
  if (!resource) {
    fdf_log::warn("Resource was freed before it could be bound for node '{}'",
                  request.node_moniker);
    return;
  }

  std::shared_ptr node = resource->owner().lock();

  // TODO(https://fxbug.dev/42075939): Add an additional guard to ensure that the node is still
  // available for binding when the match callback is fired. Currently, there are no issues from it,
  // but it is something we should address.
  if (!node) {
    fdf_log::warn("Node {} was freed before it could be bound", request.node_moniker);
    return;
  }
  auto node_moniker = node->MakeComponentMoniker();

  BindResult bind_result;
  if (resource->is_self_resource()) {
    bind_result =
        BindNodeToResult(*node, request.composite_only, result, request.tracker != nullptr);
    // If the resource fails to bind to anything, add it to the orphaned resources.
    if (!bind_result.bound() && !request.composite_only &&
        !bind_resource_set_.MultibindContains(resource->id())) {
      bind_resource_set_.AddOrphanedResource(resource);
      return;
    }
    bind_resource_set_.RemoveOrphanedResource(resource->id());
  } else {
    bind_result = BindResourceToResult(*resource, result, request.tracker != nullptr);
    bind_resource_set_.AddOrMoveMultibindResource(resource);
  }
  if (bind_result.bound()) {
    report_no_bind.cancel();
    if (request.tracker) {
      if (bind_result.is_driver_url()) {
        request.tracker->ReportSuccessfulBind(node_moniker, bind_result.driver_url());
      } else if (bind_result.is_composite_parents()) {
        request.tracker->ReportSuccessfulBind(node_moniker, bind_result.composite_parents());
      } else {
        fdf_log::error("Unknown bind result type for {}.", node_moniker);
      }
    }

    match_complete_callback();
  }
}

BindResult BindManager::BindNodeToResult(
    Node& node, bool composite_only, fidl::WireUnownedResult<fdi::DriverIndex::MatchDriver>& result,
    bool has_tracker) {
  if (!result.ok()) {
    fdf_log::error("Failed to call match Node '{}': {}", node.name(), result.error());
    return BindResult();
  }

  if (result->is_error()) {
    // Log the failed MatchDriver only if we are not tracking the results with a tracker
    // or if the error is not a ZX_ERR_NOT_FOUND error (meaning it could not find a driver).
    // When we have a tracker, the bind is happening for all the orphan resources and the
    // not found errors get very noisy.
    zx_status_t match_error = result->error_value();
    if (match_error != ZX_ERR_NOT_FOUND && !has_tracker) {
      fdf_log::warn("Failed to match Node '{}': {}", node.MakeTopologicalPath(),
                    zx_status_get_string(match_error));
    }

    return BindResult();
  }

  auto& matched_driver = result->value();
  if (composite_only && !matched_driver->is_composite_parents()) {
    return BindResult();
  }

  if (!matched_driver->is_driver() && !matched_driver->is_composite_parents()) {
    fdf_log::warn(
        "Failed to match Node '{}', the MatchedDriver is not a normal driver or a "
        "parent spec.",
        node.name());
    return BindResult();
  }

  if (matched_driver->is_composite_parents()) {
    fidl::Arena arena;
    auto result = BindNodeToSpec(arena, node, matched_driver->composite_parents());
    if (!result.is_ok()) {
      return BindResult();
    }

    auto owned_result = fidl::ToNatural(result.value());
    ZX_ASSERT(owned_result.has_value());
    return BindResult(owned_result.value());
  }

  ZX_ASSERT(matched_driver->is_driver());

  // If the node is already part of a composite, it should not bind to a driver.
  auto self_resource = node.GetSelfResource();
  if (self_resource.has_value() &&
      bind_resource_set_.MultibindContains(self_resource.value()->id())) {
    return BindResult();
  }

  auto start_result = bridge_->StartDriver(node, matched_driver->driver());
  if (start_result.is_error()) {
    fdf_log::error("Failed to start driver '{}': {}", node.name(),
                   zx_status_get_string(start_result.error_value()));
    return BindResult();
  }

  return BindResult(start_result.value());
}

BindResult BindManager::BindResourceToResult(
    Resource& resource, fidl::WireUnownedResult<fdi::DriverIndex::MatchDriver>& result,
    bool has_tracker) {
  if (!result.ok()) {
    fdf_log::error("Failed to call match Resource '{}': {}", resource.name(), result.error());
    return BindResult();
  }

  if (result->is_error()) {
    zx_status_t match_error = result->error_value();
    if (match_error != ZX_ERR_NOT_FOUND && !has_tracker) {
      std::shared_ptr node = resource.owner().lock();
      fdf_log::warn("Failed to match Resource '{}' for Node '{}': {}", resource.name(),
                    node ? node->MakeTopologicalPath() : "unknown",
                    zx_status_get_string(match_error));
    }
    return BindResult();
  }

  // Resources should only be matched as dependencies for a composite node.
  auto& matched_driver = result->value();
  if (!matched_driver->is_composite_parents()) {
    return BindResult();
  }

  fidl::Arena arena;
  auto bind_result = BindResourceToSpec(arena, resource, matched_driver->composite_parents());
  if (!bind_result.is_ok()) {
    return BindResult();
  }

  auto owned_result = fidl::ToNatural(bind_result.value());
  ZX_ASSERT(owned_result.has_value());
  return BindResult(owned_result.value());
}

zx::result<CompositeParents> BindManager::BindNodeToSpec(fidl::AnyArena& arena, Node& node,
                                                         CompositeParents parents) {
  auto self_resource = node.GetSelfResource();
  ZX_ASSERT(self_resource.has_value());

  if (node.can_multibind_composites()) {
    bind_resource_set_.AddOrMoveMultibindResource(self_resource.value());
  }

  auto result = bridge_->BindToParentSpec(arena, parents, self_resource.value(),
                                          node.can_multibind_composites());
  if (result.is_error()) {
    if (result.error_value() != ZX_ERR_NOT_FOUND) {
      fdf_log::error("Failed to bind node '{}' to any of the matched parent specs.", node.name());
    }

    node.OnMatchError(result.error_value());
    return result.take_error();
  }

  for (auto& composite : result.value().completed_node_and_drivers) {
    std::shared_ptr composite_node = composite.node.lock();
    ZX_ASSERT(composite_node);
    auto start_result = bridge_->StartDriver(*composite_node, composite.driver.driver_info());
    if (start_result.is_error()) {
      fdf_log::error("Failed to start driver '{}': {}", node.name(),
                     zx_status_get_string(start_result.error_value()));
    }
  }

  return zx::ok(result.value().bound_composite_parents);
}

zx::result<CompositeParents> BindManager::BindResourceToSpec(fidl::AnyArena& arena,
                                                             Resource& resource,
                                                             CompositeParents parents) {
  auto result = bridge_->BindToParentSpec(arena, parents, resource.shared_from_this(),
                                          /*enable_multibind=*/true);
  if (result.is_error()) {
    if (result.error_value() != ZX_ERR_NOT_FOUND) {
      fdf_log::error("Failed to bind resource '{}' to any of the matched parent specs.",
                     resource.name());
    }
    return result.take_error();
  }

  for (auto& composite : result.value().completed_node_and_drivers) {
    std::shared_ptr composite_node = composite.node.lock();
    ZX_ASSERT(composite_node);
    auto start_result = bridge_->StartDriver(*composite_node, composite.driver.driver_info());
    if (start_result.is_error()) {
      fdf_log::error("Failed to start driver '{}': {}", composite_node->name(),
                     zx_status_get_string(start_result.error_value()));
    }
  }
  return zx::ok(result.value().bound_composite_parents);
}

void BindManager::ProcessPendingBindRequests() {
  ZX_ASSERT(bind_resource_set_.is_bind_ongoing());
  if (pending_bind_requests_.empty() && pending_orphan_rebind_callbacks_.empty()) {
    bind_resource_set_.EndBindProcess();
    TryResolvePendingNodes(/* force= */ false);
    return;
  }

  // Consolidate the pending bind requests and orphaned resources to prevent collisions.
  for (auto& request : pending_bind_requests_) {
    if (auto resource = request.resource.lock(); resource) {
      bind_resource_set_.RemoveOrphanedResource(resource->id());
    }
  }

  bool have_bind_all_orphans_request = !pending_orphan_rebind_callbacks_.empty();
  // Begin the next bind process.
  bind_resource_set_.StartNextBindProcess();
  TryResolvePendingNodes(have_bind_all_orphans_request);
  size_t bind_tracker_size =
      have_bind_all_orphans_request
          ? pending_bind_requests_.size() + bind_resource_set_.NumOfAvailableResources()
          : pending_bind_requests_.size();

  // If there are no nodes to bind, then we'll run through all the callbacks and end the bind
  // process.
  if (have_bind_all_orphans_request && bind_tracker_size == 0) {
    for (auto& callback : pending_orphan_rebind_callbacks_) {
      fidl::Arena arena;
      callback(fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo>(arena, 0));
    }
    pending_orphan_rebind_callbacks_.clear();
    bind_resource_set_.EndBindProcess();
    TryResolvePendingNodes(/* force= */ true);
    return;
  }

  // Follow up with another ProcessPendingBindRequests() after all the pending bind calls are
  // complete. If there are no more accumulated bind calls, then the bind process ends.
  auto next_attempt =
      [this, callbacks = std::move(pending_orphan_rebind_callbacks_)](
          fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> results) mutable {
        for (auto& callback : callbacks) {
          callback(results);
        }
        ProcessPendingBindRequests();
      };

  std::shared_ptr<BindResultTracker> tracker =
      std::make_shared<BindResultTracker>(bind_tracker_size, std::move(next_attempt));

  // Go through all the pending bind requests.
  std::vector<BindRequest> pending_bind = std::move(pending_bind_requests_);
  for (auto& request : pending_bind) {
    auto match_complete_callback = [tracker]() mutable {
      // The bind status doesn't matter for this tracker.
      tracker->ReportNoBind();
    };
    BindInternal(std::move(request), std::move(match_complete_callback));
  }

  // If there are any pending callbacks for TryBindAllAvailable(), begin a new attempt.
  if (have_bind_all_orphans_request) {
    TryBindAllAvailableInternal(tracker);
  }
}

void BindManager::TryResolvePendingNodes(bool force) {
  if (force || has_provided_resource_change_) {
    has_provided_resource_change_ = false;
    bridge_->TryResolvePendingNodes();
  }
}

void BindManager::RecordInspect(inspect::Inspector& inspector) const {
  auto orphans = inspector.GetRoot().CreateChild("orphan_nodes");
  for (auto& [id, resource_weak] : bind_resource_set_.CurrentOrphanedResources()) {
    if (std::shared_ptr resource = resource_weak.lock()) {
      auto orphan = orphans.CreateChild(orphans.UniqueName("orphan-"));
      std::string moniker = "";
      if (auto owner = resource->owner().lock()) {
        moniker = owner->MakeComponentMoniker();
      }
      orphan.RecordString("moniker", moniker);
      orphans.Record(std::move(orphan));
    }
  }

  orphans.RecordBool("bind_all_ongoing", bind_resource_set_.is_bind_ongoing());
  orphans.RecordUint("pending_bind_requests", pending_bind_requests_.size());
  orphans.RecordUint("pending_orphan_rebind_callbacks", pending_orphan_rebind_callbacks_.size());
  inspector.GetRoot().Record(std::move(orphans));
}

std::vector<fdd::wire::CompositeNodeInfo> BindManager::GetCompositeListInfo(
    fidl::AnyArena& arena) const {
  // TODO(https://fxbug.dev/42071016): Add composite node specs to the list.
  return {};
}

}  // namespace driver_manager
