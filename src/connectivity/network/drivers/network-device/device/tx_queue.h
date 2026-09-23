// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_TX_QUEUE_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_TX_QUEUE_H_

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/port.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <fbl/mutex.h>

#include "data_structs.h"
#include "device_interface.h"

namespace network::internal {

class Session;

class TxQueue {
 public:
  static zx::result<std::unique_ptr<TxQueue>> Create(DeviceInterface* parent);

  ~TxQueue();
  // Terminates and join the |TxQueue| worker thread.
  void JoinThread();

  // Notifies the worker thread that new tx buffers are available.
  void Resume();

  // Helper functions with TA annotations that bridges the gap between parent's locks and local
  // locking requirements; TA is not otherwise able to tell that the |parent| and |parent_| are the
  // same entity.
  void AssertParentTxLocked(DeviceInterface& parent) __TA_REQUIRES(parent.tx_lock())
      __TA_ASSERT(parent_->tx_lock()) {
    ZX_DEBUG_ASSERT(parent_ == &parent);
  }

  // Sets the session for this TxQueue. The session's tx fifo will be observed and
  // the session is notified when data is available to be fetched and sent to
  // the device through |Session::FetchTx|.
  void SetSession(Session* session) __TA_REQUIRES(parent_->tx_lock());
  bool HasSession() __TA_REQUIRES(parent_->tx_lock());

  // Helper class to handle Tx transactions from the session.
  class SessionTransaction {
   public:
    SessionTransaction(cpp20::span<fuchsia_hardware_network_driver::wire::TxBuffer> buffers,
                       TxQueue* parent) __TA_REQUIRES(parent->parent_->tx_lock());
    void Commit() __TA_EXCLUDES(queue_->parent_->tx_lock());

    uint32_t available() const { return available_; }
    bool overrun() const { return available_ == 0; }
    fuchsia_hardware_network_driver::wire::TxBuffer* GetBuffer();
    void Push(uint16_t descriptor) __TA_REQUIRES(queue_->parent_->tx_lock());

    void AssertParentTxLock(DeviceInterface& parent) __TA_ASSERT(queue_->parent_->tx_lock())
        __TA_REQUIRES(parent.tx_lock()) {
      ZX_DEBUG_ASSERT(&parent == queue_->parent_);
    }

   private:
    cpp20::span<fuchsia_hardware_network_driver::wire::TxBuffer> buffers_;
    //  Pointer to queue over which transaction is opened, not owned.
    TxQueue* const queue_;
    uint32_t available_;
    uint32_t queued_;
    DISALLOW_COPY_ASSIGN_AND_MOVE(SessionTransaction);
  };

  // Marks all buffers in tx as complete, returning them to the session.
  void CompleteTxList(const fidl::VectorView<fuchsia_hardware_network_driver::wire::TxResult>& tx)
      __TA_EXCLUDES(parent_->tx_lock());

 private:
  explicit TxQueue(DeviceInterface* parent) : parent_(parent) {}

  void Thread(cpp20::span<fuchsia_hardware_network_driver::wire::TxBuffer> buffers);
  zx_status_t EnqueueUserPacket(uint64_t key);
  zx_status_t UpdateFifoWatches();

  zx_status_t HandleFifoSignal(cpp20::span<fuchsia_hardware_network_driver::wire::TxBuffer> buffers,
                               zx_signals_t signals);

  struct InFlightBuffer {
    InFlightBuffer() = default;
    InFlightBuffer(zx_status_t result, uint16_t descriptor_index)
        : result(result), descriptor_index(descriptor_index) {}
    zx_status_t result;
    uint16_t descriptor_index;
  };

  // Wrapper struct to keep track of the session with associated
  // information on whether we have an async wait installed on |port_| for tx
  // FIFOs.
  struct SessionWaiter {
    Session* session;
    bool wait_installed;
  };

  // Adds the provided session:descriptor tuple to the queue and returns the buffer id.
  uint32_t Enqueue(uint16_t descriptor) __TA_REQUIRES(parent_->tx_lock());
  // Returns all outstanding completed buffers to the session.
  //
  // Returns true if device buffers were full and the session should be notified.
  [[nodiscard]] bool ReturnBuffers() __TA_REQUIRES(parent_->tx_lock());

  static constexpr uint64_t kResumeKey = 1;
  static constexpr uint64_t kQuitKey = 2;

  // pointer to parent device, not owned.
  DeviceInterface* const parent_;

  std::unique_ptr<RingQueue<uint32_t>> return_queue_ __TA_GUARDED(parent_->tx_lock());
  std::unique_ptr<IndexedSlab<InFlightBuffer>> in_flight_ __TA_GUARDED(parent_->tx_lock());
  std::optional<SessionWaiter> session_ __TA_GUARDED(parent_->tx_lock());

  zx::port port_;
  fdf::Dispatcher dispatcher_;
  libsync::Completion dispatcher_shutdown_;
  std::atomic<bool> running_;

  static std::atomic<uint32_t> num_instances_;

  DISALLOW_COPY_ASSIGN_AND_MOVE(TxQueue);
};
}  // namespace network::internal

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_TX_QUEUE_H_
