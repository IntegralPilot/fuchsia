// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_HID_DEVICE_REPORT_READER_H_
#define SRC_UI_INPUT_DRIVERS_HID_DEVICE_REPORT_READER_H_

#include <fidl/fuchsia.hardware.input/cpp/wire.h>

#include <optional>

#include <fbl/ring_buffer.h>

namespace hid_driver {

class HidDevice;

// DeviceReportsReader is not thread safe and should be only used in one dispatcher.
class DeviceReportsReader : public fidl::WireServer<fuchsia_hardware_input::DeviceReportsReader> {
 public:
  // The pointer to `base` must stay alive for as long as DeviceReportsReader
  // is alive.
  explicit DeviceReportsReader(HidDevice* base) : base_(base) {}

  ~DeviceReportsReader() override {
    // The lock has to be grabbed to synchronize with any clients who are currently
    // trying access DeviceReportsReader.
    if (waiting_read_) {
      waiting_read_->ReplyError(ZX_ERR_PEER_CLOSED);
      waiting_read_.reset();
    }
  }

  void ReadReports(ReadReportsCompleter::Sync& completer) override;
  zx_status_t WriteToFifo(const uint8_t* report, size_t report_len, zx_time_t time);
  void SetWakeLease(const zx::eventpair& wake_lease);

 private:
  zx_status_t SendReports();
  zx_status_t ReadReportFromFifo(uint8_t* buf, size_t buf_size, zx_time_t* time,
                                 size_t* out_report_size);
  static constexpr size_t kDataFifoSize = 4096;
  fbl::RingBuffer<uint8_t, kDataFifoSize> data_fifo_;
  fbl::RingBuffer<zx_time_t, fuchsia_hardware_input::wire::kMaxReportsCount> timestamps_;

  zx::eventpair wake_lease_;

  std::optional<ReadReportsCompleter::Async> waiting_read_;
  uint32_t trace_id_ = 0;
  uint32_t reports_written_ = 0;
  // The number of reports sent out to the client.
  uint32_t reports_sent_ = 0;
  HidDevice* const base_;
};

}  // namespace hid_driver

#endif  // SRC_UI_INPUT_DRIVERS_HID_DEVICE_REPORT_READER_H_
