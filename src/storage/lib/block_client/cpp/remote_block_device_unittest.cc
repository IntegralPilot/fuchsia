// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_client/cpp/remote_block_device.h"

#include <fidl/fuchsia.storage.block/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fzl/fifo.h>
#include <lib/zx/vmo.h>

#include <cstring>
#include <thread>
#include <unordered_set>
#include <utility>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <fbl/mutex.h>
#include <gtest/gtest.h>

#include "src/devices/lib/block/block.h"
#include "src/storage/lib/buffer/owned_vmoid.h"

namespace block_client {
namespace {

constexpr uint16_t kGoldenVmoid = 2;
constexpr uint32_t kBlockSize = 4096;
constexpr uint64_t kBlockCount = 10;

// Emulate the non-standard behavior of the block device which implements both the block device APIs
// and the Node API.
class MockBlockDevice final : public fidl::testing::WireTestBase<fuchsia_storage_block::Block> {
 public:
  explicit MockBlockDevice() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    // Create buffer for read / write calls
    buffer_.resize(kBlockSize * kBlockCount);
    ZX_ASSERT(loop_.StartThread() == ZX_OK);
  }

  ~MockBlockDevice() override {
    // Shutting down the loop will force all the unbind callbacks to run.
    loop_.Shutdown();
  }

  void BindServer(fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
    fidl::BindServer(loop_.dispatcher(), std::move(server_end), this);
  }

  zx_status_t ReadFifoRequests(BlockFifoRequest* requests, size_t* count) const {
    zx_signals_t seen;
    zx_status_t status = session_.fifo_.wait_one(ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED,
                                                 zx::deadline_after(zx::sec(5)), &seen);
    if (status != ZX_OK) {
      return status;
    }
    return session_.fifo_.read(requests, BLOCK_FIFO_MAX_DEPTH, count);
  }

  zx_status_t WriteFifoResponse(const BlockFifoResponse& response) const {
    return session_.fifo_.write_one(response);
  }

  bool FifoAttached() const { return session_.fifo_.get().is_valid(); }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FAIL() << "unexpected call to: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void GetVolumeInfo(GetVolumeInfoCompleter::Sync& completer) override {
    completer.Reply(ZX_ERR_NOT_SUPPORTED, {}, {});
  }

  void GetInfo(GetInfoCompleter::Sync& completer) override {
    completer.ReplySuccess({
        .block_count = kBlockCount,
        .block_size = kBlockSize,
        .max_transfer_size = kBlockSize,
    });
  }

  void OpenSession(OpenSessionRequestView request, OpenSessionCompleter::Sync& completer) override {
    if (FifoAttached()) {
      request->session.Close(ZX_ERR_BAD_STATE);
      return;
    }
    if (zx_status_t status =
            fzl::create_fifo(BLOCK_FIFO_MAX_DEPTH, 0, &session_.peer_fifo_, &session_.fifo_);
        status != ZX_OK) {
      request->session.Close(status);
      return;
    }
    fidl::BindServer(loop_.dispatcher(), std::move(request->session), &session_,
                     [](MockSession* session, fidl::UnbindInfo,
                        fidl::ServerEnd<fuchsia_storage_block::Session>) {
                       session->fifo_.reset();
                       session->peer_fifo_.reset();
                     });
  }

 private:
  class MockSession : public fidl::WireServer<fuchsia_storage_block::Session> {
   public:
    void GetFifo(GetFifoCompleter::Sync& completer) override {
      zx::fifo fifo;
      if (zx_status_t status = peer_fifo_.get().duplicate(ZX_RIGHT_SAME_RIGHTS, &fifo);
          status != ZX_OK) {
        completer.ReplyError(status);
        return;
      }
      completer.ReplySuccess(std::move(fifo));
    }

    void AttachVmo(AttachVmoRequestView request, AttachVmoCompleter::Sync& completer) override {
      completer.ReplySuccess({
          .id = kGoldenVmoid,
      });
    }

    void Close(CloseCompleter::Sync& completer) override {
      fifo_.reset();
      peer_fifo_.reset();
      completer.ReplySuccess();
      completer.Close(ZX_OK);
    }

    fzl::fifo<BlockFifoResponse, BlockFifoRequest> fifo_;
    fzl::fifo<BlockFifoRequest, BlockFifoResponse> peer_fifo_;
  };

  MockSession session_;
  std::vector<uint8_t> buffer_;
  async::Loop loop_;
};

// Tests that the RemoteBlockDevice can be created and immediately destroyed.
TEST(RemoteBlockDeviceTest, Constructor) {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();

  MockBlockDevice mock_device;
  mock_device.BindServer(std::move(server));

  zx::result device = RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>{client.TakeChannel()});
  ASSERT_TRUE(device.is_ok()) << device.status_string();
}

// Tests that a fifo is attached to the block device for the duration of the
// RemoteBlockDevice lifetime.
TEST(RemoteBlockDeviceTest, FifoClosedOnDestruction) {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();

  MockBlockDevice mock_device;
  mock_device.BindServer(std::move(server));

  EXPECT_FALSE(mock_device.FifoAttached());
  {
    zx::result device = RemoteBlockDevice::Create(
        fidl::ClientEnd<fuchsia_storage_block::Block>{client.TakeChannel()});
    ASSERT_TRUE(device.is_ok()) << device.status_string();
    EXPECT_TRUE(mock_device.FifoAttached());
  }
  EXPECT_FALSE(mock_device.FifoAttached());
}

// Tests that the RemoteBlockDevice is capable of transmitting and receiving
// messages with the block device.
TEST(RemoteBlockDeviceTest, WriteTransactionReadResponse) {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();

  MockBlockDevice mock_device;
  mock_device.BindServer(std::move(server));

  zx::result device = RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>{client.TakeChannel()});
  ASSERT_TRUE(device.is_ok()) << device.status_string();

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

  storage::OwnedVmoid vmoid;
  ASSERT_EQ(device->BlockAttachVmo(vmo, &vmoid.GetReference(device.value().get())), ZX_OK);
  ASSERT_EQ(kGoldenVmoid, vmoid.get());

  BlockFifoRequest request;
  request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  request.reqid = 1;
  request.group = 0;
  request.vmoid = vmoid.get();
  request.length = 1;
  request.vmo_offset = 0;
  request.dev_offset = 0;

  std::thread server_thread([&mock_device, &request] {
    BlockFifoRequest server_request;
    size_t actual;
    EXPECT_EQ(mock_device.ReadFifoRequests(&server_request, &actual), ZX_OK);
    EXPECT_EQ(actual, 1u);
    EXPECT_EQ(0, memcmp(&server_request, &request, sizeof(request)));

    BlockFifoResponse response;
    response.status = ZX_OK;
    response.reqid = request.reqid;
    response.group = request.group;
    response.count = 1;
    EXPECT_EQ(mock_device.WriteFifoResponse(response), ZX_OK);
  });

  ASSERT_EQ(device->FifoTransaction(&request, 1), ZX_OK);
  vmoid.TakeId();
  server_thread.join();
}

TEST(RemoteBlockDeviceTest, VolumeManagerOrdinals) {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();

  MockBlockDevice mock_device;
  mock_device.BindServer(std::move(server));

  zx::result device = RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>{client.TakeChannel()});
  ASSERT_TRUE(device.is_ok()) << device.status_string();

  // Querying the volume returns an error; the device doesn't implement
  // any FVM protocols. However, VolumeQuery utilizes a distinct
  // channel, so the connection should remain open.
  fuchsia_storage_block::wire::VolumeManagerInfo manager_info;
  fuchsia_storage_block::wire::VolumeInfo volume_info;
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, device->VolumeGetInfo(&manager_info, &volume_info));

  // Other block functions still function correctly.
  fuchsia_storage_block::wire::BlockInfo block_info;
  EXPECT_EQ(device->BlockGetInfo(&block_info), ZX_OK);
}

TEST(RemoteBlockDeviceTest, LargeThreadCountSuceeds) {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();

  MockBlockDevice mock_device;
  mock_device.BindServer(std::move(server));

  zx::result device = RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>{client.TakeChannel()});
  ASSERT_TRUE(device.is_ok()) << device.status_string();

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

  storage::OwnedVmoid vmoid;
  ASSERT_EQ(device->BlockAttachVmo(vmo, &vmoid.GetReference(device.value().get())), ZX_OK);
  ASSERT_EQ(kGoldenVmoid, vmoid.get());

  constexpr int kThreadCount = 2 * MAX_TXN_GROUP_COUNT;
  std::thread threads[kThreadCount];
  fbl::Mutex mutex;
  fbl::ConditionVariable condition;
  int done = 0;
  for (auto& thread : threads) {
    thread = std::thread(
        [device = device.value().get(), &mutex, &done, &condition, vmoid = vmoid.get()]() {
          BlockFifoRequest requests[] = {{
                                             .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
                                             .vmoid = vmoid,
                                             .length = 1,
                                         },
                                         {
                                             .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
                                             .vmoid = vmoid,
                                             .length = 1,
                                         }};
          ASSERT_EQ(device->FifoTransaction(requests, std::size(requests)), ZX_OK);
          fbl::AutoLock lock(&mutex);
          ++done;
          condition.Signal();
        });
  }
  vmoid.TakeId();  // We don't need the vmoid any more.
  BlockFifoRequest requests[kThreadCount * 2 + BLOCK_FIFO_MAX_DEPTH];
  size_t request_count = 0;
  // Maps from group to (request-id, count).
  std::unordered_map<groupid_t, std::pair<uint32_t, int>> groups;
  bool finished = false;
  while (!finished) {
    if (request_count < kThreadCount * 2) {
      // Read some more requests.
      size_t count = 0;
      ASSERT_EQ(mock_device.ReadFifoRequests(&requests[request_count], &count), ZX_OK);
      ASSERT_GT(count, 0u);

      // Check that all the outstanding requests we have use different group IDs.
      for (size_t i = request_count; i < request_count + count; ++i) {
        auto& group = groups[requests[i].group];
        ASSERT_LT(group.second, 2) << "request #=" << i;
        ++group.second;
        if (requests[i].command.flags & BLOCK_IO_FLAG_GROUP_LAST) {
          group.first = requests[i].reqid;
        }
      }

      request_count += count;
    }
    // Finish one request.
    for (const auto& [group_id, group] : groups) {
      if (group.second == 2) {
        BlockFifoResponse response;
        response.status = ZX_OK;
        response.reqid = group.first;
        response.group = group_id;
        response.count = 1;
        int last_done = done;
        EXPECT_EQ(mock_device.WriteFifoResponse(response), ZX_OK);
        groups.erase(group_id);
        // Wait for it to be done.
        fbl::AutoLock lock(&mutex);
        while (done != last_done + 1) {
          condition.Wait(&mutex);
        }
        finished = done == kThreadCount;
        break;
      }
    }
  }
  for (auto& thread : threads) {
    thread.join();
  }
}

TEST(RemoteBlockDeviceTest, NoHangForErrorsWithMultipleThreads) {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();

  std::unique_ptr<RemoteBlockDevice> device;
  constexpr int kThreadCount = 4 * MAX_TXN_GROUP_COUNT;
  std::thread threads[kThreadCount];

  {
    MockBlockDevice mock_device;
    mock_device.BindServer(std::move(server));

    {
      zx::result result = RemoteBlockDevice::Create(
          fidl::ClientEnd<fuchsia_storage_block::Block>{client.TakeChannel()});
      ASSERT_TRUE(result.is_ok()) << result.status_string();
      device = std::move(result.value());
    }

    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

    storage::OwnedVmoid vmoid;
    ASSERT_EQ(device->BlockAttachVmo(vmo, &vmoid.GetReference(device.get())), ZX_OK);
    ASSERT_EQ(kGoldenVmoid, vmoid.get());

    for (auto& thread : threads) {
      thread = std::thread([device = device.get(), vmoid = vmoid.get()]() {
        BlockFifoRequest request = {};
        request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
        request.vmoid = vmoid;
        request.length = 1;
        ASSERT_EQ(ZX_ERR_PEER_CLOSED, device->FifoTransaction(&request, 1));
      });
    }
    vmoid.TakeId();  // We don't need the vmoid any more.

    // Wait for at least 2 requests to be received.
    BlockFifoRequest requests[BLOCK_FIFO_MAX_DEPTH];
    size_t request_count = 0;
    while (request_count < 2) {
      size_t count = 0;
      ASSERT_EQ(mock_device.ReadFifoRequests(requests, &count), ZX_OK);
      request_count += count;
    }
  }

  // We should be able to join all the threads.
  for (auto& thread : threads) {
    thread.join();
  }
}

TEST(RemoteBlockDeviceTest, TransactionSanitizesPaddingAndReqids) {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();

  MockBlockDevice mock_device;
  mock_device.BindServer(std::move(server));

  zx::result device = RemoteBlockDevice::Create(
      fidl::ClientEnd<fuchsia_storage_block::Block>{client.TakeChannel()});
  ASSERT_TRUE(device.is_ok()) << device.status_string();

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

  storage::OwnedVmoid vmoid;
  ASSERT_EQ(device->BlockAttachVmo(vmo, &vmoid.GetReference(device.value().get())), ZX_OK);
  ASSERT_EQ(kGoldenVmoid, vmoid.get());

  alignas(BlockFifoRequest) uint8_t request_buffer[sizeof(BlockFifoRequest) * 2];
  memset(request_buffer, 0xAB, sizeof(request_buffer));
  BlockFifoRequest* requests = new (request_buffer) BlockFifoRequest[2]();

  requests[0].command.opcode = BLOCK_OPCODE_READ;
  requests[0].command.flags = 0;
  requests[0].vmoid = vmoid.get();
  requests[0].length = 1;
  requests[0].vmo_offset = 0;
  requests[0].dev_offset = 0;

  requests[1].command.opcode = BLOCK_OPCODE_READ;
  requests[1].command.flags = 0;
  requests[1].vmoid = vmoid.get();
  requests[1].length = 1;
  requests[1].vmo_offset = 0;
  requests[1].dev_offset = 1;

  std::thread server_thread([&mock_device] {
    BlockFifoRequest server_requests[2];
    size_t actual;
    EXPECT_EQ(mock_device.ReadFifoRequests(server_requests, &actual), ZX_OK);
    EXPECT_EQ(actual, 2u);

    for (BlockFifoRequest& server_request : server_requests) {
      EXPECT_EQ(server_request.command.padding_to_satisfy_zerocopy[0], 0u);
      EXPECT_EQ(server_request.command.padding_to_satisfy_zerocopy[1], 0u);
      EXPECT_EQ(server_request.command.padding_to_satisfy_zerocopy[2], 0u);
      EXPECT_EQ(server_request.padding, 0u);
      // Ensure reqid does not contain junk despite maybe being zero.
      EXPECT_NE(server_request.reqid, 0xABABABABu);
    }

    BlockFifoResponse response;
    response.status = ZX_OK;
    response.reqid = server_requests[1].reqid;
    response.group = server_requests[1].group;
    response.count = 1;
    EXPECT_EQ(mock_device.WriteFifoResponse(response), ZX_OK);
  });

  ASSERT_EQ(device->FifoTransaction(requests, 2), ZX_OK);
  vmoid.TakeId();
  server_thread.join();
}

}  // namespace
}  // namespace block_client
