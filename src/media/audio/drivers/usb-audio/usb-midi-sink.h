// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_USB_AUDIO_USB_MIDI_SINK_H_
#define SRC_MEDIA_AUDIO_DRIVERS_USB_AUDIO_USB_MIDI_SINK_H_

#include <fidl/fuchsia.hardware.midi/cpp/wire.h>
#include <lib/sync/completion.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>
#include <fbl/mutex.h>
#include <usb/request-cpp.h>
#include <usb/usb.h>

namespace audio {
namespace usb {

class UsbMidiSink;
using UsbMidiSinkBase = ddk::Device<UsbMidiSink, ddk::Unbindable,
                                    ddk::Messageable<fuchsia_hardware_midi::Controller>::Mixin>;

class UsbMidiSink : public UsbMidiSinkBase,
                    public ddk::EmptyProtocol<ZX_PROTOCOL_MIDI>,
                    public fidl::WireServer<fuchsia_hardware_midi::Device> {
 public:
  using UsbDevice = ::usb::UsbDevice;
  using UsbRequest = ::usb::Request<>;
  using UsbRequestQueue = ::usb::RequestQueue<>;

  UsbMidiSink(zx_device_t* parent, const UsbDevice& usb, size_t parent_req_size)
      : UsbMidiSinkBase(parent), usb_(usb), parent_req_size_(parent_req_size) {}

  static zx_status_t Create(zx_device_t* parent, const UsbDevice& usb, int index,
                            const usb_interface_descriptor_t* intf,
                            const usb_endpoint_descriptor_t* ep, size_t req_size);

  // Device protocol implementation.
  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkRelease();

  // FIDL methods.

  void OpenSession(OpenSessionRequestView request, OpenSessionCompleter::Sync& completer) override;

  void GetInfo(GetInfoCompleter::Sync& completer) final;
  void Read(ReadCompleter::Sync& completer) final;
  void Write(WriteRequestView request, WriteCompleter::Sync& completer) final;

 private:
  zx_status_t Init(int index, const usb_interface_descriptor_t* intf,
                   const usb_endpoint_descriptor_t* ep);
  void WriteComplete(usb_request_t* req);

  zx_status_t WriteInternal(const uint8_t* src, size_t length);

  UsbDevice usb_;

  // pool of free USB requests
  UsbRequestQueue free_write_reqs_ TA_GUARDED(mutex_);
  // mutex for synchronizing access to free_write_reqs and open
  fbl::Mutex mutex_;
  // completion signals free_write_reqs not empty
  sync_completion_t free_write_completion_;

  using Binding = struct {
    fidl::ServerBindingRef<fuchsia_hardware_midi::Device> binding;
    std::optional<ddk::UnbindTxn> unbind_txn;
  };
  std::optional<Binding> binding_;

  uint64_t parent_req_size_;
};

}  // namespace usb
}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_USB_AUDIO_USB_MIDI_SINK_H_
