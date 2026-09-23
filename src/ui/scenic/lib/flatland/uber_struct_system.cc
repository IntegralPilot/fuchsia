// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/uber_struct_system.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <stack>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/logging.h"

namespace flatland {

// |UberStructSystem| implementations.

TransformHandle::InstanceId UberStructSystem::GetNextInstanceId() {
  // |latest_instance_id_| is only used for tests, but returning a member value can result in
  // threads "stealing" instance IDs from each other, so we return a local value here instead,
  // which does not have the same risk.
  auto next_instance_id = scheduling::GetNextSessionId();
  latest_instance_id_ = next_instance_id;
  return next_instance_id;
}

std::shared_ptr<UberStructSystem::UberStructQueue> UberStructSystem::AllocateQueueForSession(
    scheduling::SessionId session_id) {
  FX_DCHECK(!pending_structs_queues_.contains(session_id));

  auto [queue_kv, success] =
      pending_structs_queues_.emplace(session_id, std::make_shared<UberStructQueue>());
  FX_DCHECK(success);

  return queue_kv->second;
}

void UberStructSystem::RemoveSession(scheduling::SessionId session_id) {
  FX_DCHECK(pending_structs_queues_.contains(session_id));

  pending_structs_queues_.erase(session_id);
  snapshot_.map.erase(session_id);
}

UberStructSystem::UpdateResults UberStructSystem::UpdateInstances(
    const std::unordered_map<scheduling::SessionId, scheduling::PresentId>& instances_to_update) {
  TRACE_DURATION("gfx", "UberStructSystem::UpdateInstances", "count", instances_to_update.size());
  FLATLAND_VERBOSE_LOG << "UberStructSystem::UpdateSessions for " << instances_to_update.size()
                       << " sessions.";
  UpdateResults results;
  for (const auto& [session_id, present_id] : instances_to_update) {
    // Find the queue associated with this SessionId.  It may not exist if the session was destroyed
    // after a frame was scheduled (or for other unknown reasons).
    auto queue_kv = pending_structs_queues_.find(session_id);
    if (queue_kv == pending_structs_queues_.end()) {
      // TODO(https://fxbug.dev/467350358): consider cleaning up destroyed sessions more thoroughly
      // so that we can make this a DCHECK.
      FX_LOGS(WARNING) << "No UberStructQueue found for session_id=" << session_id
                       << " (maybe it was destroyed?)";
      continue;
    }

    bool successful_update = false;
    uint32_t present_credits_returned = 0;

    // Pop entries from that queue until the correct PresentId is found, then commit that
    // UberStruct to the snapshot. If the next pending UberStruct has a PresentId greater than the
    // target one, the update has failed because PresentIds are strictly increasing.
    auto pending_struct = queue_kv->second->Pop();

    bool queue_needs_recompute_view_tree = false;
    while (pending_struct.has_value()) {
      ++present_credits_returned;

      // We may squash some UberStructs together; we must recompute the view tree if any of them
      // required recomputation.
      queue_needs_recompute_view_tree =
          queue_needs_recompute_view_tree || pending_struct->recompute_view_tree;

      if (pending_struct->present_id == present_id) {
        FLATLAND_VERBOSE_LOG << "    Updating UberStruct for session_id=" << session_id
                             << " present_id=" << present_id
                             << " recompute_view_tree=" << queue_needs_recompute_view_tree;
        snapshot_.map[session_id] = std::move(pending_struct->uber_struct);
        successful_update = true;
        break;
      }
      if (pending_struct->present_id > present_id) {
        FX_LOGS(WARNING) << "Popped past the target present_id (target=" << present_id
                         << " found=" << pending_struct->present_id << ")";
        break;
      }

      pending_struct = queue_kv->second->Pop();
    }
    recompute_view_tree_ = recompute_view_tree_ || queue_needs_recompute_view_tree;

    FX_DCHECK(successful_update) << "No UberStruct found for session_id=" << session_id
                                 << " present_id=" << present_id;
    results.present_credits_returned[session_id] = present_credits_returned;
  }
  return results;
}

void UberStructSystem::ForceUpdateAllSessions(size_t max_updates_per_queue) {
  // Pop entries from each queue until empty.
  for (auto& [session_id, queue] : pending_structs_queues_) {
    size_t update_count = 0;
    while (auto pending_struct = queue->Pop()) {
      snapshot_.map[session_id] = std::move(pending_struct->uber_struct);
      recompute_view_tree_ = recompute_view_tree_ || pending_struct->recompute_view_tree;
      if (++update_count == max_updates_per_queue) {
        break;
      }
    }
  }
}

const UberStructSnapshot& UberStructSystem::Snapshot() { return snapshot_; }

size_t UberStructSystem::GetSessionCount() { return pending_structs_queues_.size(); }

TransformHandle::InstanceId UberStructSystem::GetLatestInstanceId() const {
  return latest_instance_id_;
}

// |UberStructSystem::Queue| implementations.

void UberStructSystem::UberStructQueue::Push(scheduling::PresentId present_id,
                                             std::unique_ptr<const UberStruct> uber_struct,
                                             bool recompute_view_tree) {
  FX_DCHECK(uber_struct);
#ifndef NDEBUG
  // PresentIds must be strictly increasing
  FX_DCHECK(!last_present_id_.load() || last_present_id_.load() < present_id);
  last_present_id_.store(present_id);
#endif

  pending_structs_.Push(PendingUberStruct{.present_id = present_id,
                                          .uber_struct = std::move(uber_struct),
                                          .recompute_view_tree = recompute_view_tree});
}

std::optional<UberStructSystem::PendingUberStruct> UberStructSystem::UberStructQueue::Pop() {
  return pending_structs_.Pop();
}

namespace {

struct Indenter {
  size_t depth;
};

std::ostream& operator<<(std::ostream& out, const Indenter& indenter) {
  size_t depth = indenter.depth;
  while (depth-- > 0) {
    out << " ";
  }
  return out;
}

}  // namespace

std::ostream& operator<<(std::ostream& out, const UberStructLayer::ImageModeProperties& image) {
  return out << "image: id=" << image.image_id << " sample_rect=" << image.sample_rect
             << " transform=" << image.transform;
}

std::ostream& operator<<(std::ostream& out,
                         const UberStructLayer::SolidColorModeProperties& solid) {
  return out << "color: color=" << solid.color[0] << "," << solid.color[1] << "," << solid.color[2]
             << "," << solid.color[3];
}

std::ostream& operator<<(std::ostream& out, const UberStructLayer& layer) {
  out << "display_rect=" << layer.common.display_rect << " opacity=" << layer.common.opacity
      << " blend_mode=" << layer.common.blend_mode;
  if (std::holds_alternative<UberStructLayer::ImageModeProperties>(layer.content)) {
    out << " " << std::get<UberStructLayer::ImageModeProperties>(layer.content);
  } else if (std::holds_alternative<UberStructLayer::SolidColorModeProperties>(layer.content)) {
    out << " " << std::get<UberStructLayer::SolidColorModeProperties>(layer.content);
  } else {
    static_assert(3 == std::variant_size_v<decltype(UberStructLayer::content)>,
                  "Must handle all UberStructLayer content types");
    out << " invisible";
  }
  return out;
}

std::ostream& operator<<(std::ostream& out, const UberStruct& us) {
  if (us.view_ref) {
    out << *us.view_ref << "\n";
  }

  auto& topology = us.local_topology;

  size_t index = 0;
  std::stack<uint64_t> children_remaining;
  children_remaining.push(1);  // The root of the topology.

  while (index < topology.size()) {
    auto& handle = topology[index].handle;

    out << Indenter{children_remaining.size()} << handle;

    {
      auto it = us.local_opacity_values.find(handle);
      if (it != us.local_opacity_values.end()) {
        out << "  opacity=" << it->second;
      }
    }

    {
      auto it = us.local_clip_regions.find(handle);
      if (it != us.local_clip_regions.end()) {
        out << "  clip_region=" << it->second;
      }
    }

    {
      auto it = us.layer_stacks.find(handle);
      if (it != us.layer_stacks.end()) {
        out << "  layer_stack=[";
        bool first = true;
        for (const auto& layer_handle : it->second) {
          if (!first) {
            out << ", ";
          }
          first = false;
          out << layer_handle;

          auto layer_it = us.layers.find(layer_handle);
          if (layer_it != us.layers.end()) {
            out << "(" << layer_it->second << ")";
          }
        }
        out << "]";
      }
    }

    out << "\n";

    FX_DCHECK(!children_remaining.empty() && children_remaining.top() > 0);
    --children_remaining.top();

    if (topology[index].child_count > 0) {
      children_remaining.push(topology[index].child_count);
    }

    while (!children_remaining.empty() && children_remaining.top() == 0) {
      children_remaining.pop();
    }

    ++index;
  }

  return out;
}

}  // namespace flatland
