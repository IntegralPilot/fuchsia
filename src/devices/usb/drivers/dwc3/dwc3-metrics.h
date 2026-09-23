// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_METRICS_H_
#define SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_METRICS_H_

#include <lib/inspect/component/cpp/component.h>
#include <zircon/types.h>

#include <deque>
#include <string>

#include "src/devices/usb/drivers/dwc3/dwc3-types.h"

namespace fdf {
class MmioBuffer;
}

namespace dwc3 {
class Dwc3;

enum class MetricEventType : uint32_t {
  kDevtDisconnect = DEVT_DISCONNECT,
  kDevtUsbReset = DEVT_USB_RESET,
  kDevtConnectionDone = DEVT_CONNECTION_DONE,
  kDevtLinkStateChange = DEVT_LINK_STATE_CHANGE,
  kDevtRemoteWakeup = DEVT_REMOTE_WAKEUP,
  kDevtHibernateRequest = DEVT_HIBERNATE_REQUEST,
  kDevtSuspendEntry = DEVT_SUSPEND_ENTRY,
  kDevtSof = DEVT_SOF,
  kDevtUnknown8 = 8,  // Unused event type
  kDevtErraticError = DEVT_ERRATIC_ERROR,
  kDevtCommandComplete = DEVT_COMMAND_COMPLETE,
  kDevtEventBufOverflow = DEVT_EVENT_BUF_OVERFLOW,
  kDevtVendorTestLmp = DEVT_VENDOR_TEST_LMP,
  kDevtStoppedDisconnect = DEVT_STOPPED_DISCONNECT,
  kDevtL1ResumeDetect = DEVT_L1_RESUME_DETECT,
  kDevtLdmResponse = DEVT_LDM_RESPONSE,
  kDevtUnknown,
  kDevtNumEventTypes,  // Keeps track of the number of event types
};

struct EventLogEntry {
  zx_time_t timestamp;
  std::string message;
};

class Dwc3Metrics {
 public:
  void Init();
  void IncrementEventCount(uint32_t type) {
    type = std::min(type, static_cast<uint32_t>(MetricEventType::kDevtUnknown));
    event_counts_[type]++;
  }
  void RecordEvent(std::string message);
  void UpdateMaxEventBatch(uint32_t batch_size) {
    if (batch_size > max_event_batch_size_) {
      max_event_batch_size_ = batch_size;
    }
  }
  inspect::Inspector RecordMetrics(fdf::MmioBuffer* mmio, Dwc3* dwc3);

  friend struct std::formatter<MetricEventType>;

 private:
  zx_time_t time_start_;
  uint64_t event_counts_[static_cast<uint32_t>(MetricEventType::kDevtNumEventTypes)];
  uint32_t max_event_batch_size_{0};

  std::deque<EventLogEntry> event_history_;
  static constexpr size_t kEventHistoryCapacity = 50;
};

}  // namespace dwc3

// Needs to be in the root scope.
template <>
struct std::formatter<dwc3::MetricEventType> : std::formatter<std::string> {
  auto format(const dwc3::MetricEventType& type, format_context& ctx) const {
    std::string fmt;
    switch (type) {
      case dwc3::MetricEventType::kDevtDisconnect:
        fmt = "DEVT_DISCONNECT";
        break;
      case dwc3::MetricEventType::kDevtUsbReset:
        fmt = "DEVT_USB_RESET";
        break;
      case dwc3::MetricEventType::kDevtConnectionDone:
        fmt = "DEVT_CONNECTION_DONE";
        break;
      case dwc3::MetricEventType::kDevtLinkStateChange:
        fmt = "DEVT_LINK_STATE_CHANGE";
        break;
      case dwc3::MetricEventType::kDevtRemoteWakeup:
        fmt = "DEVT_REMOTE_WAKEUP";
        break;
      case dwc3::MetricEventType::kDevtHibernateRequest:
        fmt = "DEVT_HIBERNATE_REQUEST";
        break;
      case dwc3::MetricEventType::kDevtSuspendEntry:
        fmt = "DEVT_SUSPEND_ENTRY";
        break;
      case dwc3::MetricEventType::kDevtSof:
        fmt = "DEVT_SOF";
        break;
      case dwc3::MetricEventType::kDevtUnknown8:
        fmt = "DEVT_UNKNOWN_8";
        break;
      case dwc3::MetricEventType::kDevtErraticError:
        fmt = "DEVT_ERRATIC_ERROR";
        break;
      case dwc3::MetricEventType::kDevtCommandComplete:
        fmt = "DEVT_COMMAND_COMPLETE";
        break;
      case dwc3::MetricEventType::kDevtEventBufOverflow:
        fmt = "DEVT_EVENT_BUF_OVERFLOW";
        break;
      case dwc3::MetricEventType::kDevtVendorTestLmp:
        fmt = "DEVT_VENDOR_TEST_LMP";
        break;
      case dwc3::MetricEventType::kDevtStoppedDisconnect:
        fmt = "DEVT_STOPPED_DISCONNECT";
        break;
      case dwc3::MetricEventType::kDevtL1ResumeDetect:
        fmt = "DEVT_L1_RESUME_DETECT";
        break;
      case dwc3::MetricEventType::kDevtLdmResponse:
        fmt = "DEVT_LDM_RESPONSE";
        break;
      case dwc3::MetricEventType::kDevtUnknown:
      case dwc3::MetricEventType::kDevtNumEventTypes:
        fmt = "UNKNOWN";
        break;
    }
    return std::formatter<std::string>::format(fmt, ctx);
  }
};

#endif  // SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_METRICS_H_
