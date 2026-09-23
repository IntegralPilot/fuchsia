// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "batch_case.h"

#include <fidl/test.ipc/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/assert.h>

void BatchCase::send(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  std::string message = std::string(config_.message_size, 'a');
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->send_duration);
    fidl::ClientEnd<test_ipc::BatchSender> client_end(std::move(chan));
    fidl::WireSyncClient client(std::move(client_end));

    std::vector<std::string> batch_data(config_.batch_size, message);
    std::vector<fidl::StringView> batch(config_.batch_size);
    for (size_t i = 0; i < config_.batch_size; ++i) {
      batch[i] = fidl::StringView::FromExternal(batch_data[i].data(), batch_data[i].size());
    }

    size_t left_to_send = config_.messages_to_send;
    while (left_to_send > 0) {
      ZX_ASSERT(left_to_send >= config_.batch_size);
      left_to_send -= config_.batch_size;
      auto ret = client->Send(fidl::VectorView<fidl::StringView>::FromExternal(batch));
      if (!ret.ok()) {
        FX_LOGS(ERROR) << "Failed to send: " << ret.status_string();

        std::terminate();
      }
    }
  }

  stop_barrier_.arrive_and_wait();
}

namespace {
class BatchReceiver : public fidl::WireServer<test_ipc::BatchSender> {
  using SendCompleter = ::fidl::internal::WireCompleter<test_ipc::BatchSender::Send>;

 public:
  explicit BatchReceiver(size_t* received) : received_(received) {}

  void Send(::test_ipc::wire::BatchSenderSendRequest* request,
            SendCompleter::Sync& completer) override {
    *received_ += request->messages.size();
    completer.Reply();
  }

 private:
  size_t* received_;
};
}  // namespace

void BatchCase::recv(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->recv_duration);

    fidl::ServerEnd<test_ipc::BatchSender> server_end(std::move(chan));
    size_t received = 0;
    BatchReceiver receiver(&received);
    auto binding = fidl::BindServer(
        loop.dispatcher(), std::move(server_end), &receiver,
        [&loop](BatchReceiver*, fidl::UnbindInfo info,
                fidl::ServerEnd<test_ipc::BatchSender> server_end) { loop.Quit(); });

    loop.Run();
    ZX_ASSERT(received == config_.messages_to_send);
  }

  stop_barrier_.arrive_and_wait();
}
