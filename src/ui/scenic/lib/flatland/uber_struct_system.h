// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_UBER_STRUCT_SYSTEM_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_UBER_STRUCT_SYSTEM_H_

#ifndef NDEBUG
#include <atomic>
#endif
#include <optional>
#include <unordered_map>

#include "src/lib/containers/cpp/mpsc_queue.h"
#include "src/ui/scenic/lib/flatland/transform_handle.h"
#include "src/ui/scenic/lib/flatland/uber_struct.h"

namespace flatland {

// Snapshot of all Flatland sessions' state, plus additional info.
struct UberStructSnapshot {
  UberStruct::InstanceMap map;
};

// TODO(https://fxbug.dev/42122511): write a bug to find a better name for this system
//
// A system for aggregating local data from Flatland instances to be consumed by the render loop.
// All functions are thread safe. The intent is for separate worker threads to own each Flatland
// instance, compute local data (such as topology vectors) in their local thread, and then commit
// those vectors to this class in a concurrent manner.
class UberStructSystem {
 public:
  UberStructSystem() = default;

  // Returns the next instance ID for this particular UberStructSystem. Instance IDs are guaranteed
  // to be unique for each caller and should be used as keys for setting UberStructs and accessing
  // UberStructs in snapshots.
  TransformHandle::InstanceId GetNextInstanceId();

  // An UberStruct that has not been published to the visible snapshot and the PresentId it is
  // associated with.
  struct PendingUberStruct {
    scheduling::PresentId present_id;
    std::unique_ptr<const UberStruct> uber_struct;
    bool recompute_view_tree = true;
  };

  // An interface for UberStructSystem clients to queue UberStructs to be published into the
  // visible snapshot.
  class UberStructQueue {
   public:
    // Queues an UberStruct for |present_id|. Each Flatland instance can queue multiple UberStructs
    // in the UberStructSystem by using different PresentIds. PresentIds must be increasing between
    // subsequent calls.
    void Push(scheduling::PresentId present_id, std::unique_ptr<const UberStruct> uber_struct,
              bool recompute_view_tree = true);

    // Pops a PendingUberStruct off of this Queue. If the queue is currently empty, returns
    // std::nullopt.
    std::optional<PendingUberStruct> Pop();

   private:
#ifndef NDEBUG
    std::atomic<scheduling::PresentId> last_present_id_;
#endif
    containers::MpscQueue<PendingUberStruct> pending_structs_;
  };

  // Allocates an UberStructQueue for |session_id| and returns a shared reference to that
  // UberStructQueue. Callers should call |RemoveSession| when the session associated with that
  // |session_id| has exited to clean up the allocated resources.
  std::shared_ptr<UberStructQueue> AllocateQueueForSession(scheduling::SessionId session_id);

  // Removes the UberStructQueue and current UberStruct associated with |session_id|. Any
  // PendingUberStructs pushed into the queue after this call will never be published to the
  // InstanceMap.
  void RemoveSession(scheduling::SessionId session_id);

  // The results of calling UpdateInstances().
  struct UpdateResults {
    // The number of present tokens available to each updated session. Values will always be
    // positive; sessions with no available tokens will be excluded.
    std::unordered_map<scheduling::SessionId, /*present_credits_returned=*/uint32_t>
        present_credits_returned;
  };

  // Commits a new UberStruct to the instance map for each key/value pair in |sessions_to_update|.
  // All pending UberStructs associated with each SessionId with lower PresentIds will be
  // discarded.
  UpdateResults UpdateInstances(
      const std::unordered_map<scheduling::SessionId, scheduling::PresentId>& instances_to_update);

  // Snapshots the current map of UberStructs and returns the copy.
  // Note that this function returns a reference. This structure may only be modified on main
  // thread.
  const UberStructSnapshot& Snapshot();

  // For pushing all pending UberStructs in tests.
  void ForceUpdateAllSessions(size_t max_updates_per_queue = 10);

  // For validating cleanup logic in tests.
  size_t GetSessionCount();

  // For getting Flatland InstanceIds in tests.
  TransformHandle::InstanceId GetLatestInstanceId() const;

  // Returns true if the ViewTree needs to be recomputed.  Clears the stored value, so immediatlely
  // calling it a second time will always return false.
  bool MustRecomputeViewTree() {
    if (recompute_view_tree_) {
      recompute_view_tree_ = false;
      return true;
    }
    return false;
  }

 private:
  // The queue of UberStructs pending for each active session. Flatland instances push UberStructs
  // onto these queues using |UberStructQueue::Push()|. This UberStructSystem removes entries using
  // |UberStructQueue::Pop()|. Both of those operations are threadsafe, but the map itself is only
  // modified from a single thread.
  std::unordered_map<scheduling::SessionId, std::shared_ptr<UberStructQueue>>
      pending_structs_queues_;

  // The current UberStruct for each Flatland instance.
  UberStructSnapshot snapshot_;

  // Track whether the ViewTree needs to be recomputed.
  bool recompute_view_tree_ = false;

  // The InstanceId most recently returned from GetNextInstanceId().
  TransformHandle::InstanceId latest_instance_id_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_UBER_STRUCT_SYSTEM_H_
