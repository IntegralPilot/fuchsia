// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_HID_HID_INSTANCE_H_
#define SRC_UI_INPUT_DRIVERS_HID_HID_INSTANCE_H_

#include <fidl/fuchsia.hardware.input/cpp/wire.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/eventpair.h>

#include <list>

#include <fbl/intrusive_double_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ring_buffer.h>

#include "device-report-reader.h"
#include "hid-fifo.h"

namespace hid_driver {

class HidDevice;

class HidInstance : public fidl::WireServer<fuchsia_hardware_input::Device>,
                    public fbl::DoublyLinkedListable<fbl::RefPtr<HidInstance>>,
                    public fbl::RefCounted<HidInstance> {
 public:
  HidInstance(HidDevice* base, zx::event fifo_event,
              fidl::ServerEnd<fuchsia_hardware_input::Device> session);
  ~HidInstance() override = default;

  void Query(QueryCompleter::Sync& completer) override;
  void GetReportDesc(GetReportDescCompleter::Sync& completer) override;
  void GetReportsEvent(GetReportsEventCompleter::Sync& completer) override;
  void GetReport(GetReportRequestView request, GetReportCompleter::Sync& completer) override;
  void SetReport(SetReportRequestView request, SetReportCompleter::Sync& completer) override;
  void SetTraceId(SetTraceIdRequestView request, SetTraceIdCompleter::Sync& completer) override;
  void ReadReports(ReadReportsCompleter::Sync& completer) override;
  void ReadReport(ReadReportCompleter::Sync& completer) override;
  void GetDeviceReportsReader(GetDeviceReportsReaderRequestView request,
                              GetDeviceReportsReaderCompleter::Sync& completer) override;

  void CloseInstance();
  void WriteToFifo(const uint8_t* report, size_t report_len, zx_time_t time);
  void SetWakeLease(const zx::eventpair& wake_lease);

 private:
  void SetReadable();
  void ClearReadable();
  zx_status_t ReadReportFromFifo(uint8_t* buf, size_t buf_size, zx_time_t* time,
                                 size_t* report_size);
  HidDevice* base_;

  uint32_t flags_ = 0;

  zx_hid_fifo_t fifo_ = {};
  static const size_t kMaxNumReports = 50;
  fbl::RingBuffer<zx_time_t, kMaxNumReports> timestamps_;

  zx::event fifo_event_;

  uint32_t trace_id_ = 0;
  uint32_t reports_written_ = 0;
  // The number of reports sent out to the client.
  uint32_t reports_sent_ = 0;

  std::list<DeviceReportsReader> readers_;

  fidl::ServerBinding<fuchsia_hardware_input::Device> binding_;
  fidl::ServerBindingGroup<fuchsia_hardware_input::DeviceReportsReader> readers_binding_;
};

}  // namespace hid_driver

#endif  // SRC_UI_INPUT_DRIVERS_HID_HID_INSTANCE_H_
