// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_PC_PS2_DEVICE_H_
#define SRC_UI_INPUT_DRIVERS_PC_PS2_DEVICE_H_

#include <fidl/fuchsia.hardware.input/cpp/wire.h>
#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/irq.h>
#include <lib/ddk/debug.h>
#include <lib/hid/boot.h>
#include <lib/zx/interrupt.h>
#include <zircon/assert.h>

#include <condition_variable>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>
#include <ddktl/unbind-txn.h>

#include "src/ui/input/drivers/pc-ps2/controller.h"
#include "src/ui/input/lib/input-report-reader/reader.h"

namespace i8042 {

struct PS2KbdInputReport {
  size_t num_pressed_keys_3 = 0;
  fuchsia_input::wire::Key pressed_keys_3[fuchsia_input_report::wire::kKeyboardMaxNumKeys];

  void Reset() { num_pressed_keys_3 = 0; }
};

struct PS2MouseInputReport {
  uint8_t buttons;
  int8_t rel_x;
  int8_t rel_y;

  void Reset() {
    buttons = 0;
    rel_x = 0;
    rel_y = 0;
  }
};

struct PS2InputReport {
  zx::time event_time;
  fuchsia_hardware_hidbus::HidBootProtocol type;
  std::variant<PS2KbdInputReport, PS2MouseInputReport> report;

  void ToFidlInputReport(
      fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
      fidl::AnyArena& allocator) const;
  void Reset() {
    event_time = {};
    type = fuchsia_hardware_hidbus::HidBootProtocol::kNone;
    if (std::holds_alternative<PS2KbdInputReport>(report)) {
      std::get<PS2KbdInputReport>(report).Reset();
    } else if (std::holds_alternative<PS2MouseInputReport>(report)) {
      std::get<PS2MouseInputReport>(report).Reset();
    }
  }
};

enum ModStatus {
  kSet = 1,
  kExists = 2,
  kRollover = 3,
};

enum KeyStatus {
  kKeyAdded = 1,
  kKeyExists = 2,
  kKeyRollover = 3,
  kKeyRemoved = 4,
  kKeyNotFound = 5,
};
constexpr uint8_t kAck = 0xfa;

class I8042Device;
using DeviceType = ddk::Device<I8042Device, ddk::Suspendable, ddk::Unbindable,
                               ddk::Messageable<fuchsia_input_report::InputDevice>::Mixin>;
class I8042Device : public DeviceType, public ddk::EmptyProtocol<ZX_PROTOCOL_INPUTREPORT> {
 public:
  // Max unacknowledged report count allowed for 1/2 second.
  // PS/2 mouse default sampling rate is 100 Hz, yielding 50 reports per 1/2 second (100 Hz / 2).
  static constexpr uint16_t kMaxReportsPerHalfSecond = 50;

  explicit I8042Device(Controller* parent, async_dispatcher_t* dispatcher, Port port)
      : DeviceType(parent->zxdev()),
        dispatcher_(dispatcher),
        controller_(parent),
        port_(port),
        report_({
            .event_time = {},
            .type = fuchsia_hardware_hidbus::HidBootProtocol::kNone,
        }) {}

  static zx_status_t Bind(Controller* parent, async_dispatcher_t* dispatcher, Port port);
  zx_status_t Bind();

  void DdkRelease() {
    Shutdown();
    delete this;
  }
  void DdkSuspend(ddk::SuspendTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn);

  void GetInputReportsReader(GetInputReportsReaderRequestView request,
                             GetInputReportsReaderCompleter::Sync& completer) override;
  void GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                               GetInputReportsReaderV2Completer::Sync& completer) override;
  void GetDescriptor(GetDescriptorCompleter::Sync& completer) override;
  void SendOutputReport(SendOutputReportRequestView request,
                        SendOutputReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void GetFeatureReport(GetFeatureReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void SetFeatureReport(SetFeatureReportRequestView request,
                        SetFeatureReportCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void GetInputReport(GetInputReportRequestView request,
                      GetInputReportCompleter::Sync& completer) override {
    ZX_DEBUG_ASSERT(false);
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputDevice> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    zxlogf(WARNING, "Unexpected fidl method invoked: %ld", metadata.method_ordinal);
  }

#ifdef PS2_TEST
  zx_status_t WaitForNextReader(zx::duration timeout) {
    zx_status_t status = sync_completion_wait(&next_reader_wait_, timeout.get());
    if (status == ZX_OK) {
      sync_completion_reset(&next_reader_wait_);
    }
    return status;
  }
#endif

 private:
  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);

  void Shutdown();

  async_dispatcher_t* dispatcher_;

  Controller* controller_;
  Port port_;
  fuchsia_hardware_hidbus::wire::HidBootProtocol protocol_;
  zx::interrupt irq_;
  async::IrqMethod<I8042Device, &I8042Device::HandleIrq> irq_handler_{this};

  std::mutex hid_lock_;
  input_report_reader::InputReportReaderManager<PS2InputReport> input_report_readers_
      __TA_GUARDED(hid_lock_);
#ifdef PS2_TEST
  sync_completion_t next_reader_wait_;
#endif

  uint8_t last_code_ = 0;
  PS2InputReport report_;
  PS2KbdInputReport& keyboard_report() {
    ZX_ASSERT(std::holds_alternative<PS2KbdInputReport>(report_.report));
    return std::get<PS2KbdInputReport>(report_.report);
  }
  PS2MouseInputReport& mouse_report() {
    ZX_ASSERT(std::holds_alternative<PS2MouseInputReport>(report_.report));
    return std::get<PS2MouseInputReport>(report_.report);
  }

  zx::result<fuchsia_hardware_hidbus::wire::HidBootProtocol> Identify();
  // Keyboard input
  void ProcessScancode(zx::time timestamp, uint8_t code);
  KeyStatus AddKey(fuchsia_input::wire::Key key);
  KeyStatus RemoveKey(fuchsia_input::wire::Key key);
  // Mouse input
  void ProcessMouse(zx::time timestamp, uint8_t code);
};

}  // namespace i8042

#endif  // SRC_UI_INPUT_DRIVERS_PC_PS2_DEVICE_H_
