// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <unistd.h>

#include <atomic>
#include <condition_variable>
#include <mutex>
#include <queue>
#include <span>
#include <thread>
#include <unordered_set>

#include <gtest/gtest.h>

#include "src/devices/lib/block/block.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/block_server/block_server.h"
#include "src/storage/lib/buffer/owned_vmoid.h"

namespace block_server {
namespace {

namespace fblock = ::fuchsia_storage_block;

constexpr uint64_t kBlocks = 1024;
constexpr uint64_t kBlockSize = 512;

using RequestHook = std::function<zx::result<>(const Request&)>;

class TestInterface : public Interface {
 public:
  explicit TestInterface(const block_server::PartitionInfo& info) {
    server_.emplace(info, this);
    // We process requests on a different thread to make sure we support that.
    request_thread_ = std::jthread([this](const std::stop_token& stop_token) {
      while (!stop_token.stop_requested()) {
        Request request;
        {
          std::unique_lock lock(mutex_);
          requests_cv_.wait(mutex_, stop_token, [this] { return !requests_.empty(); });
          if (stop_token.stop_requested()) {
            return;
          }
          request = requests_.front();
          requests_.pop();
        }
        HandleRequest(request);
      }
    });
  }

  // Deletes the server instance.
  void ResetServer() { server_.reset(); }

  BlockServer& server() { return *server_; }
  int threads_running() const { return threads_running_; }

  // Sets a callback `hook` to be invoked before each request is handled. `hook` will be invoked
  // on a dedicated background thread.
  void SetHook(RequestHook hook) { hook_ = std::move(hook); }

  void StartThread(std::unique_ptr<Thread> thread) override {
    std::thread([this, thread = std::move(thread)]() mutable {
      ++threads_running_;
      thread->Run();

      // Deliberately add a delay to increase the chances of catching regressions where the server
      // does not wait for the thread to terminate.
      usleep(1000);

      --threads_running_;
    }).detach();
  }

  void OnNewSession(std::unique_ptr<Session> session) override {
    std::thread([this, session = std::move(session)]() mutable {
      ++threads_running_;
      session->Run();

      // Deliberately add a delay to increase the chances of catching regressions where the server
      // does not wait for the thread to terminate.
      usleep(1000);

      --threads_running_;
    }).detach();
  }

  void OnRequests(std::span<Request> requests) override {
    {
      std::scoped_lock lock(mutex_);
      for (const Request& request : requests) {
        requests_.push(request);
      }
    }
    requests_cv_.notify_all();
  }

 private:
  void HandleRequest(const Request& request) {
    if (hook_) {
      if (zx::result status = (*hook_)(request); status.is_error()) {
        server_->SendReply(request.request_id, status);
        return;
      }
    }
    switch (request.operation.tag) {
      case Operation::Tag::Read:
        EXPECT_EQ(
            request.vmo->write(&data_[request.operation.read.device_block_offset * kBlockSize],
                               request.operation.read.vmo_offset,
                               request.operation.read.block_count * kBlockSize),
            ZX_OK);
        break;

      case Operation::Tag::Write:
        EXPECT_EQ(
            request.vmo->read(&data_[request.operation.write.device_block_offset * kBlockSize],
                              request.operation.write.vmo_offset,
                              request.operation.write.block_count * kBlockSize),
            ZX_OK);
        break;
      case Operation::Tag::Flush:
        break;
      default:
        ZX_PANIC("Unexpected operation");
    }
    server_->SendReply(request.request_id, zx::ok());
  }

  std::optional<BlockServer> server_;
  std::atomic<int> threads_running_ = 0;
  std::unique_ptr<uint8_t[]> data_ = std::make_unique<uint8_t[]>(kBlockSize * kBlocks);
  std::optional<RequestHook> hook_;

  std::mutex mutex_;
  std::condition_variable_any requests_cv_;
  std::queue<Request> requests_;
  std::jthread request_thread_;
};

TEST(BlockServer, Basic) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.server().Serve(std::move(server_end));

  auto client = block_client::RemoteBlockDevice::Create(*std::move(client_end));
  ASSERT_EQ(client.status_value(), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write("hello", kBlockSize, 5), ZX_OK);
  storage::Vmoid vmoid;
  ASSERT_EQ(client->BlockAttachVmo(vmo, &vmoid), ZX_OK);
  storage::OwnedVmoid owned_vmoid(std::move(vmoid), (*client).get());

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
          },
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 1,
      .dev_offset = 3,
  };

  ASSERT_EQ(client->FifoTransaction(&request, 1), ZX_OK);

  request.command.opcode = BLOCK_OPCODE_READ;
  request.vmo_offset = 2;
  ASSERT_EQ(client->FifoTransaction(&request, 1), ZX_OK);

  char buffer[6] = {};
  ASSERT_EQ(vmo.read(buffer, 2 * kBlockSize, sizeof(buffer)), ZX_OK);

  ASSERT_EQ(memcmp(buffer, "hello", 6), 0);
}

TEST(BlockServer, Termination) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.server().Serve(std::move(server_end));

  {
    std::unique_ptr<block_client::RemoteBlockDevice> client;
    auto client_result = block_client::RemoteBlockDevice::Create(*std::move(client_end));
    ASSERT_EQ(client_result.status_value(), ZX_OK);
    client = *std::move(client_result);
  }

  test_interface.ResetServer();

  EXPECT_EQ(test_interface.threads_running(), 0);
}

TEST(BlockServer, AsyncTermination) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  std::unique_ptr<block_client::RemoteBlockDevice> client;

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });

  test_interface.server().Serve(std::move(server_end));

  auto client_result = block_client::RemoteBlockDevice::Create(*std::move(client_end));
  ASSERT_EQ(client_result.status_value(), ZX_OK);
  client = *std::move(client_result);

  sync_completion_t completion;
  test_interface.server().DestroyAsync([&] {
    EXPECT_EQ(test_interface.threads_running(), 0);
    sync_completion_signal(&completion);
  });

  sync_completion_wait(&completion, ZX_TIME_INFINITE);
}

TEST(BlockServer, FailedOnNewSession) {
  class TestInterfaceWithFailedOnNewSession : public TestInterface {
   public:
    explicit TestInterfaceWithFailedOnNewSession(const block_server::PartitionInfo info)
        : TestInterface(info) {}
    void OnNewSession(std::unique_ptr<Session> session) override {
      // Do nothing.
    }
  };

  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterfaceWithFailedOnNewSession test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });

  std::unique_ptr<block_client::RemoteBlockDevice> client;

  test_interface.server().Serve(std::move(server_end));

  auto client_result = block_client::RemoteBlockDevice::Create(*std::move(client_end));
  EXPECT_EQ(client_result.status_value(), ZX_ERR_PEER_CLOSED);

  test_interface.ResetServer();
  EXPECT_EQ(test_interface.threads_running(), 0);
}

TEST(BlockServer, FullFifo) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });

  test_interface.server().Serve(std::move(server_end));

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_TRUE(fidl::WireCall(*client_end)->OpenSession(std::move(server)).ok());
  const fidl::WireResult result = fidl::WireCall(session)->GetFifo();
  ASSERT_TRUE(result.ok());
  fit::result response = result.value();
  ASSERT_TRUE(response.is_ok());
  zx::fifo fifo = std::move(response->fifo);

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
          },
      .vmoid = 1,  // Invalid, but doesn't matter.
      .length = 1,
      .vmo_offset = 1,
      .dev_offset = 3,
  };

  // Write 1000 requests without removing the responses.
  for (uint32_t request_id = 0; request_id < 1000; ++request_id) {
    request.reqid = request_id;
    zx_status_t status;
    size_t actual;
    while ((status = fifo.write(sizeof(BlockFifoRequest), &request, 1, &actual)) ==
           ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      fifo.wait_one(ZX_FIFO_WRITABLE, zx::time::infinite(), &signals);
    }
    ASSERT_EQ(actual, 1u);
    ASSERT_EQ(status, ZX_OK);
  }

  // Now make sure we receive the 1000 requests.
  std::unordered_set<uint32_t> received;
  for (int i = 0; i < 1000; ++i) {
    BlockFifoResponse response;
    zx_status_t status;
    size_t actual;
    while ((status = fifo.read(sizeof(BlockFifoResponse), &response, 1, &actual)) ==
           ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), &signals);
    }
    ASSERT_EQ(status, ZX_OK);
    EXPECT_TRUE(received.insert(response.reqid).second);
  }
}

TEST(BlockServer, Group) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });

  test_interface.server().Serve(std::move(server_end));

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_TRUE(fidl::WireCall(*client_end)->OpenSession(std::move(server)).ok());

  zx::fifo fifo;
  {
    fidl::WireResult result = fidl::WireCall(session)->GetFifo();
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    fifo = std::move(response->fifo);
  }

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(1024ul * 1024, 0, &vmo), ZX_OK);
  zx::vmo duplicate;
  ASSERT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate), ZX_OK);

  uint16_t vmo_id;
  {
    fidl::WireResult result = fidl::WireCall(session)->AttachVmo(std::move(duplicate));
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    vmo_id = response->vmoid.id;
  }

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_READ,
              .flags = BLOCK_IO_FLAG_GROUP_ITEM,
          },
      .group = 1234,
      .vmoid = vmo_id,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  // Write 1000 requests as a group.
  for (uint32_t request_id = 0; request_id < 1000; ++request_id) {
    request.reqid = request_id;
    if (request_id == 999)
      request.command.flags |= BLOCK_IO_FLAG_GROUP_LAST;
    zx_status_t status;
    size_t actual;
    while ((status = fifo.write(sizeof(BlockFifoRequest), &request, 1, &actual)) ==
           ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      fifo.wait_one(ZX_FIFO_WRITABLE, zx::time::infinite(), &signals);
    }
    ASSERT_EQ(actual, 1u);
    ASSERT_EQ(status, ZX_OK);
  }

  BlockFifoResponse response;
  zx_status_t status;
  size_t actual;
  while ((status = fifo.read(sizeof(BlockFifoResponse), &response, 1, &actual)) ==
         ZX_ERR_SHOULD_WAIT) {
    zx_signals_t signals;
    fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), &signals);
  }
  ASSERT_EQ(status, ZX_OK);
  EXPECT_EQ(response.status, ZX_OK);
  EXPECT_EQ(response.group, 1234);
  EXPECT_EQ(response.reqid, 999u);
}

TEST(BlockServer, SplitRequest) {
  Request request = {.operation = {.tag = Operation::Tag::Read,
                                   .read = {
                                       .device_block_offset = 10,
                                       .block_count = 20,
                                       .vmo_offset = 4096,
                                   }}};

  Request head = SplitRequest(request, 5, 512);

  EXPECT_EQ(head.operation.read.device_block_offset, 10u);
  EXPECT_EQ(head.operation.read.block_count, 5u);
  EXPECT_EQ(head.operation.read.vmo_offset, 4096u);

  EXPECT_EQ(request.operation.read.device_block_offset, 15u);
  EXPECT_EQ(request.operation.read.block_count, 15u);
  EXPECT_EQ(request.operation.read.vmo_offset, 6656u);
}

TEST(BlockServer, SimulatedBarrierFlushFailure) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.SetHook([](const Request& request) -> zx::result<> {
    if (request.operation.tag == Operation::Tag::Flush) {
      return zx::error(ZX_ERR_IO);
    }
    return zx::ok();
  });
  test_interface.server().Serve(std::move(server_end));

  auto client = block_client::RemoteBlockDevice::Create(*std::move(client_end));
  ASSERT_EQ(client.status_value(), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write("hello", kBlockSize, 5), ZX_OK);
  storage::Vmoid vmoid;
  ASSERT_EQ(client->BlockAttachVmo(vmo, &vmoid), ZX_OK);
  storage::OwnedVmoid owned_vmoid(std::move(vmoid), (*client).get());

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
              .flags = BLOCK_IO_FLAG_PRE_BARRIER,
          },
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 1,
      .dev_offset = 3,
  };

  ASSERT_EQ(client->FifoTransaction(&request, 1), ZX_ERR_IO);

  test_interface.ResetServer();
  EXPECT_EQ(test_interface.threads_running(), 0);
}

TEST(BlockServer, SimulatedBarrierWriteFailure) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.SetHook([](const Request& request) -> zx::result<> {
    if (request.operation.tag == Operation::Tag::Write) {
      return zx::error(ZX_ERR_IO);
    }
    return zx::ok();
  });
  test_interface.server().Serve(std::move(server_end));

  auto client = block_client::RemoteBlockDevice::Create(*std::move(client_end));
  ASSERT_EQ(client.status_value(), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write("hello", kBlockSize, 5), ZX_OK);
  storage::Vmoid vmoid;
  ASSERT_EQ(client->BlockAttachVmo(vmo, &vmoid), ZX_OK);
  storage::OwnedVmoid owned_vmoid(std::move(vmoid), (*client).get());

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
              .flags = BLOCK_IO_FLAG_PRE_BARRIER,
          },
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 1,
      .dev_offset = 3,
  };

  ASSERT_EQ(client->FifoTransaction(&request, 1), ZX_ERR_IO);

  test_interface.ResetServer();
  EXPECT_EQ(test_interface.threads_running(), 0);
}

TEST(BlockServer, SimulatedFuaFlushFailure) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.SetHook([](const Request& request) -> zx::result<> {
    if (request.operation.tag == Operation::Tag::Flush) {
      return zx::error(ZX_ERR_IO);
    }
    return zx::ok();
  });
  test_interface.server().Serve(std::move(server_end));

  auto client = block_client::RemoteBlockDevice::Create(*std::move(client_end));
  ASSERT_EQ(client.status_value(), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write("hello", kBlockSize, 5), ZX_OK);
  storage::Vmoid vmoid;
  ASSERT_EQ(client->BlockAttachVmo(vmo, &vmoid), ZX_OK);
  storage::OwnedVmoid owned_vmoid(std::move(vmoid), (*client).get());

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
              .flags = BLOCK_IO_FLAG_FORCE_ACCESS,
          },
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 1,
      .dev_offset = 3,
  };

  ASSERT_EQ(client->FifoTransaction(&request, 1), ZX_ERR_IO);

  test_interface.ResetServer();
  EXPECT_EQ(test_interface.threads_running(), 0);
}

TEST(BlockServer, SimulatedFuaWriteFailure) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.SetHook([](const Request& request) -> zx::result<> {
    if (request.operation.tag == Operation::Tag::Write) {
      return zx::error(ZX_ERR_IO);
    }
    return zx::ok();
  });
  test_interface.server().Serve(std::move(server_end));

  auto client = block_client::RemoteBlockDevice::Create(*std::move(client_end));
  ASSERT_EQ(client.status_value(), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  ASSERT_EQ(vmo.write("hello", kBlockSize, 5), ZX_OK);
  storage::Vmoid vmoid;
  ASSERT_EQ(client->BlockAttachVmo(vmo, &vmoid), ZX_OK);
  storage::OwnedVmoid owned_vmoid(std::move(vmoid), (*client).get());

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
              .flags = BLOCK_IO_FLAG_FORCE_ACCESS,
          },
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 1,
      .dev_offset = 3,
  };

  ASSERT_EQ(client->FifoTransaction(&request, 1), ZX_ERR_IO);

  test_interface.ResetServer();
  EXPECT_EQ(test_interface.threads_running(), 0);
}

TEST(BlockServer, GroupWithSimulatedFua) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.SetHook([&, flushed = false, last_write = std::optional<Request>()](
                             const Request& request) mutable -> zx::result<> {
    if (request.operation.tag == Operation::Tag::Write) {
      last_write = request;
    } else if (request.operation.tag == Operation::Tag::Flush) {
      EXPECT_TRUE(last_write);
      EXPECT_EQ(last_write->operation.write.device_block_offset, 1u);
      EXPECT_FALSE(flushed);
      flushed = true;
    }
    return zx::ok();
  });
  test_interface.server().Serve(std::move(server_end));

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_TRUE(fidl::WireCall(*client_end)->OpenSession(std::move(server)).ok());

  zx::fifo fifo;
  {
    fidl::WireResult result = fidl::WireCall(session)->GetFifo();
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    fifo = std::move(response->fifo);
  }

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(1024ul * 1024, 0, &vmo), ZX_OK);
  zx::vmo duplicate;
  ASSERT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate), ZX_OK);

  uint16_t vmo_id;
  {
    fidl::WireResult result = fidl::WireCall(session)->AttachVmo(std::move(duplicate));
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    vmo_id = response->vmoid.id;
  }

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
              .flags = BLOCK_IO_FLAG_GROUP_ITEM,
          },
      .group = 1234,
      .vmoid = vmo_id,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  // Write 3 requests as a group.  Set the FORCE_ACCESS flag on the middle one.
  for (uint32_t request_id = 0; request_id < 3; ++request_id) {
    request.reqid = request_id;
    request.dev_offset = request_id;
    if (request_id == 1) {
      request.command.flags |= BLOCK_IO_FLAG_FORCE_ACCESS;
    } else {
      request.command.flags &= ~BLOCK_IO_FLAG_FORCE_ACCESS;
    }
    if (request_id == 2)
      request.command.flags |= BLOCK_IO_FLAG_GROUP_LAST;
    zx_status_t status;
    size_t actual;
    while ((status = fifo.write(sizeof(BlockFifoRequest), &request, 1, &actual)) ==
           ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      fifo.wait_one(ZX_FIFO_WRITABLE, zx::time::infinite(), &signals);
    }
    ASSERT_EQ(actual, 1u);
    ASSERT_EQ(status, ZX_OK);
  }

  BlockFifoResponse response;
  zx_status_t status;
  size_t actual;
  while ((status = fifo.read(sizeof(BlockFifoResponse), &response, 1, &actual)) ==
         ZX_ERR_SHOULD_WAIT) {
    zx_signals_t signals;
    fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), &signals);
  }
  ASSERT_EQ(status, ZX_OK);
  EXPECT_EQ(response.status, ZX_OK);
  EXPECT_EQ(response.group, 1234);
  EXPECT_EQ(response.reqid, 2u);
}
TEST(BlockServer, GroupWithSimulatedBarrierAndFailedFlush) {
  fidl::ServerEnd<fblock::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .device_flags = 0,
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
      .flags = 0,
      .max_transfer_size = 0,
  });
  test_interface.SetHook([](const Request& request) -> zx::result<> {
    if (request.operation.tag == Operation::Tag::Flush) {
      return zx::error(ZX_ERR_IO);
    }
    return zx::ok();
  });
  test_interface.server().Serve(std::move(server_end));

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_TRUE(fidl::WireCall(*client_end)->OpenSession(std::move(server)).ok());

  zx::fifo fifo;
  {
    fidl::WireResult result = fidl::WireCall(session)->GetFifo();
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    fifo = std::move(response->fifo);
  }

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(1024ul * 1024, 0, &vmo), ZX_OK);
  zx::vmo duplicate;
  ASSERT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate), ZX_OK);

  uint16_t vmo_id;
  {
    fidl::WireResult result = fidl::WireCall(session)->AttachVmo(std::move(duplicate));
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    vmo_id = response->vmoid.id;
  }

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
              .flags = BLOCK_IO_FLAG_GROUP_ITEM,
          },
      .group = 1234,
      .vmoid = vmo_id,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  // Write 1000 requests as a group.  Set the BARRIER flag on the middle one.
  for (uint32_t request_id = 0; request_id < 1000; ++request_id) {
    request.reqid = request_id;
    if (request_id == 500) {
      request.command.flags |= BLOCK_IO_FLAG_PRE_BARRIER;
    } else {
      request.command.flags &= ~BLOCK_IO_FLAG_PRE_BARRIER;
    }
    if (request_id == 999)
      request.command.flags |= BLOCK_IO_FLAG_GROUP_LAST;
    zx_status_t status;
    size_t actual;
    while ((status = fifo.write(sizeof(BlockFifoRequest), &request, 1, &actual)) ==
           ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      fifo.wait_one(ZX_FIFO_WRITABLE, zx::time::infinite(), &signals);
    }
    ASSERT_EQ(actual, 1u);
    ASSERT_EQ(status, ZX_OK);
  }

  BlockFifoResponse response;
  zx_status_t status;
  size_t actual;
  while ((status = fifo.read(sizeof(BlockFifoResponse), &response, 1, &actual)) ==
         ZX_ERR_SHOULD_WAIT) {
    zx_signals_t signals;
    fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), &signals);
  }
  ASSERT_EQ(status, ZX_OK);
  EXPECT_EQ(response.status, ZX_ERR_IO);
  EXPECT_EQ(response.group, 1234);
  EXPECT_EQ(response.reqid, 999u);
}

TEST(BlockServer, GroupWithSimulatedFuaAndFailedFlush) {
  fidl::ServerEnd<fuchsia_storage_block::Block> server_end;
  zx::result client_end = fidl::CreateEndpoints(&server_end);
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  TestInterface test_interface(PartitionInfo{
      .start_block = 0,
      .block_count = kBlocks,
      .block_size = kBlockSize,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "partition",
  });
  test_interface.SetHook([](const Request& request) -> zx::result<> {
    if (request.operation.tag == Operation::Tag::Flush) {
      return zx::error(ZX_ERR_IO);
    }
    return zx::ok();
  });
  test_interface.server().Serve(std::move(server_end));

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  ASSERT_TRUE(fidl::WireCall(*client_end)->OpenSession(std::move(server)).ok());

  zx::fifo fifo;
  {
    fidl::WireResult result = fidl::WireCall(session)->GetFifo();
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    fifo = std::move(response->fifo);
  }

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(1024ul * 1024, 0, &vmo), ZX_OK);
  zx::vmo duplicate;
  ASSERT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate), ZX_OK);

  uint16_t vmo_id;
  {
    fidl::WireResult result = fidl::WireCall(session)->AttachVmo(std::move(duplicate));
    ASSERT_TRUE(result.ok());
    fit::result response = result.value();
    ASSERT_TRUE(response.is_ok());
    vmo_id = response->vmoid.id;
  }

  BlockFifoRequest request = {
      .command =
          {
              .opcode = BLOCK_OPCODE_WRITE,
              .flags = BLOCK_IO_FLAG_GROUP_ITEM,
          },
      .group = 1234,
      .vmoid = vmo_id,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  // Write 1000 requests as a group.  Set the FORCE_ACCESS flag on the middle one.
  for (uint32_t request_id = 0; request_id < 1000; ++request_id) {
    request.reqid = request_id;
    if (request_id == 500) {
      request.command.flags |= BLOCK_IO_FLAG_FORCE_ACCESS;
    } else {
      request.command.flags &= ~BLOCK_IO_FLAG_FORCE_ACCESS;
    }
    if (request_id == 999)
      request.command.flags |= BLOCK_IO_FLAG_GROUP_LAST;
    zx_status_t status;
    size_t actual;
    while ((status = fifo.write(sizeof(BlockFifoRequest), &request, 1, &actual)) ==
           ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      fifo.wait_one(ZX_FIFO_WRITABLE, zx::time::infinite(), &signals);
    }
    ASSERT_EQ(actual, 1u);
    ASSERT_EQ(status, ZX_OK);
  }

  BlockFifoResponse response;
  zx_status_t status;
  size_t actual;
  while ((status = fifo.read(sizeof(BlockFifoResponse), &response, 1, &actual)) ==
         ZX_ERR_SHOULD_WAIT) {
    zx_signals_t signals;
    fifo.wait_one(ZX_FIFO_READABLE, zx::time::infinite(), &signals);
  }
  ASSERT_EQ(status, ZX_OK);
  EXPECT_EQ(response.status, ZX_ERR_IO);
  EXPECT_EQ(response.group, 1234);
  EXPECT_EQ(response.reqid, 999u);
}

class TestDriverInterface : public DriverInterface {
 public:
  void OnRequests(std::span<Request> requests) final {}
  std::string_view SessionSchedulerRole() const override { return "foo"; }

  std::shared_ptr<std::atomic<size_t>> dispatcher_shutdown_count() const {
    return dispatcher_shutdown_count_;
  }

 protected:
  void OnDispatcherShutdown(fdf_dispatcher_t* dispatcher) const override {
    ++(*dispatcher_shutdown_count_);
    fdf_dispatcher_destroy(dispatcher);
  }

 private:
  std::shared_ptr<std::atomic<size_t>> dispatcher_shutdown_count_ =
      std::make_shared<std::atomic<size_t>>(0);
};

// Ensure that `DriverInterface::OnDispatcherShutdown` is called for every dispatcher that is
// created by the block server. Each dispatcher drivers create must be explicitly shutdown before
// they are asynchronously destroyed.
TEST(BlockServer, DriverInterfaceDispatcherCleanup) {
  fdf_testing::DriverRuntime runtime;
  TestDriverInterface test_interface;
  std::shared_ptr<std::atomic<size_t>> dispatcher_shutdown_count =
      test_interface.dispatcher_shutdown_count();

  {
    block_server::BlockServer server(
        PartitionInfo{
            .device_flags = 0,
            .start_block = 0,
            .block_count = kBlocks,
            .block_size = kBlockSize,
            .type_guid = {1, 2, 3, 4},
            .instance_guid = {5, 6, 7, 8},
            .name = "partition",
            .flags = 0,
            .max_transfer_size = 0,
        },
        &test_interface);

    auto [block_client, server_end] = fidl::Endpoints<fblock::Block>::Create();
    server.Serve(std::move(server_end));

    // Start 4 different sessions.
    std::array<fidl::ClientEnd<fuchsia_storage_block::Session>, 4> sessions;
    for (auto& session : sessions) {
      auto [session_client, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
      ASSERT_TRUE(fidl::WireCall(block_client)->OpenSession(std::move(server)).ok());
      session = std::move(session_client);
    }

    // Close all sessions. We should see all 4 of their dispatchers eventually get shutdown.
    sessions = {};
    runtime.RunUntil([&] { return dispatcher_shutdown_count->load() == 4; });
  }

  // Now that the server has been destroyed, we should see its dispatcher also get shutdown.
  runtime.RunUntil([&] { return dispatcher_shutdown_count->load() == 5; });
}

TEST(BlockServer, DriverInterfaceParallelSessions) {
  fdf_testing::DriverRuntime runtime;
  TestDriverInterface test_interface;
  std::shared_ptr<std::atomic<size_t>> dispatcher_shutdown_count =
      test_interface.dispatcher_shutdown_count();

  constexpr size_t kNumThreads = 12;
  constexpr size_t kIterations = 20;

  {
    block_server::BlockServer server(
        PartitionInfo{
            .device_flags = 0,
            .start_block = 0,
            .block_count = kBlocks,
            .block_size = kBlockSize,
            .type_guid = {1, 2, 3, 4},
            .instance_guid = {5, 6, 7, 8},
            .name = "partition",
            .flags = 0,
            .max_transfer_size = 0,
        },
        &test_interface);
    auto [block_client, server_end] = fidl::Endpoints<fblock::Block>::Create();
    server.Serve(std::move(server_end));

    // Spawn multiple threads which will continuously open, use, and close sessions.
    // There should never be more than `kNumThreads` sessions active at once, which means that we
    // should never hit the Driver Framework's internal thread limit.
    // See https://fxbug.dev/510041620 for context.
    std::atomic<size_t> completed_threads = 0;
    std::array<std::thread, kNumThreads> threads;
    for (auto& thread : threads) {
      thread = std::thread([&] {
        for (size_t i = 0; i < kIterations; ++i) {
          auto [session_client, session_server] =
              fidl::Endpoints<fuchsia_storage_block::Session>::Create();
          EXPECT_EQ(fidl::WireCall(block_client)->OpenSession(std::move(session_server)).status(),
                    ZX_OK);
          fidl::WireResult result = fidl::WireCall(session_client)->GetFifo();
          EXPECT_EQ(result.status(), ZX_OK);
          EXPECT_TRUE(result.value().is_ok());
          fidl::WireResult close_result = fidl::WireCall(session_client)->Close();
          EXPECT_EQ(close_result.status(), ZX_OK);
        }
        ++completed_threads;
      });
    }

    // Instead of `thread.join()` which blocks this thread, we need to cycle the runtime loops to
    // process any background messages and dispatcher teardowns.
    runtime.RunUntil([&] { return completed_threads.load() == kNumThreads; });

    for (auto& thread : threads) {
      thread.join();
    }

    runtime.RunUntil(
        [&] { return dispatcher_shutdown_count->load() == kNumThreads * kIterations; });

    std::atomic<bool> server_destroyed = false;
    server.DestroyAsync([&] { server_destroyed = true; });
    runtime.RunUntil([&] { return server_destroyed.load(); });
  }

  runtime.RunUntil(
      [&] { return dispatcher_shutdown_count->load() == (kNumThreads * kIterations) + 1; });
}

class ThreadLifetimeTestInterface : public DriverInterface {
 public:
  void OnRequests(std::span<Request> requests) final {}

  void set_on_shutdown(fit::callback<void()> cb) { on_shutdown_ = std::move(cb); }

 protected:
  void OnDispatcherShutdown(fdf_dispatcher_t* dispatcher) const override {
    if (on_shutdown_) {
      on_shutdown_();
    }
    fdf_dispatcher_destroy(dispatcher);
  }

 private:
  mutable fit::callback<void()> on_shutdown_;
};

TEST(BlockServer, ThreadLifetimeOutlivesDispatcherShutdown) {
  fdf_testing::DriverRuntime runtime;
  ThreadLifetimeTestInterface test_interface;

  std::atomic<bool> dispatcher_shutdown_ran = false;
  std::atomic<bool> thread_released_after_shutdown = false;

  test_interface.set_on_shutdown([&] { dispatcher_shutdown_ran = true; });

  {
    block_server::BlockServer server(
        PartitionInfo{
            .start_block = 0,
            .block_count = kBlocks,
            .block_size = kBlockSize,
            .type_guid = {1, 2, 3, 4},
            .instance_guid = {5, 6, 7, 8},
            .name = "partition",
        },
        &test_interface);

    auto [block_client, server_end] = fidl::Endpoints<fblock::Block>::Create();
    server.Serve(std::move(server_end));

    server.DestroyAsync([&] {
      if (dispatcher_shutdown_ran) {
        thread_released_after_shutdown = true;
      }
    });

    runtime.RunUntil([&] { return thread_released_after_shutdown.load(); });
  }

  EXPECT_TRUE(dispatcher_shutdown_ran);
  EXPECT_TRUE(thread_released_after_shutdown);
}

TEST(BlockServer, ServeAfterDestroyAsync) {
  fdf_testing::DriverRuntime runtime;
  ThreadLifetimeTestInterface test_interface;

  std::atomic<bool> server_destroyed = false;

  {
    block_server::BlockServer server(
        PartitionInfo{
            .start_block = 0,
            .block_count = kBlocks,
            .block_size = kBlockSize,
            .type_guid = {1, 2, 3, 4},
            .instance_guid = {5, 6, 7, 8},
            .name = "partition",
        },
        &test_interface);

    auto [block_client, server_end] = fidl::Endpoints<fblock::Block>::Create();
    server.Serve(std::move(server_end));

    // Start destruction.
    server.DestroyAsync([&] { server_destroyed = true; });

    // Try to serve a new connection. This should be rejected (close the channel).
    auto [block_client2, server_end2] = fidl::Endpoints<fblock::Block>::Create();
    server.Serve(std::move(server_end2));

    // The client end should be closed.
    zx_signals_t signals;
    EXPECT_EQ(
        block_client2.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &signals),
        ZX_OK);
    EXPECT_TRUE(signals & ZX_CHANNEL_PEER_CLOSED);

    runtime.RunUntil([&] { return server_destroyed.load(); });
  }
}

}  // namespace
}  // namespace block_server
