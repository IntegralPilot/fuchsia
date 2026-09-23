// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/bind/bind_resource_set.h"

#include "src/devices/bin/driver_manager/resource.h"

namespace driver_manager {

void BindResourceSet::StartNextBindProcess() {
  if (is_bind_ongoing_) {
    CompleteOngoingBind();
  }

  new_orphaned_resources_ = orphaned_resources_;
  is_bind_ongoing_ = true;
  NotifyBindState();
}

void BindResourceSet::EndBindProcess() {
  ZX_ASSERT(is_bind_ongoing_);
  CompleteOngoingBind();
  is_bind_ongoing_ = false;
  NotifyBindState();
}

void BindResourceSet::CompleteOngoingBind() {
  ZX_ASSERT(is_bind_ongoing_);
  orphaned_resources_ = std::move(new_orphaned_resources_);

  for (auto& [id, resource_weak] : new_multibind_resources_) {
    multibind_resources_[id] = resource_weak;
  }
  new_multibind_resources_ = {};
}

void BindResourceSet::NotifyBindState() {
  if (on_bind_state_changed_) {
    on_bind_state_changed_();
  }
}

void BindResourceSet::AddOrphanedResource(const std::shared_ptr<Resource>& resource) {
  ResourceId id = resource->id();
  ZX_ASSERT(!MultibindContains(id));
  if (is_bind_ongoing_) {
    new_orphaned_resources_[id] = resource;
    return;
  }
  orphaned_resources_[id] = resource;
}

void BindResourceSet::RemoveOrphanedResource(ResourceId id) {
  if (is_bind_ongoing_) {
    new_orphaned_resources_.erase(id);
    return;
  }
  orphaned_resources_.erase(id);
}

void BindResourceSet::AddOrMoveMultibindResource(const std::shared_ptr<Resource>& resource) {
  ResourceId id = resource->id();
  RemoveOrphanedResource(id);
  if (is_bind_ongoing_) {
    new_multibind_resources_[id] = resource;
    return;
  }
  multibind_resources_[id] = resource;
}

bool BindResourceSet::MultibindContains(ResourceId id) const {
  return multibind_resources_.find(id) != multibind_resources_.end() ||
         new_multibind_resources_.find(id) != new_multibind_resources_.end();
}

}  // namespace driver_manager
