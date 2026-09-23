// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "hid-instance.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/hid/boot.h>
#include <lib/trace/event.h>

#include "hid.h"

namespace hid_driver {

namespace fhidbus = fuchsia_hardware_hidbus;

namespace {

constexpr uint32_t kHidFlagsDead = (1 << 0);
constexpr uint32_t kHidFlagsWriteFailed = (1 << 1);

constexpr uint64_t hid_report_trace_id(uint32_t instance_id, uint64_t report_id) {
  return (report_id << 32) | instance_id;
}

}  // namespace

HidInstance::HidInstance(HidDevice* base, zx::event fifo_event,
                         fidl::ServerEnd<fuchsia_hardware_input::Device> session)
    : base_(base),
      fifo_event_(std::move(fifo_event)),
      binding_(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(session), this,
               [](HidInstance* instance, fidl::UnbindInfo info) {
                 if (!instance) {
                   return;
                 }
                 instance->CloseInstance();
                 instance->base_->RemoveInstance(*instance);
               }) {
  zx_hid_fifo_init(&fifo_);
}

void HidInstance::SetReadable() { fifo_event_.signal(0, ZX_USER_SIGNAL_0); }

void HidInstance::ClearReadable() { fifo_event_.signal(ZX_USER_SIGNAL_0, 0); }

zx_status_t HidInstance::ReadReportFromFifo(uint8_t* buf, size_t buf_size, zx_time_t* time,
                                            size_t* report_size) {
  uint8_t rpt_id;
  if (zx_hid_fifo_peek(&fifo_, &rpt_id) <= 0) {
    return ZX_ERR_SHOULD_WAIT;
  }

  size_t xfer = base_->GetReportSizeById(rpt_id, fhidbus::ReportType::kInput);
  if (xfer == 0) {
    fdf::error("error reading hid device: unknown report id ({})!", rpt_id);
    return ZX_ERR_BAD_STATE;
  }

  // Check if we have enough room left in the buffer.
  if (xfer > buf_size) {
    return ZX_ERR_BUFFER_TOO_SMALL;
  }

  ssize_t rpt_size = zx_hid_fifo_read(&fifo_, buf, xfer);
  if (rpt_size <= 0) {
    // Something went wrong. The fifo should always contain full reports in it.
    return ZX_ERR_INTERNAL;
  }

  size_t left = zx_hid_fifo_size(&fifo_);
  if (left == 0) {
    ClearReadable();
  }

  *report_size = rpt_size;

  *time = timestamps_.front();
  timestamps_.pop();

  reports_sent_ += 1;
  TRACE_FLOW_STEP("input", "hid_report", hid_report_trace_id(trace_id_, reports_sent_));

  return ZX_OK;
}

void HidInstance::ReadReport(ReadReportCompleter::Sync& completer) {
  TRACE_DURATION("input", "HID ReadReport Instance", "bytes_in_fifo", zx_hid_fifo_size(&fifo_));

  if (flags_ & kHidFlagsDead) {
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  std::array<uint8_t, fhidbus::wire::kMaxReportData> buf;
  zx_time_t time = 0;
  size_t report_size = 0;
  zx_status_t status = ReadReportFromFifo(buf.data(), buf.size(), &time, &report_size);
  if (status != ZX_OK) {
    return completer.ReplyError(status);
  }

  auto buf_view = fidl::VectorView<uint8_t>::FromExternal(buf.data(), report_size);
  fidl::Arena arena;
  auto report = fhidbus::wire::Report::Builder(arena).buf(buf_view).timestamp(time);
  completer.ReplySuccess(report.Build());
}

void HidInstance::ReadReports(ReadReportsCompleter::Sync& completer) {
  TRACE_DURATION("input", "HID GetReports Instance", "bytes_in_fifo", zx_hid_fifo_size(&fifo_));

  if (flags_ & kHidFlagsDead) {
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  std::array<uint8_t, fhidbus::wire::kMaxReportData> buf;
  size_t buf_index = 0;
  zx_status_t status = ZX_OK;
  zx_time_t time;

  while (status == ZX_OK) {
    size_t report_size;
    status =
        ReadReportFromFifo(buf.data() + buf_index, buf.size() - buf_index, &time, &report_size);
    if (status == ZX_OK) {
      buf_index += report_size;
    }
  }

  if ((buf_index > 0) && ((status == ZX_ERR_BUFFER_TOO_SMALL) || (status == ZX_ERR_SHOULD_WAIT))) {
    status = ZX_OK;
  }

  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  auto buf_view = fidl::VectorView<uint8_t>::FromExternal(buf.data(), buf_index);
  completer.ReplySuccess(buf_view);
}

void HidInstance::GetReportsEvent(GetReportsEventCompleter::Sync& completer) {
  zx::event new_event;
  zx_status_t status = fifo_event_.duplicate(ZX_RIGHTS_BASIC, &new_event);
  if (status != ZX_OK) {
    return completer.ReplyError(status);
  }

  completer.ReplySuccess(std::move(new_event));
}

void HidInstance::Query(QueryCompleter::Sync& completer) {
  fidl::Arena arena;
  completer.ReplySuccess(fidl::ToWire(arena, base_->GetHidInfo()));
}

void HidInstance::GetReportDesc(GetReportDescCompleter::Sync& completer) {
  size_t desc_size = base_->GetReportDescLen();
  const uint8_t* desc = base_->GetReportDesc();

  // (BUG 35762) Const cast is necessary until simple data types are generated
  // as const in LLCPP. We know the data is not modified.
  completer.Reply(fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(desc), desc_size));
}

void HidInstance::GetReport(GetReportRequestView request, GetReportCompleter::Sync& completer) {
  size_t needed = base_->GetReportSizeById(request->id, request->type);
  if (needed == 0) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }
  if (needed > fhidbus::kMaxReportLen) {
    fdf::error("hid: GetReport: Report size {:#x} larger than max size {:#x}", needed,
               fhidbus::kMaxReportLen);
    completer.ReplyError(ZX_ERR_INTERNAL);
    return;
  }

  auto& client = base_->GetHidbusProtocol();
  auto result = client.sync()->GetReport(request->type, request->id, needed);
  if (!result.ok()) {
    fdf::error("FIDL transport failed on GetReport(): {}", result.error().FormatDescription());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("HID device failed to get report: {}", result->error_value());
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess(result.value()->data);
}

void HidInstance::SetReport(SetReportRequestView request, SetReportCompleter::Sync& completer) {
  size_t needed = base_->GetReportSizeById(request->id, request->type);
  if (needed != request->report.size()) {
    fdf::error("Tried to set Report {} (size {:#x}) with {:#x} bytes", request->id, needed,
               request->report.size());
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  auto& client = base_->GetHidbusProtocol();
  auto result = client.sync()->SetReport(request->type, request->id, request->report);
  if (!result.ok()) {
    fdf::error("FIDL transport failed on SetReport(): {}", result.error().FormatDescription());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("HID device failed to set report: {}", result->error_value());
    completer.ReplyError(result->error_value());
    return;
  }
  completer.ReplySuccess();
}

void HidInstance::GetDeviceReportsReader(GetDeviceReportsReaderRequestView request,
                                         GetDeviceReportsReaderCompleter::Sync& completer) {
  readers_binding_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                              std::move(request->reader), &readers_.emplace_back(base_),
                              fidl::kIgnoreBindingClosure);
  completer.ReplySuccess();
}

void HidInstance::SetTraceId(SetTraceIdRequestView request, SetTraceIdCompleter::Sync& completer) {
  trace_id_ = request->id;
}

void HidInstance::CloseInstance() {
  flags_ |= kHidFlagsDead;
  SetReadable();
}

void HidInstance::WriteToFifo(const uint8_t* report, size_t report_len, zx_time_t time) {
  auto iter = readers_.begin();
  while (iter != readers_.end()) {
    if ((*iter).WriteToFifo(report, report_len, time) != ZX_OK) {
      iter = readers_.erase(iter);
    } else {
      iter++;
    }
  }

  if (timestamps_.full()) {
    flags_ |= kHidFlagsWriteFailed;
    return;
  }

  bool was_empty = zx_hid_fifo_size(&fifo_) == 0;

  ssize_t wrote = zx_hid_fifo_write(&fifo_, report, report_len);
  if (wrote <= 0) {
    if (!(flags_ & kHidFlagsWriteFailed)) {
      fdf::error("Could not write to hid fifo (ret={})", wrote);
      flags_ |= kHidFlagsWriteFailed;
    }
    return;
  }

  timestamps_.push(time);

  TRACE_FLOW_BEGIN("input", "hid_report", hid_report_trace_id(trace_id_, reports_written_));
  ++reports_written_;
  flags_ &= ~kHidFlagsWriteFailed;
  if (was_empty) {
    SetReadable();
  }
}

void HidInstance::SetWakeLease(const zx::eventpair& wake_lease) {
  for (auto& reader : readers_) {
    reader.SetWakeLease(wake_lease);
  }
}

}  // namespace hid_driver
