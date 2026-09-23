// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vmo_case.h"

#include <fidl/test.ipc/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>

void VmoCase::send(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  std::string message = std::string(config_.message_size, 'a');
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->send_duration);
    fidl::ClientEnd<test_ipc::VmoSender> client_end(std::move(chan));
    fidl::WireSyncClient client(std::move(client_end));

    size_t to_send = config_.messages_to_send;

    while (to_send > 0) {
      size_t this_batch = std::min(to_send, config_.batch_size * config_.per_vmo_batch);
      to_send -= this_batch;

      std::vector<zx::vmo> vmos;
      size_t left_in_this_vmo = config_.per_vmo_batch;
      size_t offset = 0;
      zx::vmo vmo;
      for (size_t i = 0; i < this_batch; ++i) {
        if (left_in_this_vmo == 0) {
          ZX_ASSERT(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &offset, sizeof(offset)) == ZX_OK);
          vmos.emplace_back(std::move(vmo));
          vmo = zx::vmo();
          left_in_this_vmo = config_.per_vmo_batch;
          offset = 0;
        }
        if (!vmo.is_valid()) {
          ZX_ASSERT(zx::vmo::create(message.size() * config_.per_vmo_batch, 0, &vmo) == ZX_OK);
        }
        vmo.write(message.data(), offset, message.size());
        offset += message.size();
        left_in_this_vmo -= 1;
      }
      if (vmo.is_valid()) {
        ZX_ASSERT(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &offset, sizeof(offset)) == ZX_OK);
        vmos.push_back(std::move(vmo));
      }

      auto ret = client->Send(fidl::VectorView<zx::vmo>::FromExternal(vmos));
      if (!ret.ok()) {
        FX_LOGS(ERROR) << "Failed to send VMO: " << ret.status_string();
        std::terminate();
      }
    }
  }

  stop_barrier_.arrive_and_wait();
}

namespace {
class VmoReceiver : public fidl::WireServer<test_ipc::VmoSender> {
  using SendCompleter = ::fidl::internal::WireCompleter<test_ipc::VmoSender::Send>;

 public:
  explicit VmoReceiver(size_t* received, size_t message_size, size_t per_vmo_batch)
      : received_bytes_(received), message_size_(message_size), per_vmo_batch_(per_vmo_batch) {}

  void Send(::test_ipc::wire::VmoSenderSendRequest* request,
            SendCompleter::Sync& completer) override {
    for (zx::vmo& vmo : request->vmos) {
      ZX_ASSERT(vmo.is_valid());
      size_t size;
      vmo.get_property(ZX_PROP_VMO_CONTENT_SIZE, &size, sizeof(size));
      ZX_ASSERT(size <= message_size_ * per_vmo_batch_);
      std::vector<uint8_t> buffer;
      buffer.resize(size);
      ZX_ASSERT(vmo.read(buffer.data(), 0, size) == ZX_OK);
      *received_bytes_ += size;
    }

    completer.Reply();
  }

 private:
  size_t* received_bytes_;
  size_t message_size_;
  size_t per_vmo_batch_;
};
}  // namespace

void VmoCase::recv(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->recv_duration);
    async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

    fidl::ServerEnd<test_ipc::VmoSender> server_end(std::move(chan));
    size_t received_bytes = 0;
    VmoReceiver receiver(&received_bytes, config_.message_size, config_.per_vmo_batch);
    auto binding =
        fidl::BindServer(loop.dispatcher(), std::move(server_end), &receiver,
                         [&loop](VmoReceiver*, fidl::UnbindInfo info,
                                 fidl::ServerEnd<test_ipc::VmoSender> server_end) { loop.Quit(); });

    loop.Run();
    ZX_ASSERT(received_bytes == config_.messages_to_send * config_.message_size);
  }

  stop_barrier_.arrive_and_wait();
}
