// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_H_
#define SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_H_

#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/io-buffer.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/virtio/device.h>
#include <lib/virtio/ring.h>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>
#include <fbl/mutex.h>
#include <virtio/input.h>

#include "src/ui/input/drivers/virtio/input_device.h"

namespace virtio {

class InputDevice
    : public Device,
      public ddk::Device<InputDevice, ddk::Messageable<fuchsia_input_report::InputDevice>::Mixin>,
      public ddk::EmptyProtocol<ZX_PROTOCOL_INPUTREPORT> {
 public:
  // Max unacknowledged report count allowed for 1/2 second.
  // VirtIO input devices are event-driven via VirtQueue interrupt notifications.
  // Assuming a standard max event rate of 120 Hz yields 60 reports per 1/2 second (120 Hz / 2).
  static constexpr uint16_t kMaxReportsPerHalfSecond = 60;

  InputDevice(zx_device_t* device, zx::bti bti, std::unique_ptr<Backend> backend);
  virtual ~InputDevice();

  zx_status_t Init() override;

  void IrqRingUpdate() override;
  void IrqConfigChange() override;
  const char* tag() const override { return "virtio-input"; }

  // DDK driver hooks
  void DdkRelease();

  // fuchsia_input_report::InputDevice required methods
  void GetInputReportsReader(GetInputReportsReaderRequestView request,
                             GetInputReportsReaderCompleter::Sync& completer) override {
    ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
  }
  void GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                               GetInputReportsReaderV2Completer::Sync& completer) override;
  void GetDescriptor(GetDescriptorCompleter::Sync& completer) override {
    fidl::Arena<kFeatureAndDescriptorBufferSize> allocator;
    completer.Reply(hid_device_->GetDescriptor(allocator));
  }
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
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputDevice> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    zxlogf(WARNING, "Unexpected fidl method invoked: %ld", metadata.method_ordinal);
  }

 private:
  static constexpr size_t kFeatureAndDescriptorBufferSize = 512;

  static constexpr size_t kEventCount = 64;
  // We don't currently send status events to the device, so we use the
  // smallest value that is a power of 2, as required by virtio::Ring::Init();
  static constexpr size_t kStatusCount = 2;

  void ReceiveEvent(virtio_input_event_t* event);

  void SelectConfig(uint8_t select, uint8_t subsel);

  virtio_input_config_t config_;

  io_buffer_t eventq_buffers_[kEventCount];

  io_buffer_t statusq_buffers_[kStatusCount];

  fbl::Mutex lock_;

  std::unique_ptr<HidDeviceBase> hid_device_;

  // The input device's event virtqueue, or `eventq`.
  //
  // Defined in the VIRTIO spec Section 5.8.2 "Input Device" > "Virtqueues".
  Ring eventq_vring_{this};

  // The input device's status virtqueue, or `statusq`.
  //
  // Defined in the VIRTIO spec Section 5.8.2 "Input Device" > "Virtqueues".
  Ring statusq_vring_{this};

  inspect::Inspector inspector_;
  inspect::Node metrics_root_;
  inspect::UintProperty total_report_count_;
  inspect::UintProperty last_event_timestamp_;
};

}  // namespace virtio

#endif  // SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_H_
