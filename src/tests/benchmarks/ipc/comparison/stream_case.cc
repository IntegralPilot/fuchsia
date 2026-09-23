// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "stream_case.h"

#include <fidl/test.ipc/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/assert.h>

void StreamCase::send(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  std::string message = std::string(config_.message_size, 'a');
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->send_duration);
    fidl::ClientEnd<test_ipc::StreamSender> client_end(std::move(chan));
    fidl::WireSyncClient client(std::move(client_end));

    zx::socket s1, s2;
    ZX_ASSERT(zx::socket::create(ZX_SOCKET_STREAM, &s1, &s2) == ZX_OK);
    ZX_ASSERT(client->Send(std::move(s1)).ok());

    size_t left_to_send = config_.messages_to_send;
    while (left_to_send > 0) {
      std::vector<uint8_t> formatted_msg(config_.batch_size * (sizeof(uint32_t) + message.size()));
      *reinterpret_cast<uint32_t*>(formatted_msg.data()) = static_cast<uint32_t>(message.size());
      std::memcpy(formatted_msg.data() + sizeof(uint32_t), message.data(), message.size());
      left_to_send -= 1;
      for (size_t i = 1; i < config_.batch_size && left_to_send > 0; ++i) {
        std::memcpy(formatted_msg.data() + (i * (sizeof(uint32_t) + message.size())),
                    formatted_msg.data(), sizeof(uint32_t) + message.size());
        left_to_send -= 1;
      }

      size_t offset = 0;
      while (offset < formatted_msg.size()) {
        size_t actual;
        auto ret =
            s2.write(0, formatted_msg.data() + offset, formatted_msg.size() - offset, &actual);
        if (ret == ZX_ERR_SHOULD_WAIT) {
          zx_signals_t obs;
          ZX_ASSERT(s2.wait_one(ZX_SOCKET_WRITABLE, zx::time::infinite(), &obs) == ZX_OK);
        } else {
          ZX_ASSERT(ret == ZX_OK);
          offset += actual;
        }
      }
    }
  }

  stop_barrier_.arrive_and_wait();
}

namespace {

class StreamReceiver : public fidl::WireServer<test_ipc::StreamSender> {
  using SendCompleter = ::fidl::internal::WireCompleter<test_ipc::StreamSender::Send>;

 public:
  explicit StreamReceiver(size_t* received) : received_(received) {}

  void Send(::test_ipc::wire::StreamSenderSendRequest* request,
            SendCompleter::Sync& completer) override {
    completer.Close(ZX_OK);

    auto socket = std::move(request->stream);

    std::vector<uint8_t> buffer;
    buffer.resize(64 * 1024);
    std::vector<uint8_t> remaining;

    while (true) {
      size_t actual;
      auto ret = socket.read(0, buffer.data(), buffer.size(), &actual);
      if (ret == ZX_ERR_PEER_CLOSED) {
        buffer.clear();
        actual = 0;
      } else if (ret == ZX_ERR_SHOULD_WAIT) {
        zx_signals_t obs;
        socket.wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED, zx::time::infinite(), &obs);
        continue;
      } else {
        if (ret != ZX_OK) {
          FX_LOGS(ERROR) << "Failed to read: " << ret;
        }
        ZX_ASSERT(ret == ZX_OK);
      }

      remaining.insert(remaining.end(), buffer.data(), buffer.data() + actual);

      std::optional<uint32_t> remaining_size;
      while (remaining.size() >= sizeof(uint32_t)) {
        remaining_size = *reinterpret_cast<uint32_t*>(remaining.data());
        if (remaining.size() < sizeof(uint32_t) + remaining_size.value()) {
          break;
        }
        *received_ += 1;
        remaining.erase(remaining.begin(),
                        remaining.begin() + sizeof(uint32_t) + remaining_size.value());
        remaining_size.reset();
      }

      if (actual == 0) {
        break;
      }
    }
  }

 private:
  size_t* received_;
};

}  // namespace

void StreamCase::recv(zx::channel chan, Timing* cur_timing) {
  ZX_ASSERT(chan.is_valid());
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  start_barrier_.arrive_and_wait();

  {
    Timer t(&cur_timing->recv_duration);

    fidl::ServerEnd<test_ipc::StreamSender> server_end(std::move(chan));
    size_t received = 0;
    StreamReceiver receiver(&received);
    auto binding = fidl::BindServer(
        loop.dispatcher(), std::move(server_end), &receiver,
        [&loop](StreamReceiver*, fidl::UnbindInfo info,
                fidl::ServerEnd<test_ipc::StreamSender> server_end) { loop.Quit(); });

    loop.Run();
    ZX_ASSERT(received == config_.messages_to_send);
  }

  stop_barrier_.arrive_and_wait();
}
