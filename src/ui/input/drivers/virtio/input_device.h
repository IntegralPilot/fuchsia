// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_DEVICE_H_
#define SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_DEVICE_H_

#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/zx/clock.h>
#include <zircon/types.h>

#include <virtio/input.h>

#include "src/ui/input/lib/input-report-reader/reader.h"

namespace virtio {

class HidDeviceBase {
 public:
  virtual ~HidDeviceBase() = default;

  // Gets the HID Report Descriptor for this device. The memory for the descriptor
  // is dynamically allocated and placed in |data| with length |len|.
  virtual fuchsia_input_report::wire::DeviceDescriptor GetDescriptor(fidl::AnyArena& allocator) = 0;

  // Process a virtio event for this device and update the private HID
  // report accordingly.
  virtual void ReceiveEvent(virtio_input_event_t* event) = 0;

  virtual void GetInputReportsReader(
      async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_input_report::InputReportsReader> reader) = 0;
  virtual zx_status_t GetInputReportsReaderV2(
      async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_input_report::InputReportsReaderV2> reader,
      uint16_t max_unacknowledged_reports) = 0;
  virtual zx::time SendReportToAllReaders() = 0;
};

// Each HidDevice is responsible for taking virtio events and translating them
// into HID events. This class should be inherited once for each type of input
// device that should be supported (e.g: mice, keyboards, touchscreens).
template <typename ReportType>
class HidDevice : public HidDeviceBase {
 public:
  void GetInputReportsReader(
      async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_input_report::InputReportsReader> reader) override {
    ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
  }

  zx_status_t GetInputReportsReaderV2(
      async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_input_report::InputReportsReaderV2> reader,
      uint16_t max_unacknowledged_reports) override {
    return readers_.CreateReaderV2(dispatcher, std::move(reader), max_unacknowledged_reports);
  }

  zx::time SendReportToAllReaders() override {
    report_.event_time = zx::clock::get_monotonic();
    readers_.SendReportToAllReaders(report_);
    return report_.event_time;
  }

 protected:
  ReportType report_;

 private:
  input_report_reader::InputReportReaderManager<ReportType> readers_;
};

}  // namespace virtio

#endif  // SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_DEVICE_H_
