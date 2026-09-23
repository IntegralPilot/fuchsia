// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_INSPECT_INCLUDE_USB_INSPECT_USB_INSPECT_H_
#define SRC_DEVICES_USB_LIB_USB_INSPECT_INCLUDE_USB_INSPECT_USB_INSPECT_H_

#include <lib/async/cpp/task.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/types.h>
// We include this legacy Banjo header strictly to retrieve the `usb_speed_t` enum type
// used in our public API method signatures, allowing consumers to pass strongly-typed speed enums.
#include <fuchsia/hardware/usb/c/banjo.h>

#include <atomic>
#include <deque>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include <usb/descriptors.h>

namespace usb_inspect {

const char* SpeedToString(usb_speed_t speed);

// Class to manage common USB endpoint diagnostics using a lazy inspect node.
// Hot-path counters are atomic and VMO writes are completely eliminated from
// the driver runtime (only populated on-demand upon query).
class EndpointInspect {
 public:
  // Default capacity for event log history. Set to 15 to capture multiple occurrences
  // of endpoint-specific state transitions (such as stall/clear-stall).
  static constexpr size_t kDefaultEventCapacity = 15;

  // Default capacity for transfer snapshots history.
  static constexpr size_t kDefaultTransferCapacity = 15;

  // Encapsulates cumulative transfer statistics for root properties and snapshots.
  struct TransferStats {
    // Total bytes transmitted since boot.
    uint64_t total_bytes_tx = 0;
    // Total bytes received since boot.
    uint64_t total_bytes_rx = 0;
    // Maximum bytes per second observed since boot.
    uint64_t max_bytes_per_second = 0;
  };

  EndpointInspect() = default;

  void Init(inspect::Node& parent, const std::string& name,
            size_t event_capacity = kDefaultEventCapacity,
            size_t transfer_snapshot_capacity = kDefaultTransferCapacity);
  // Update TX pending requests count.
  void UpdateTxQueue(size_t pending_requests) {
    tx_pending_requests_.store(pending_requests, std::memory_order_relaxed);
  }

  // Update RX pending requests count.
  void UpdateRxQueue(size_t pending_requests) {
    rx_pending_requests_.store(pending_requests, std::memory_order_relaxed);
  }

  void UpdateRxPendingProcessing(size_t pending_requests) {
    rx_pending_processing_.store(pending_requests, std::memory_order_relaxed);
  }

  void AddTxBytes(size_t bytes) { total_bytes_tx_.fetch_add(bytes, std::memory_order_relaxed); }

  void AddRxBytes(size_t bytes) { total_bytes_rx_.fetch_add(bytes, std::memory_order_relaxed); }

  void AddFailedTxBytes(size_t bytes) {
    failed_bytes_tx_.fetch_add(bytes, std::memory_order_relaxed);
  }

  void AddFailedRxBytes(size_t bytes) {
    failed_bytes_rx_.fetch_add(bytes, std::memory_order_relaxed);
  }

  void SetMaxByteRate(uint64_t max_rate) {
    max_bytes_per_second_.store(max_rate, std::memory_order_relaxed);
  }

  void MeasureThroughput(zx::duration elapsed);

  void RecordEvent(const std::string& event_name);

  // Captures an independent cumulative transfer stats snapshot at current boot time.
  // Note: These values represent the totals and max rate since boot, not the
  // incremental traffic and max rate since the last snapshot.
  // Called automatically by RecordEvent(), or directly by drivers at any point.
  void SnapshotTransferStats();

  uint64_t total_bytes_tx_val() const { return total_bytes_tx_.load(std::memory_order_relaxed); }
  uint64_t total_bytes_rx_val() const { return total_bytes_rx_.load(std::memory_order_relaxed); }

  uint64_t failed_bytes_tx_val() const { return failed_bytes_tx_.load(std::memory_order_relaxed); }

  uint64_t failed_bytes_rx_val() const { return failed_bytes_rx_.load(std::memory_order_relaxed); }

 private:
  struct EventLogEntry {
    zx_instant_boot_t timestamp;
    std::string event;
  };

  struct TransferSnapshotEntry {
    zx_instant_boot_t timestamp;
    TransferStats stats;
  };

  void SnapshotTransferStatsLocked(zx_instant_boot_t raw_time) __TA_REQUIRES(lock_);

  mutable std::mutex lock_;
  inspect::LazyNode lazy_node_;

  std::atomic<uint64_t> tx_pending_requests_{0};
  std::atomic<uint64_t> rx_pending_requests_{0};
  std::atomic<uint64_t> rx_pending_processing_{0};
  std::atomic<uint64_t> total_bytes_tx_{0};
  std::atomic<uint64_t> total_bytes_rx_{0};
  std::atomic<uint64_t> failed_bytes_tx_{0};
  std::atomic<uint64_t> failed_bytes_rx_{0};
  std::atomic<uint64_t> max_bytes_per_second_{0};

  std::deque<EventLogEntry> event_history_ __TA_GUARDED(lock_);
  size_t event_history_capacity_ = kDefaultEventCapacity;
  size_t event_history_index_ __TA_GUARDED(lock_) = 0;

  std::deque<TransferSnapshotEntry> transfer_snapshots_ __TA_GUARDED(lock_);
  size_t transfer_snapshot_capacity_ = kDefaultTransferCapacity;
  size_t transfer_snapshots_index_ __TA_GUARDED(lock_) = 0;

  std::atomic<uint64_t> last_total_bytes_val_{0};
};

// Class to manage common DCI/peripheral controller Inspect metrics.
class DciInspect {
 public:
  // Default capacity for control transfer history. Set to 30, which leaves comfortable
  // headroom over the 20 control transfers generated during a typical host enumeration
  // and USB function initialization sequence.
  static constexpr size_t kDefaultControlTransferCapacity = 30;
  // Default capacity for event log history. Set to 40, which is enough to hold ~4
  // complete cable plug/unplug and endpoint configuration cycles (~10 events per cycle).
  static constexpr size_t kDefaultEventCapacity = 40;

  DciInspect() = default;

  struct ControlTransferInfo {
    uint8_t request_type;
    uint8_t request;
    uint16_t value;
    uint16_t index;
    // Transfer length in bytes
    uint16_t length;
    zx_status_t status;
    // Actual transferred length in bytes
    size_t actual_length;
  };

  void Init(inspect::Node& parent, const std::string& name,
            size_t ctrl_capacity = kDefaultControlTransferCapacity,
            size_t event_capacity = kDefaultEventCapacity);

  void UpdateState(const std::string& state);
  void UpdateConnectionStatus(bool connected, usb_speed_t speed);
  void UpdateUsbMode(usb_mode_t usb_mode);

  void RecordEvent(const std::string& event_name);
  void RecordControlTransfer(const ControlTransferInfo& info);

 private:
  struct EventLogEntry {
    zx_instant_boot_t timestamp;
    std::string event;
  };

  struct ControlTransferLogEntry {
    zx_instant_boot_t timestamp;
    uint8_t bm_request_type;
    uint8_t b_request;
    uint16_t w_value;
    uint16_t w_index;
    uint16_t w_length;
    zx_status_t status;
    size_t response_length;
  };

  mutable std::mutex lock_;
  inspect::LazyNode lazy_node_;

  std::string state_ __TA_GUARDED(lock_) = "unknown";
  bool connected_ __TA_GUARDED(lock_) = false;
  std::string speed_ __TA_GUARDED(lock_) = "undefined";
  std::string usb_mode_ __TA_GUARDED(lock_) = "unknown";

  std::deque<EventLogEntry> event_history_ __TA_GUARDED(lock_);
  size_t event_history_capacity_ = kDefaultEventCapacity;
  size_t event_history_index_ __TA_GUARDED(lock_) = 0;

  std::deque<ControlTransferLogEntry> control_history_ __TA_GUARDED(lock_);
  size_t control_history_capacity_ = kDefaultControlTransferCapacity;
  size_t control_history_index_ __TA_GUARDED(lock_) = 0;
};

// Class to manage common USB function metrics (configuration, interface descriptors, events).
class FunctionInspect {
 public:
  // Default capacity for event log history. Set to 20 to capture multiple occurrences
  // of function-specific transitions (such as configuration and interface changes).
  static constexpr size_t kDefaultEventCapacity = 20;

  FunctionInspect() = default;

  void Init(inspect::Node& parent, const std::string& name, uint8_t index,
            size_t event_capacity = kDefaultEventCapacity);

  void UpdateConfiguration(uint8_t config, bool configured);
  void UpdateDescriptorInfo(uint8_t intf_class, uint8_t subclass, uint8_t protocol);
  void SetDescriptors(std::vector<uint8_t> descriptors);
  void RecordEvent(const std::string& event_name);

 private:
  struct EventLogEntry {
    zx_instant_boot_t timestamp;
    std::string event;
  };

  inspect::LazyNode lazy_node_;

  uint8_t index_ __TA_GUARDED(lock_) = 0;
  uint8_t configuration_ __TA_GUARDED(lock_) = 0;
  bool configured_ __TA_GUARDED(lock_) = false;
  uint8_t interface_class_ __TA_GUARDED(lock_) = 0;
  uint8_t interface_subclass_ __TA_GUARDED(lock_) = 0;
  uint8_t interface_protocol_ __TA_GUARDED(lock_) = 0;
  std::vector<uint8_t> descriptors_ __TA_GUARDED(lock_);

  mutable std::mutex lock_;
  std::deque<EventLogEntry> event_history_ __TA_GUARDED(lock_);
  size_t event_history_capacity_ = kDefaultEventCapacity;
  size_t event_history_index_ __TA_GUARDED(lock_) = 0;
};

// Periodically triggers a measurement callback at a fixed interval to calculate throughput.
class ThroughputTracker {
 public:
  using MeasureCallback = fit::function<void(zx::duration)>;

  ThroughputTracker(async_dispatcher_t* dispatcher, MeasureCallback callback,
                    zx::duration interval = zx::sec(60))
      : dispatcher_(dispatcher), callback_(std::move(callback)), interval_(interval) {
    ZX_ASSERT(interval_ > zx::sec(0));
  }

  // Starts the periodic throughput timer.
  void Start() {
    last_time_ = zx::clock::get_monotonic();
    Post();
  }

  // Stops the periodic throughput timer.
  void Stop() { task_.Cancel(); }

  // Manually triggers the measurement callback with a fixed elapsed duration.
  void MeasureForTesting(zx::duration delta) { callback_(delta); }

 private:
  void Post() { task_.PostDelayed(dispatcher_, interval_); }

  void Run() {
    zx::time now = zx::clock::get_monotonic();
    zx::duration delta = now - last_time_;
    last_time_ = now;
    callback_(delta);
    Post();
  }

  async_dispatcher_t* dispatcher_;
  MeasureCallback callback_;
  zx::duration interval_;
  zx::time last_time_;
  async::TaskClosure task_{[this]() { Run(); }};
};

}  // namespace usb_inspect

#endif  // SRC_DEVICES_USB_LIB_USB_INSPECT_INCLUDE_USB_INSPECT_USB_INSPECT_H_
