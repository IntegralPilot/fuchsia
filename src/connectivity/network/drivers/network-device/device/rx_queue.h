// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_RX_QUEUE_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_RX_QUEUE_H_

#include <lib/sync/cpp/completion.h>
#include <lib/zx/port.h>
#include <lib/zx/thread.h>
#include <lib/zx/timer.h>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>

#include "data_structs.h"
#include "definitions.h"
#include "device_interface.h"
#include "simple_rx_buffer_estimator.h"

namespace network::internal {

class Session;

struct VmoOperation {
  enum class Type : uint8_t { kNone, kPrepareBatch, kReleaseSingle };
  Type type{Type::kNone};
  // Only used for the prepare action.
  //
  // The range (rx_vmo_state_.last_in_use, first_vmo_to_not_prepare) is being prepared
  // by DeviceInterface (calling the driver implementation's PrepareVmos method).
  RxVmoList::iterator first_vmo_to_not_prepare;
};

class RxQueue {
 public:
  static constexpr uint64_t kTriggerRxKey = 1;
  static constexpr uint64_t kSessionSwitchKey = 2;
  static constexpr uint64_t kFifoWatchKey = 3;
  static constexpr uint64_t kQuitWatchKey = 4;
  static constexpr uint64_t kTimerWatchKey = 5;

  static zx::result<std::unique_ptr<RxQueue>> Create(DeviceInterface* parent);
  ~RxQueue();

  // Helper function with TA annotations that bridges the gap between parent's locks and local
  // locking requirements; TA is not otherwise able to tell that the |parent| and |parent_| are the
  // same entity.
  void AssertParentRxLocked(DeviceInterface& parent) __TA_REQUIRES(parent.rx_lock())
      __TA_ASSERT(parent_->rx_lock()) {
    ZX_DEBUG_ASSERT(parent_ == &parent);
  }
  // Drops all queued buffers attributed to the given session, and marks the session as rx-disabled.
  // Called by the DeviceInterface parent when the session is marked as dead.
  void PurgeSession();
  // Returns rx buffers to their respective sessions.
  void CompleteRxList(
      const fidl::VectorView<::fuchsia_hardware_network_driver::wire::RxBuffer>& rx_buffer_list)
      __TA_EXCLUDES(parent_->rx_lock());
  // Notifies watcher thread that the session changed.
  void TriggerSessionChanged();
  // Poke watcher thread to try to fetch more rx descriptors.
  void TriggerRxWatch();
  // Kills and joins the watcher thread.
  void JoinThread();
  // Helper function to verify if a pending rx lease can be delegated to the
  // session.
  void MaybeDelegateRxLease() __TA_REQUIRES(parent_->rx_lock())
      __TA_REQUIRES_SHARED(parent_->control_lock());
  void SetSession(Session* session) __TA_REQUIRES(parent_->rx_lock()) { session_ = session; }
  void SetRxBufferManagement(const netdriver::RxBufferManagement& management)
      __TA_REQUIRES(parent_->rx_lock());

  // Updates the packet arrival rate for dynamic Rx buffer management.
  void UpdatePeakRateInSamplePeriod(uint64_t sample_rate) __TA_REQUIRES(parent_->rx_lock());
  // Runs when a timer is fired for dynamic Rx buffer management to update target buffers and
  // evaluate VMO releases.
  void TimerTick() __TA_REQUIRES(parent_->rx_lock());

  // Like the |SetTargetRxBuffer|, but only updates the target buffer if requesting more.
  // Returns true if the target buffer number is increased.
  [[nodiscard]] bool SetTargetRxBuffersIfMore(uint16_t target) __TA_REQUIRES(parent_->rx_lock());
  // Requests for more Rx buffers if available to accommodate the target available buffers.
  // The optional callback is called after wanted VMOs are prepared. Note that the callback
  // is NOT called if |RequestRxSpace| does not schedule any VMOs to be prepared.
  void RequestRxSpace(fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> cb =
                          nullptr) __TA_REQUIRES(parent_->rx_lock());

  // Sets the Rx VMOs in use for the queue. The list must be sorted by their VMO ids to allow
  // fast check whether a buffer descriptor belongs to a prepared VMO or not. These VMOs may
  // be dynamically managed based on the parameters provided by the device implementation driver.
  void SetRxVmos(RxVmoList rx_vmos) __TA_REQUIRES(parent_->rx_lock());
  // Callback for VMO operation completion. When a preparation completes, it makes all previously
  // withheld buffers available. When a release completes, it decommits the VMO to finish memory
  // reclamation.
  void VmoOpFinished(zx_status_t status = ZX_OK) __TA_REQUIRES(parent_->rx_lock());
  uint64_t rx_completed_frame_index() const __TA_REQUIRES(parent_->rx_lock()) {
    return rx_completed_frame_index_;
  }

  // A transaction to add buffers from a session to the RxQueue.
  class SessionTransaction {
   public:
    explicit SessionTransaction(RxQueue* parent) __TA_REQUIRES(parent->parent_->rx_lock())
        : queue_(parent) {}
    ~SessionTransaction() __TA_REQUIRES(queue_->parent_->rx_lock()) = default;
    uint32_t remaining();
    bool Push(uint16_t descriptor);
    void AssertLock(DeviceInterface& parent) __TA_ASSERT(parent.rx_lock()) {
      ZX_DEBUG_ASSERT(queue_->parent_ == &parent);
    }

   private:
    // Pointer to parent queue, not owned.
    RxQueue* const queue_;
    DISALLOW_COPY_ASSIGN_AND_MOVE(SessionTransaction);
  };

 private:
  explicit RxQueue(DeviceInterface* parent) : parent_(parent) {}

  struct InFlightBuffer {
    InFlightBuffer() = default;
    explicit InFlightBuffer(uint16_t descriptor_index)
        : descriptor_index(descriptor_index), next_withheld_inflight_buffer(kInvalidIdx) {}
    uint16_t descriptor_index;
    uint32_t next_withheld_inflight_buffer;
  };
  // Get a single buffer from the queue, along with its identifier. On success, the buffer is popped
  // from the queue. The returned buffer pointer is still owned by the queue and the pointer should
  // not outlive the currently held lock.
  std::tuple<InFlightBuffer*, uint32_t> GetBuffer() __TA_REQUIRES(parent_->rx_lock())
      __TA_REQUIRES_SHARED(parent_->control_lock());
  // Pops a buffer from the queue, if any are available, and stores the space information in `buff`.
  // Returns ZX_ERR_NO_RESOURCES if there are no buffers available.
  zx_status_t PrepareBuff(fuchsia_hardware_network_driver::wire::RxSpaceBuffer* buff)
      __TA_REQUIRES(parent_->rx_lock()) __TA_REQUIRES_SHARED(parent_->control_lock());
  // Pushes all the unavailable in-flight buffers from the target VMO into the available_queue so
  // that it can be used by the device driver.
  void ReleaseWithheldBuffers(RxVmoList::iterator target) __TA_REQUIRES(parent_->rx_lock());
  // Checks if the candidate VMO (last_in_use) is fully withheld and eligible for release, and
  // initiates the release operation if so.
  void MaybeReleaseCandidateVmo() __TA_REQUIRES(parent_->rx_lock())
      __TA_EXCLUDES(parent_->control_lock());
  // Sets the target buffers to use for Rx, clamps the value between the minimum rx buffers and
  // total rx buffers. Returns true if target is increased and potentially more Rx VMO is needed.
  [[nodiscard]] bool SetTargetRxBuffers(uint16_t target) __TA_REQUIRES(parent_->rx_lock());
  int WatchThread(
      std::unique_ptr<fuchsia_hardware_network_driver::wire::RxSpaceBuffer[]> space_buffers);

  // pointer to parent device, not owned.
  DeviceInterface* const parent_;
  Session* session_ __TA_GUARDED(parent_->rx_lock()) = nullptr;
  std::unique_ptr<IndexedSlab<InFlightBuffer>> in_flight_ __TA_GUARDED(parent_->rx_lock());
  std::unique_ptr<RingQueue<uint32_t>> available_queue_ __TA_GUARDED(parent_->rx_lock());
  uint16_t device_buffer_count_ __TA_GUARDED(parent_->rx_lock()) = 0;
  uint64_t rx_completed_frame_index_ __TA_GUARDED(parent_->rx_lock()) = 0;

  zx::port rx_watch_port_;
  fdf::Dispatcher dispatcher_;
  libsync::Completion dispatcher_shutdown_;
  std::atomic<bool> running_;

  static std::atomic<uint32_t> num_instances_;
  struct {
    // List of all registered Rx VMOs, sorted by VMO ID ascending.
    RxVmoList vmos;
    // Iterator pointing to the last VMO in `vmos` that is currently in use (prepared).
    RxVmoList::iterator last_in_use;
    // Total number of Rx buffers across all currently prepared VMOs.
    uint16_t current_available_buffers = 0;
    // Desired number of Rx buffers as computed by `estimator`.
    uint16_t target_available_buffers = 0;
    // Sum of Rx buffers across all registered VMOs.
    uint16_t total_buffers = 0;
    // Minimum allowed target Rx buffers (e.g. buffers from the first VMO).
    uint16_t min_buffers = 0;
    // An ongoing asynchronous prepare or release operation on VMOs, if any.
    VmoOperation pending_op;
    // Optional dynamic buffer estimator for automatic Rx VMO scaling.
    std::optional<SimpleRxBufferEstimator> estimator;
    // Timestamp of the last Rx batch completion, used to calculate packet arrival rates.
    zx::time last_complete_time = zx::time::infinite_past();
    // Peak estimated packet arrival rate observed within the current sampling interval.
    uint64_t current_max_packet_rate = 0;
  } rx_vmo_state_ __TA_GUARDED(parent_->rx_lock());

  DISALLOW_COPY_ASSIGN_AND_MOVE(RxQueue);
};

// Newtype for the internal SessionTransaction class to allow other header files to forward declare
// it.
class RxSessionTransaction : public RxQueue::SessionTransaction {
 private:
  friend RxQueue;
  explicit RxSessionTransaction(RxQueue* parent) : RxQueue::SessionTransaction(parent) {}
};

}  // namespace network::internal

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_RX_QUEUE_H_
