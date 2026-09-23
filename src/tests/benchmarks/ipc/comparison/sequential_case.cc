// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sequential_case.h"

#include <fidl/test.ipc/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/assert.h>

void SequentialCase::send(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  std::string message(config_.message_size, 'a');
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->send_duration);
    fidl::ClientEnd<test_ipc::SequentialSender> client_end(std::move(chan));
    fidl::WireSyncClient client(std::move(client_end));

    size_t left_to_send = config_.messages_to_send;
    while (left_to_send > 0) {
      left_to_send -= 1;
      if (!chan_capacity_.try_acquire()) {
        // Do a channel wait to simulate a syscall returning ZX_ERR_SHOULD_WAIT.
        zx_signals_t obs;
        client.client_end().channel().wait_one(ZX_CHANNEL_WRITABLE, zx::time::infinite(), &obs);
        chan_capacity_.acquire();
      }

      auto ret = client->Send(fidl::StringView::FromExternal(message.data(), message.size()));
      if (!ret.ok()) {
        FX_LOGS(ERROR) << "Failed to send: " << ret.status_string();
        std::terminate();
      }
    }
  }

  stop_barrier_.arrive_and_wait();
}

namespace {
class SequentialReceiver : public fidl::WireServer<test_ipc::SequentialSender> {
  using SendCompleter = ::fidl::internal::WireCompleter<test_ipc::SequentialSender::Send>;

 public:
  explicit SequentialReceiver(size_t* received, std::counting_semaphore<>* chan_capacity)
      : received_(received), chan_capacity_(chan_capacity) {}

  void Send(::test_ipc::wire::SequentialSenderSendRequest* request,
            SendCompleter::Sync& completer) override {
    chan_capacity_->release();
    *received_ += 1;
  }

 private:
  size_t* received_;
  std::counting_semaphore<>* chan_capacity_;
};
}  // namespace

void SequentialCase::recv(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->recv_duration);

    fidl::ServerEnd<test_ipc::SequentialSender> server_end(std::move(chan));
    size_t received = 0;
    SequentialReceiver receiver(&received, &chan_capacity_);
    auto binding = fidl::BindServer(
        loop.dispatcher(), std::move(server_end), &receiver,
        [&loop](SequentialReceiver*, fidl::UnbindInfo info,
                fidl::ServerEnd<test_ipc::SequentialSender> server_end) { loop.Quit(); });

    loop.Run();
    ZX_ASSERT(received == config_.messages_to_send);
  }

  stop_barrier_.arrive_and_wait();
}
