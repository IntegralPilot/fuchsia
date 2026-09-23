// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-midi-source.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>

#include <algorithm>

#include <ddktl/fidl.h>
#include <fbl/auto_lock.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

#include "midi.h"

namespace audio {
namespace usb {

constexpr size_t READ_REQ_COUNT = 20;

void UsbMidiSource::ReadComplete(usb_request_t* req) {
  if (req->response.status == ZX_ERR_IO_NOT_PRESENT) {
    usb_request_release(req);
    return;
  }

  fbl::AutoLock lock(&mutex_);

  if (req->response.status == ZX_OK && req->response.actual > 0) {
    completed_reads_.push(UsbRequest(req, parent_req_size_));
    read_ready_.Signal();
  } else {
    usb_request_complete_callback_t complete = {
        .callback = [](void* ctx,
                       usb_request_t* req) { static_cast<UsbMidiSource*>(ctx)->ReadComplete(req); },
        .ctx = this,
    };
    usb_.RequestQueue(req, &complete);
  }
}

void UsbMidiSource::DdkUnbind(ddk::UnbindTxn txn) {
  if (binding_.has_value()) {
    Binding& binding = binding_.value();
    ZX_ASSERT(!binding.unbind_txn.has_value());
    binding.unbind_txn.emplace(std::move(txn));
    binding.binding.Unbind();
  } else {
    txn.Reply();
  }
}

void UsbMidiSource::DdkRelease() { delete this; }

void UsbMidiSource::OpenSession(OpenSessionRequestView request,
                                OpenSessionCompleter::Sync& completer) {
  if (binding_.has_value()) {
    request->session.Close(ZX_ERR_ALREADY_BOUND);
    return;
  }
  // queue up reads, including stale completed reads
  usb_request_complete_callback_t complete = {
      .callback = [](void* ctx,
                     usb_request_t* req) { static_cast<UsbMidiSource*>(ctx)->ReadComplete(req); },
      .ctx = this,
  };
  std::optional<UsbRequest> req;
  {
    fbl::AutoLock lock(&mutex_);
    while ((req = completed_reads_.pop()).has_value()) {
      usb_.RequestQueue(req->take(), &complete);
    }
    while ((req = free_read_reqs_.pop()).has_value()) {
      usb_.RequestQueue(req->take(), &complete);
    }
  }
  binding_ = Binding{
      .binding = fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                  std::move(request->session), this,
                                  [](UsbMidiSource* self, fidl::UnbindInfo,
                                     fidl::ServerEnd<fuchsia_hardware_midi::Device>) {
                                    std::optional opt = std::exchange(self->binding_, {});
                                    ZX_ASSERT(opt.has_value());
                                    Binding& binding = opt.value();
                                    if (binding.unbind_txn.has_value()) {
                                      binding.unbind_txn.value().Reply();
                                    }
                                  }),
  };
}

zx_status_t UsbMidiSource::ReadInternal(void* data, size_t len, size_t* actual) {
  if (len < 3) {
    return ZX_ERR_BUFFER_TOO_SMALL;
  }

  fbl::AutoLock lock(&mutex_);

  // Block until read is ready.
  auto req = completed_reads_.pop();
  if (!req.has_value()) {
    read_ready_.Wait(&mutex_);

    req = completed_reads_.pop();
    ZX_ASSERT(req.has_value());
  }

  // MIDI events are 4 bytes. We can ignore the zeroth byte
  zx::result<size_t> copied = req->CachedCopyFrom(data, 3, 1);
  if (copied.is_error() || *copied == 0) {
    *actual = 0;
  } else {
    *actual = std::min<size_t>(*copied, get_midi_message_length(*(static_cast<uint8_t*>(data))));
  }
  free_read_reqs_.push(std::move(*req));

  usb_request_complete_callback_t complete = {
      .callback = [](void* ctx,
                     usb_request_t* req) { static_cast<UsbMidiSource*>(ctx)->ReadComplete(req); },
      .ctx = this,
  };

  while ((req = free_read_reqs_.pop()).has_value()) {
    usb_.RequestQueue(req->take(), &complete);
  }

  return ZX_OK;
}

void UsbMidiSource::GetInfo(GetInfoCompleter::Sync& completer) {
  fuchsia_hardware_midi::wire::Info info = {
      .is_sink = false,
      .is_source = true,
  };
  completer.Reply(info);
}

void UsbMidiSource::Read(ReadCompleter::Sync& completer) {
  std::array<uint8_t, fuchsia_hardware_midi::wire::kReadSize> buffer{};
  size_t actual = 0;
  auto status = ReadInternal(buffer.data(), buffer.size(), &actual);
  if (status == ZX_OK) {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(buffer.data(), actual));
  } else {
    completer.ReplyError(status);
  }
}

void UsbMidiSource::Write(WriteRequestView request, WriteCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t UsbMidiSource::Create(zx_device_t* parent, const UsbDevice& usb, int index,
                                  const usb_interface_descriptor_t* intf,
                                  const usb_endpoint_descriptor_t* ep, const size_t req_size) {
  auto dev = std::make_unique<UsbMidiSource>(parent, usb, req_size);
  auto status = dev->Init(index, intf, ep);
  if (status != ZX_OK) {
    return status;
  }

  // devmgr is now in charge of the device.
  [[maybe_unused]] auto* _ = dev.release();
  return ZX_OK;
}

zx_status_t UsbMidiSource::Init(int index, const usb_interface_descriptor_t* intf,
                                const usb_endpoint_descriptor_t* ep) {
  int packet_size = usb_ep_max_packet(ep);
  if (intf->b_alternate_setting != 0) {
    usb_.SetInterface(intf->b_interface_number, intf->b_alternate_setting);
  }
  for (size_t i = 0; i < READ_REQ_COUNT; i++) {
    std::optional<UsbRequest> req;
    auto status = UsbRequest::Alloc(&req, packet_size, ep->b_endpoint_address, parent_req_size_);
    if (status != ZX_OK) {
      return status;
    }
    req->request()->header.length = packet_size;
    fbl::AutoLock al(&mutex_);
    free_read_reqs_.push(std::move(*req));
  }

  char name[ZX_DEVICE_NAME_MAX];
  snprintf(name, sizeof(name), "usb-midi-source-%d", index);

  return DdkAdd(name);
}

}  // namespace usb
}  // namespace audio
