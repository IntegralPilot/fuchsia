// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_BIND_BIND_RESOURCE_SET_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_BIND_BIND_RESOURCE_SET_H_

#include <unordered_map>

#include "src/devices/bin/driver_manager/node.h"

namespace driver_manager {

class Resource;

// This class keeps track of all the resources available for binding. Its purpose is to prevent the
// set of resources from being modified during an ongoing bind process, which is the cause of most
// async bind errors. During an ongoing bind process, it'll track any new changes to the
// orphaned and multibind resources, and then apply the changes once the process is complete.
class BindResourceSet {
 public:
  // Starts the next bind process. If there's already an ongoing bind, complete it and prepare
  // the resource sets for the next bind process.
  void StartNextBindProcess();

  // Complete the bind process and set |is_bind_ongoing_| to false. Must only be called when
  // |is_bind_ongoing_| is true.
  void EndBindProcess();

  void AddOrphanedResource(const std::shared_ptr<Resource>& resource);

  // If available, remove the resource with the matching |resource_id| from the orphaned resources.
  void RemoveOrphanedResource(ResourceId resource_id);

  // Add |resource| to the multibind resources. Remove it from the orphaned resources if it exists.
  void AddOrMoveMultibindResource(const std::shared_ptr<Resource>& resource);
  bool MultibindContains(ResourceId resource_id) const;

  // Functions to return a copy of |orphaned_resources_| and |multibind_resources_|. We return a
  // copy, not const reference to prevent iterator invalidating errors.
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> CurrentOrphanedResources() const {
    return orphaned_resources_;
  }
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> CurrentMultibindResources() const {
    return multibind_resources_;
  }

  void set_on_bind_state_changed(fit::function<void()> callback) {
    on_bind_state_changed_ = std::move(callback);
  }

  size_t NumOfOrphanedResources() const { return orphaned_resources_.size(); }
  size_t NumOfAvailableResources() const {
    return orphaned_resources_.size() + multibind_resources_.size();
  }

  bool is_bind_ongoing() const { return is_bind_ongoing_; }

 private:
  // Completes the current bind process by applying all the new changes to |orphaned_resources_| and
  // |multibind_resources_|. Must only be called when |is_bind_ongoing_| is true.
  void CompleteOngoingBind();

  void NotifyBindState();

  // Orphaned resources are resources that have failed to bind to a driver, either
  // because no matching driver could be found, or because the matching driver
  // failed to start. Maps the ResourceId to the resource's weak pointer. Should be mutually
  // exclusive to |multibind_resources_|.
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> orphaned_resources_;

  // A list of resources that can multibind to composites. A resource's owner node can parent
  // multiple composite nodes. To support this, we store the resources in a map to bind
  // them to other composites. A resource is added to this set if its owner node is matched to a
  // composite's parent and can multibind to composites. Should be mutually exclusive to
  // |orphaned_resources_|.
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> multibind_resources_;

  // Sets that contain the new changes to |orphaned_resources_| and |multibind_resources_|. When
  // CompleteOngoingBind() is called, the changes are moved into the main sets.
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> new_orphaned_resources_;
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> new_multibind_resources_;

  fit::function<void()> on_bind_state_changed_;

  // True when a bind process is ongoing. Set to true by StartNextBindProcess() and false by
  // EndBindProcess().
  bool is_bind_ongoing_ = false;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_BIND_BIND_RESOURCE_SET_H_
