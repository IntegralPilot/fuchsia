// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-midi-sink.h"

#include <lib/sync/completion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>

#include <ddktl/fidl.h>
#include <fbl/auto_lock.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

#include "midi.h"

constexpr size_t WRITE_REQ_COUNT = 20;

namespace audio {
namespace usb {

void UsbMidiSink::WriteComplete(usb_request_t* req) {
  if (req->response.status == ZX_ERR_IO_NOT_PRESENT) {
    usb_request_release(req);
    return;
  }

  {
    fbl::AutoLock lock(&mutex_);
    free_write_reqs_.push(UsbRequest(req, parent_req_size_));
  }
  sync_completion_signal(&free_write_completion_);
}

void UsbMidiSink::DdkUnbind(ddk::UnbindTxn txn) {
  if (binding_.has_value()) {
    Binding& binding = binding_.value();
    ZX_ASSERT(!binding.unbind_txn.has_value());
    binding.unbind_txn.emplace(std::move(txn));
    binding.binding.Unbind();
  } else {
    txn.Reply();
  }
}

void UsbMidiSink::DdkRelease() { delete this; }

void UsbMidiSink::OpenSession(OpenSessionRequestView request,
                              OpenSessionCompleter::Sync& completer) {
  if (binding_.has_value()) {
    request->session.Close(ZX_ERR_ALREADY_BOUND);
    return;
  }
  binding_ = Binding{
      .binding = fidl::BindServer(
          fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(request->session), this,
          [](UsbMidiSink* self, fidl::UnbindInfo, fidl::ServerEnd<fuchsia_hardware_midi::Device>) {
            std::optional opt = std::exchange(self->binding_, {});
            ZX_ASSERT(opt.has_value());
            Binding& binding = opt.value();
            if (binding.unbind_txn.has_value()) {
              binding.unbind_txn.value().Reply();
            }
          }),
  };
}

zx_status_t UsbMidiSink::WriteInternal(const uint8_t* src, size_t length) {
  zx_status_t status = ZX_OK;

  while (length > 0) {
    sync_completion_wait(&free_write_completion_, ZX_TIME_INFINITE);

    std::optional<UsbRequest> req;
    {
      fbl::AutoLock lock(&mutex_);
      req = free_write_reqs_.pop();
      if (free_write_reqs_.is_empty()) {
        sync_completion_reset(&free_write_completion_);
      }
    }

    if (!req) {
      // shouldn't happen!
      return ZX_ERR_INTERNAL;
    }

    size_t message_length = get_midi_message_length(*src);
    if (message_length < 1 || message_length > length)
      return ZX_ERR_INVALID_ARGS;

    uint8_t buffer[4];
    buffer[0] = (src[0] & 0xF0) >> 4;
    buffer[1] = src[0];
    buffer[2] = (message_length > 1 ? src[1] : 0);
    buffer[3] = (message_length > 2 ? src[2] : 0);

    zx::result<size_t> result = req->CachedCopyTo(buffer, 4, 0);
    if (result.is_error()) {
      return result.status_value();
    }
    if (*result != 4) {
      return ZX_ERR_IO;
    }
    req->request()->header.length = 4;
    usb_request_complete_callback_t complete = {
        .callback = [](void* ctx,
                       usb_request_t* req) { static_cast<UsbMidiSink*>(ctx)->WriteComplete(req); },
        .ctx = this,
    };
    usb_.RequestQueue(req->take(), &complete);

    src += message_length;
    length -= message_length;
  }

  return status;
}

void UsbMidiSink::GetInfo(GetInfoCompleter::Sync& completer) {
  fuchsia_hardware_midi::wire::Info info = {
      .is_sink = true,
      .is_source = false,
  };
  completer.Reply(info);
}

void UsbMidiSink::Read(ReadCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void UsbMidiSink::Write(WriteRequestView request, WriteCompleter::Sync& completer) {
  auto status = WriteInternal(request->data.data(), request->data.size());
  if (status == ZX_OK) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(status);
  }
}

zx_status_t UsbMidiSink::Create(zx_device_t* parent, const UsbDevice& usb, int index,
                                const usb_interface_descriptor_t* intf,
                                const usb_endpoint_descriptor_t* ep, const size_t req_size) {
  auto dev = std::make_unique<UsbMidiSink>(parent, usb, req_size);
  auto status = dev->Init(index, intf, ep);
  if (status != ZX_OK) {
    return status;
  }

  // devmgr is now in charge of the device.
  [[maybe_unused]] auto* _ = dev.release();
  return ZX_OK;
}

zx_status_t UsbMidiSink::Init(int index, const usb_interface_descriptor_t* intf,
                              const usb_endpoint_descriptor_t* ep) {
  int packet_size = usb_ep_max_packet(ep);
  if (intf->b_alternate_setting != 0) {
    usb_.SetInterface(intf->b_interface_number, intf->b_alternate_setting);
  }

  for (size_t i = 0; i < WRITE_REQ_COUNT; i++) {
    std::optional<UsbRequest> req;
    auto status =
        UsbRequest::Alloc(&req, usb_ep_max_packet(ep), ep->b_endpoint_address, parent_req_size_);
    if (status != ZX_OK) {
      return status;
    }
    req->request()->header.length = packet_size;
    fbl::AutoLock lock(&mutex_);
    free_write_reqs_.push(std::move(*req));
  }
  sync_completion_signal(&free_write_completion_);

  char name[ZX_DEVICE_NAME_MAX];
  snprintf(name, sizeof(name), "usb-midi-sink-%d", index);

  return DdkAdd(name);
}

}  // namespace usb
}  // namespace audio
