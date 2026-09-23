// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "blktest.h"

#include <errno.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/zx/fifo.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
#include <zircon/syscalls.h>

#include <climits>
#include <iostream>
#include <iterator>
#include <limits>
#include <memory>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/unique_fd.h>
#include <pretty/hexdump.h>
#include <zxtest/zxtest.h>

#include "src/storage/lib/block_client/cpp/client.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace tests {

zx_status_t BRead(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, void* buffer,
                  size_t buffer_size, size_t offset) {
  return block_client::SingleReadBytes(device, buffer, buffer_size, offset);
}

zx_status_t BWrite(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, void* buffer,
                   size_t buffer_size, size_t offset) {
  return block_client::SingleWriteBytes(device, buffer, buffer_size, offset);
}

static void get_testdev(uint64_t* blk_size, uint64_t* blk_count,
                        fidl::ClientEnd<fuchsia_storage_block::Block>* client) {
  const char* blkdev_path = getenv(BLKTEST_BLK_DEV);
  ASSERT_NOT_NULL(blkdev_path, "No test device specified");
  // Open the block device
  zx::result block = component::Connect<fuchsia_storage_block::Block>(blkdev_path);
  ASSERT_OK(block.status_value());

  const fidl::WireResult result = fidl::WireCall(block.value())->GetInfo();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));

  *blk_size = response.value()->info.block_size;
  *blk_count = response.value()->info.block_count;
  *client = std::move(block.value());
}

TEST(BlkdevTests, blkdev_test_simple) {
  uint64_t blk_size, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));
  int64_t buffer_size = blk_size * 2;

  fbl::AllocChecker checker;
  std::unique_ptr<uint8_t[]> buf(new (&checker) uint8_t[buffer_size]);
  ASSERT_TRUE(checker.check());
  std::unique_ptr<uint8_t[]> out(new (&checker) uint8_t[buffer_size]);
  ASSERT_TRUE(checker.check());

  memset(buf.get(), 'a', buffer_size);
  memset(out.get(), 0, buffer_size);

  // Write three blocks.
  ASSERT_EQ(BWrite(client.borrow(), buf.get(), buffer_size, 0), ZX_OK);
  ASSERT_EQ(BWrite(client.borrow(), buf.get(), buffer_size / 2, buffer_size), ZX_OK);

  // Seek to the start of the device and read the contents
  ASSERT_EQ(BRead(client.borrow(), out.get(), buffer_size, 0), ZX_OK);
  ASSERT_EQ(memcmp(out.get(), buf.get(), buffer_size), 0);
  ASSERT_EQ(BRead(client.borrow(), out.get(), buffer_size / 2, buffer_size), ZX_OK);
  ASSERT_EQ(memcmp(out.get(), buf.get(), buffer_size / 2), 0);
}

TEST(BlkdevTests, blkdev_test_bad_requests) {
  uint64_t blk_size, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  fbl::AllocChecker checker;
  std::unique_ptr<uint8_t[]> buf(new (&checker) uint8_t[blk_size * 4]);
  ASSERT_TRUE(checker.check());
  memset(buf.get(), 'a', blk_size * 4);

  // Read / write non-multiples of the block size
  ASSERT_NE(BWrite(client.borrow(), buf.get(), blk_size - 1, 0), ZX_OK);
  ASSERT_NE(BWrite(client.borrow(), buf.get(), blk_size / 2, 0), ZX_OK);

  ASSERT_NE(BRead(client.borrow(), buf.get(), blk_size - 1, 0), ZX_OK);
  ASSERT_NE(BRead(client.borrow(), buf.get(), blk_size / 2, 0), ZX_OK);

  // Read / write from unaligned offset
  ASSERT_NE(BWrite(client.borrow(), buf.get(), blk_size, 1), ZX_OK);
  ASSERT_NE(BRead(client.borrow(), buf.get(), blk_size, 1), ZX_OK);

  // Read / write from beyond end of device
  off_t dev_size = blk_size * blk_count;
  ASSERT_NE(BWrite(client.borrow(), buf.get(), blk_size, dev_size), ZX_OK);
  ASSERT_NE(BRead(client.borrow(), buf.get(), blk_size, dev_size), ZX_OK);

  // Read / write starting within device but extending past end of device
  if (blk_count > 0) {
    off_t near_end = (blk_count - 1) * blk_size;
    ASSERT_NE(BWrite(client.borrow(), buf.get(), blk_size * 2, near_end), ZX_OK);
    ASSERT_NE(BRead(client.borrow(), buf.get(), blk_size * 2, near_end), ZX_OK);
  }
}

TEST(BlkdevTests, blkdev_test_fifo_no_op) {
  // Get a FIFO connection to a blkdev and immediately close it
  uint64_t blk_size, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();

  const fidl::Status result = fidl::WireCall(client)->OpenSession(std::move(server));
  ASSERT_OK(result.status());
}

static void fill_random(uint8_t* buf, uint64_t size) {
  for (size_t i = 0; i < size; i++) {
    buf[i] = static_cast<uint8_t>(rand());
  }
}

zx::result<std::pair<fidl::ClientEnd<fuchsia_storage_block::Session>, zx::fifo>> CreateRawSession(
    fidl::UnownedClientEnd<fuchsia_storage_block::Block> block) {
  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();

  const fidl::Status result = fidl::WireCall(block)->OpenSession(std::move(server));
  if (!result.ok()) {
    return zx::error(result.status());
  }

  const fidl::WireResult fifo_result = fidl::WireCall(session)->GetFifo();
  if (!fifo_result.ok()) {
    return zx::error(fifo_result.status());
  }
  const fit::result fifo_response = fifo_result.value();
  if (fifo_response.is_error()) {
    return zx::error(fifo_response.error_value());
  }
  return zx::ok(std::make_pair(std::move(session), std::move(fifo_response->fifo)));
}

zx::result<std::unique_ptr<block_client::Client>> CreateSession(
    fidl::UnownedClientEnd<fuchsia_storage_block::Block> block) {
  zx::result result = CreateRawSession(block);
  if (result.is_error()) {
    return result.take_error();
  }
  auto& [session, fifo] = result.value();
  return zx::ok(std::make_unique<block_client::Client>(std::move(session), std::move(fifo)));
}

TEST(BlkdevTests, blkdev_test_fifo_out_of_bounds) {
  uint64_t blk_size, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(blk_size * 2, 0, &vmo));
  zx::result vmoid_result = block_client.RegisterVmo(vmo);
  ASSERT_OK(vmoid_result);
  vmoid_t vmoid = vmoid_result->get();

  // Test 1: dev_offset = blk_count (starts past end of device)
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .group = 0,
      .vmoid = vmoid,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = blk_count,
  };
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  request.command = {.opcode = BLOCK_OPCODE_TRIM, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  // Test 2: dev_offset = blk_count - 1, length = 2 (starts inside, extends past end of device)
  if (blk_count > 0) {
    request = {
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
        .group = 0,
        .vmoid = vmoid,
        .length = 2,
        .vmo_offset = 0,
        .dev_offset = blk_count - 1,
    };
    ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

    request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
    ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

    request.command = {.opcode = BLOCK_OPCODE_TRIM, .flags = 0};
    ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);
  }

  // Test 3: dev_offset + length integer overflow (UINT64_MAX - 1, length 10)
  request = {
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .group = 0,
      .vmoid = vmoid,
      .length = 10,
      .vmo_offset = 0,
      .dev_offset = UINT64_MAX - 1,
  };
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_OK);
}

TEST(BlkdevTests, blkdev_test_fifo_basic) {
  uint64_t blk_size, blk_count;
  // Set up the initial handshake connection with the blkdev
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create an arbitrary VMO, fill it with some stuff
  const uint64_t vmo_size = blk_size * 3;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(vmo_size, 0, &vmo), ZX_OK, "Failed to create VMO");
  std::unique_ptr<uint8_t[]> buf(new uint8_t[vmo_size]);
  fill_random(buf.get(), vmo_size);

  ASSERT_EQ(vmo.write(buf.get(), 0, vmo_size), ZX_OK);

  // Send a handle to the vmo to the block device, get a vmoid which identifies it
  zx::result vmoid_result = block_client.RegisterVmo(vmo);
  ASSERT_OK(vmoid_result);
  vmoid_t vmoid = vmoid_result->get();

  // Batch write the VMO to the blkdev
  // Split it into two requests, spread across the disk
  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
          .group = group,
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
          .group = group,
          .vmoid = vmoid,
          .length = 2,
          .vmo_offset = 1,
          .dev_offset = 100,
      },
  };

  ASSERT_EQ(block_client.Transaction(requests, std::size(requests)), ZX_OK);

  // Empty the vmo, then read the info we just wrote to the disk
  std::unique_ptr<uint8_t[]> out(new uint8_t[vmo_size]());

  ASSERT_EQ(vmo.write(out.get(), 0, vmo_size), ZX_OK);
  requests[0].command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  requests[1].command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  ASSERT_EQ(block_client.Transaction(requests, std::size(requests)), ZX_OK);
  ASSERT_EQ(vmo.read(out.get(), 0, vmo_size), ZX_OK);
  ASSERT_EQ(memcmp(buf.get(), out.get(), blk_size * 3), 0, "Read data not equal to written data");

  // Close the current vmo
  requests[0].command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  ASSERT_EQ(block_client.Transaction(requests, 1), ZX_OK);
}

// TODO(https://fxbug.dev/42121033): enable.
//
// This test has been disabled since its introduction in 4ef35f3b8366d64cccb0fe6e240fb101238d7dfb.
TEST(BlkdevTests, DISABLED_blkdev_test_fifo_whole_disk) {
  uint64_t blk_size, blk_count;
  // Set up the initial handshake connection with the blkdev
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create an arbitrary VMO, fill it with some stuff
  uint64_t vmo_size = blk_size * blk_count;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(vmo_size, 0, &vmo), ZX_OK, "Failed to create VMO");
  std::unique_ptr<uint8_t[]> buf(new uint8_t[vmo_size]);
  fill_random(buf.get(), vmo_size);

  ASSERT_EQ(vmo.write(buf.get(), 0, vmo_size), ZX_OK);

  // Send a handle to the vmo to the block device, get a vmoid which identifies it
  zx::result vmoid_result = block_client.RegisterVmo(vmo);
  ASSERT_OK(vmoid_result);
  vmoid_t vmoid = vmoid_result->get();

  // Batch write the VMO to the blkdev
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
      .group = group,
      .vmoid = vmoid,
      .length = static_cast<uint32_t>(blk_count),
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_OK);

  // Empty the vmo, then read the info we just wrote to the disk
  std::unique_ptr<uint8_t[]> out(new uint8_t[vmo_size]());

  ASSERT_EQ(vmo.write(out.get(), 0, vmo_size), ZX_OK);
  request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_OK);
  ASSERT_EQ(vmo.read(out.get(), 0, vmo_size), ZX_OK);
  ASSERT_EQ(memcmp(buf.get(), out.get(), blk_size * 3), 0, "Read data not equal to written data");

  // Close the current vmo
  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_OK);
}

struct TestVmoObject {
  uint64_t vmo_size = 0;
  zx::vmo vmo;
  fuchsia_storage_block::wire::VmoId vmoid;
  std::unique_ptr<uint8_t[]> buf;

  void RandomizeVmo(size_t block_size) {
    vmo_size = block_size + (rand() % 5) * block_size;
    ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo));
    buf.reset(new uint8_t[vmo_size]);
    fill_random(buf.get(), vmo_size);
    ASSERT_OK(vmo.write(buf.get(), 0, vmo_size));
  }
};

// Creates a VMO, fills it with data, and gives it to the block device.
void CreateVmoHelper(block_client::Client& block_client, TestVmoObject& obj, size_t block_size) {
  ASSERT_NO_FATAL_FAILURE(obj.RandomizeVmo(block_size));
  zx::result vmoid = block_client.RegisterVmo(obj.vmo);
  ASSERT_OK(vmoid);
  // Intentional leak
  obj.vmoid.id = vmoid.value().TakeId();
}

// Write all vmos in a striped pattern on disk.
// For objs.size() == 10,
// i = 0 will write vmo block 0, 1, 2, 3... to dev block 0, 10, 20, 30...
// i = 1 will write vmo block 0, 1, 2, 3... to dev block 1, 11, 21, 31...
void WriteStripedVmoHelper(block_client::Client& block_client, const TestVmoObject& obj, size_t i,
                           size_t objs, groupid_t group, size_t kBlockSize) {
  // Make a separate request for each block
  size_t blocks = obj.vmo_size / kBlockSize;
  std::vector<BlockFifoRequest> requests(blocks);
  for (size_t b = 0; b < blocks; b++) {
    requests[b].group = group;
    requests[b].vmoid = obj.vmoid.id;
    requests[b].command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
    requests[b].length = 1;
    requests[b].vmo_offset = b;
    requests[b].dev_offset = i + b * objs;
  }
  // Write entire vmos at once
  ASSERT_EQ(block_client.Transaction(requests.data(), requests.size()), ZX_OK);
}

// Verifies the result from "WriteStripedVmoHelper"
void ReadStripedVmoHelper(block_client::Client& block_client, const TestVmoObject& obj, size_t i,
                          size_t objs, groupid_t group, size_t kBlockSize) {
  // First, empty out the VMO
  std::unique_ptr<uint8_t[]> out(new uint8_t[obj.vmo_size]());
  ASSERT_EQ(obj.vmo.write(out.get(), 0, obj.vmo_size), ZX_OK);

  // Next, read to the vmo from the disk
  size_t blocks = obj.vmo_size / kBlockSize;
  std::vector<BlockFifoRequest> requests(blocks);
  for (size_t b = 0; b < blocks; b++) {
    requests[b].group = group;
    requests[b].vmoid = obj.vmoid.id;
    requests[b].command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
    requests[b].length = 1;
    requests[b].vmo_offset = b;
    requests[b].dev_offset = i + b * objs;
  }

  // Read entire vmos at once
  ASSERT_EQ(block_client.Transaction(requests.data(), requests.size()), ZX_OK);

  // Finally, write from the vmo to an out buffer, where we can compare
  // the results with the input buffer.
  ASSERT_EQ(obj.vmo.read(out.get(), 0, obj.vmo_size), ZX_OK);
  ASSERT_EQ(memcmp(obj.buf.get(), out.get(), obj.vmo_size), 0,
            "Read data not equal to written data");
}

// Tears down an object created by "CreateVmoHelper".
void CloseVmoHelper(block_client::Client& block_client, TestVmoObject& obj, groupid_t group) {
  BlockFifoRequest request;
  request.group = group;
  request.vmoid = obj.vmoid.id;
  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_OK);
  obj.vmo.reset();
}

TEST(BlkdevTests, blkdev_test_fifo_multiple_vmo) {
  // Set up the initial handshake connection with the blkdev
  uint64_t blk_size, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create multiple VMOs
  std::vector<TestVmoObject> objs(10);
  for (auto& obj : objs) {
    ASSERT_NO_FATAL_FAILURE(CreateVmoHelper(block_client, obj, blk_size));
  }

  for (size_t i = 0; i < objs.size(); i++) {
    ASSERT_NO_FATAL_FAILURE(
        WriteStripedVmoHelper(block_client, objs[i], i, objs.size(), group, blk_size));
  }

  for (size_t i = 0; i < objs.size(); i++) {
    ASSERT_NO_FATAL_FAILURE(
        ReadStripedVmoHelper(block_client, objs[i], i, objs.size(), group, blk_size));
  }

  for (auto& obj : objs) {
    ASSERT_NO_FATAL_FAILURE(CloseVmoHelper(block_client, obj, group));
  }
}

TEST(BlkdevTests, blkdev_test_fifo_multiple_vmo_multithreaded) {
  // Set up the initial handshake connection with the blkdev
  uint64_t kBlockSize, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&kBlockSize, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  // Create multiple VMOs
  constexpr size_t kNumThreads = MAX_TXN_GROUP_COUNT;
  std::vector<TestVmoObject> objs(kNumThreads);

  std::vector<std::thread> threads;
  for (size_t i = 0; i < kNumThreads; i++) {
    // Capture i by value to get the updated version each loop iteration.
    threads.emplace_back([&, i]() {
      groupid_t group = static_cast<groupid_t>(i);
      ASSERT_NO_FATAL_FAILURE(CreateVmoHelper(block_client, objs[i], kBlockSize));
      ASSERT_NO_FATAL_FAILURE(
          WriteStripedVmoHelper(block_client, objs[i], i, objs.size(), group, kBlockSize));
      ASSERT_NO_FATAL_FAILURE(
          ReadStripedVmoHelper(block_client, objs[i], i, objs.size(), group, kBlockSize));
      ASSERT_NO_FATAL_FAILURE(CloseVmoHelper(block_client, objs[i], group));
    });
  }

  for (auto& thread : threads) {
    thread.join();
  }
}

TEST(BlkdevTests, blkdev_test_fifo_unclean_shutdown) {
  // Set up the blkdev
  uint64_t kBlockSize, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&kBlockSize, &blk_count, &client));

  std::vector<TestVmoObject> objs(10);
  groupid_t group = 0;
  {
    // We intentionally create a raw session which does not clean up when going out of scope, so we
    // trigger asynchronous cleanup.
    zx::result result = CreateRawSession(client);
    ASSERT_OK(result);
    auto& [session, fifo] = result.value();

    // Create multiple VMOs.  We intentionally leak the vmoids, since once |session| goes out of
    // scope the block server should clean the session up.
    for (auto& obj : objs) {
      ASSERT_NO_FATAL_FAILURE(obj.RandomizeVmo(kBlockSize));
      zx::vmo dup;
      ASSERT_OK(obj.vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
      const fidl::WireResult result = fidl::WireCall(session)->AttachVmo(std::move(dup));
      ASSERT_OK(result);
      const fit::result response = result.value();
      ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    }
  }

  // The block server should still be functioning. We should be able to re-bind to it.
  {
    zx::result block_client_ptr = CreateSession(client);
    ASSERT_OK(block_client_ptr);
    block_client::Client& block_client = *block_client_ptr.value();

    for (auto& obj : objs) {
      ASSERT_NO_FATAL_FAILURE(CreateVmoHelper(block_client, obj, kBlockSize));
    }
    for (size_t i = 0; i < objs.size(); i++) {
      ASSERT_NO_FATAL_FAILURE(
          WriteStripedVmoHelper(block_client, objs[i], i, objs.size(), group, kBlockSize));
    }
    for (size_t i = 0; i < objs.size(); i++) {
      ASSERT_NO_FATAL_FAILURE(
          ReadStripedVmoHelper(block_client, objs[i], i, objs.size(), group, kBlockSize));
    }
    for (auto& obj : objs) {
      ASSERT_NO_FATAL_FAILURE(CloseVmoHelper(block_client, obj, group));
    }
  }
}

TEST(BlkdevTests, blkdev_test_fifo_bad_client_vmoid) {
  // Try to flex the server's error handling by sending 'malicious' client requests.
  // Set up the blkdev
  uint64_t kBlockSize, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&kBlockSize, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create a vmo
  TestVmoObject obj;
  ASSERT_NO_FATAL_FAILURE(CreateVmoHelper(block_client, obj, kBlockSize));

  // Bad request: Writing to the wrong vmoid
  BlockFifoRequest request;
  request.group = group;
  request.vmoid = static_cast<vmoid_t>(obj.vmoid.id + 5);
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
  request.length = 1;
  request.vmo_offset = 0;
  request.dev_offset = 0;
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_IO, "Expected IO error with bad vmoid");
}

TEST(BlkdevTests, blkdev_test_fifo_bad_client_unaligned_request) {
  // Try to flex the server's error handling by sending 'malicious' client requests.
  // Set up the blkdev
  uint64_t kBlockSize, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&kBlockSize, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create a vmo of at least size "kBlockSize * 2", since we'll
  // be reading "kBlockSize" bytes from an offset below, and we want it
  // to fit within the bounds of the VMO.
  TestVmoObject obj;
  ASSERT_NO_FATAL_FAILURE(CreateVmoHelper(block_client, obj, kBlockSize * 2));

  BlockFifoRequest request;
  request.group = group;
  request.vmoid = static_cast<vmoid_t>(obj.vmoid.id);
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};

  // Send a request that has zero length
  request.length = 0;
  request.vmo_offset = 0;
  request.dev_offset = 0;
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_INVALID_ARGS, "");
}

TEST(BlkdevTests, blkdev_test_barriers) {
  uint64_t blk_size, blk_count;
  // Set up the initial handshake connection with the blkdev
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create an arbitrary VMO, fill it with some stuff
  const uint64_t vmo_size = blk_size;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo), "Failed to create VMO");
  std::unique_ptr<uint8_t[]> random_buf(new uint8_t[vmo_size]);
  fill_random(random_buf.get(), vmo_size);

  ASSERT_OK(vmo.write(random_buf.get(), 0, vmo_size));

  // Send a handle to the vmo to the block device, get a vmoid which identifies it
  zx::result vmoid_result = block_client.RegisterVmo(vmo);
  ASSERT_OK(vmoid_result);
  vmoid_t vmoid = vmoid_result->get();

  // Write the VMO to the blkdev with a barrier.
  BlockFifoRequest request;
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_PRE_BARRIER};
  request.group = group;
  request.vmoid = vmoid;
  request.length = 1;
  request.vmo_offset = 0;
  request.dev_offset = 0;

  ASSERT_OK(block_client.Transaction(&request, 1));

  // Empty the vmo, then read the info we just wrote to the disk
  std::unique_ptr<uint8_t[]> out(new uint8_t[vmo_size]());
  // Fill the buffer with known data
  memset(out.get(), 0xaa, vmo_size);

  // Read and verify blkdev.
  ASSERT_OK(vmo.write(out.get(), 0, vmo_size));
  request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  ASSERT_OK(block_client.Transaction(&request, 1));
  ASSERT_OK(vmo.read(out.get(), 0, vmo_size));
  ASSERT_EQ(memcmp(random_buf.get(), out.get(), vmo_size), 0,
            "Read data not equal to written data");

  // Close the current vmo
  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  ASSERT_OK(block_client.Transaction(&request, 1));
}

TEST(BlkdevTests, blkdev_test_force_access) {
  uint64_t blk_size, blk_count;
  // Set up the initial handshake connection with the blkdev
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create an arbitrary VMO, fill it with some stuff
  const uint64_t vmo_size = blk_size;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo), "Failed to create VMO");
  std::unique_ptr<uint8_t[]> random_buf(new uint8_t[vmo_size]);
  fill_random(random_buf.get(), vmo_size);

  ASSERT_OK(vmo.write(random_buf.get(), 0, vmo_size));

  // Send a handle to the vmo to the block device, get a vmoid which identifies it
  zx::result vmoid_result = block_client.RegisterVmo(vmo);
  ASSERT_OK(vmoid_result);
  vmoid_t vmoid = vmoid_result->get();

  // Write the VMO to the blkdev with FUA set.
  BlockFifoRequest request;
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS};
  request.group = group;
  request.vmoid = vmoid;
  request.length = 1;
  request.vmo_offset = 0;
  request.dev_offset = 0;

  ASSERT_OK(block_client.Transaction(&request, 1));

  // Empty the vmo, then read the info we just wrote to the disk
  std::unique_ptr<uint8_t[]> out(new uint8_t[vmo_size]());
  // Fill the buffer with known data
  memset(out.get(), 0xaa, vmo_size);

  // Read and verify blkdev.
  ASSERT_OK(vmo.write(out.get(), 0, vmo_size));
  request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  ASSERT_OK(block_client.Transaction(&request, 1));
  ASSERT_OK(vmo.read(out.get(), 0, vmo_size));
  ASSERT_EQ(memcmp(random_buf.get(), out.get(), vmo_size), 0,
            "Read data not equal to written data");

  // Close the current vmo
  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  ASSERT_OK(block_client.Transaction(&request, 1));
}

TEST(BlkdevTests, blkdev_test_fifo_bad_client_overflow) {
  // Try to flex the server's error handling by sending 'malicious' client requests.
  // Set up the blkdev
  uint64_t kBlockSize, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&kBlockSize, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create a vmo of at least size "kBlockSize * 2", since we'll
  // be reading "kBlockSize" bytes from an offset below, and we want it
  // to fit within the bounds of the VMO.
  TestVmoObject obj;
  ASSERT_NO_FATAL_FAILURE(CreateVmoHelper(block_client, obj, kBlockSize * 2));

  BlockFifoRequest request;
  request.group = group;
  request.vmoid = static_cast<vmoid_t>(obj.vmoid.id);
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};

  // Send a request that is barely out-of-bounds for the device
  request.length = 1;
  request.vmo_offset = 0;
  request.dev_offset = blk_count;
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  // Send a request that is half out-of-bounds for the device
  request.length = 2;
  request.vmo_offset = 0;
  request.dev_offset = blk_count - 1;
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  // Send a request that is very out-of-bounds for the device
  request.length = 1;
  request.vmo_offset = 0;
  request.dev_offset = blk_count + 1;
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  // Send a request that tries to overflow the VMO
  request.length = 2;
  request.vmo_offset = std::numeric_limits<uint64_t>::max();
  request.dev_offset = 0;
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  // Send a request that tries to overflow the device
  request.length = 2;
  request.vmo_offset = 0;
  request.dev_offset = std::numeric_limits<uint64_t>::max();
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);
}

TEST(BlkdevTests, blkdev_test_fifo_bad_client_bad_vmo) {
  // Try to flex the server's error handling by sending 'malicious' client requests.
  // Set up the blkdev
  uint64_t kBlockSize, blk_count;
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&kBlockSize, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create a vmo of one block.
  //
  // The underlying VMO may be rounded up to the nearest zx_system_get_page_size().
  TestVmoObject obj;
  obj.vmo_size = kBlockSize;
  ASSERT_EQ(zx::vmo::create(obj.vmo_size, 0, &obj.vmo), ZX_OK, "Failed to create vmo");
  obj.buf.reset(new uint8_t[obj.vmo_size]);
  fill_random(obj.buf.get(), obj.vmo_size);
  ASSERT_EQ(obj.vmo.write(obj.buf.get(), 0, obj.vmo_size), ZX_OK, "Failed to write to vmo");

  {
    zx::result vmoid = block_client.RegisterVmo(obj.vmo);
    ASSERT_OK(vmoid);
    // Intentional leak
    obj.vmoid.id = vmoid.value().TakeId();
  }

  // Send a request to write to write multiple blocks -- enough that
  // the request is larger than the VMO.
  const uint64_t length =
      1 +
      (fbl::round_up(obj.vmo_size, static_cast<uint64_t>(zx_system_get_page_size())) / kBlockSize);
  BlockFifoRequest request;
  request.group = group;
  request.vmoid = static_cast<vmoid_t>(obj.vmoid.id);
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
  request.length = static_cast<uint32_t>(length);
  request.vmo_offset = 0;
  request.dev_offset = 0;
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);
  // Do the same thing, but for reading
  request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  ASSERT_EQ(block_client.Transaction(&request, 1), ZX_ERR_OUT_OF_RANGE);
}

TEST(BlkdevTests, blkdev_test_fifo_trim) {
  uint64_t blk_size, blk_count;
  // Set up the initial handshake connection with the blkdev
  fidl::ClientEnd<fuchsia_storage_block::Block> client;
  ASSERT_NO_FATAL_FAILURE(get_testdev(&blk_size, &blk_count, &client));

  zx::result block_client_ptr = CreateSession(client);
  ASSERT_OK(block_client_ptr);
  block_client::Client& block_client = *block_client_ptr.value();

  groupid_t group = 0;

  // Create an arbitrary VMO, fill it with some stuff
  const uint64_t vmo_size = blk_size;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &vmo), "Failed to create VMO");
  std::unique_ptr<uint8_t[]> random_buf(new uint8_t[vmo_size]);
  fill_random(random_buf.get(), vmo_size);

  ASSERT_OK(vmo.write(random_buf.get(), 0, vmo_size));

  // Send a handle to the vmo to the block device, get a vmoid which identifies it
  zx::result vmoid_result = block_client.RegisterVmo(vmo);
  ASSERT_OK(vmoid_result);
  vmoid_t vmoid = vmoid_result->get();

  // Write the VMO to the blkdev
  BlockFifoRequest request;
  request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
  request.group = group;
  request.vmoid = vmoid;
  request.length = 1;
  request.vmo_offset = 0;
  request.dev_offset = 0;
  ASSERT_OK(block_client.Transaction(&request, 1));

  // Empty the vmo, then read the info we just wrote to the disk
  std::unique_ptr<uint8_t[]> out(new uint8_t[vmo_size]());
  // Fill the buffer with known data
  memset(out.get(), 0xaa, vmo_size);

  // Read and verify blkdev.
  ASSERT_OK(vmo.write(out.get(), 0, vmo_size));
  request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
  ASSERT_OK(block_client.Transaction(&request, 1));
  ASSERT_OK(vmo.read(out.get(), 0, vmo_size));
  ASSERT_EQ(memcmp(random_buf.get(), out.get(), vmo_size), 0,
            "Read data not equal to written data");

  // Send TRIM command
  request.command = {.opcode = BLOCK_OPCODE_TRIM, .flags = 0};
  zx_status_t trim_status = block_client.Transaction(&request, 1);

  // Check for data consistency if the device supports the TRIM command.
  if (trim_status != ZX_ERR_NOT_SUPPORTED) {
    ASSERT_OK(trim_status);

    // Fill the vmo with random data because we will be reading a trimmed block filled with zeros.
    ASSERT_OK(vmo.write(random_buf.get(), 0, vmo_size));

    // Read and verify blkdev. The trimmed buffer should be filled with zeros.
    request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
    ASSERT_OK(block_client.Transaction(&request, 1));
    ASSERT_OK(vmo.read(out.get(), 0, vmo_size));

    if (memcmp(random_buf.get(), out.get(), vmo_size) == 0) {
      // The data returned for unmapped blocks depends on the LBPRZ field of the Logical Block
      // Provisioning VPD page. For now, we handle the case where some devices ignore it (e.g.,
      // virtio-scsi with `discard=ignore` option).
      std::cerr << "[  WARNING ] "
                << "The TRIM command was issued, but the blocks were not trimmed. " << std::endl;
    } else {
      // Depending on device characteristics, reading an unmapped area will return 0xff or 0x0.
      ASSERT_TRUE(out.get()[0] == 0x0 || out.get()[0] == 0xff, "Invalid trimmed data[0] = [0x%x]",
                  out.get()[0]);

      // Initialize check_buffer values to 0x0 or 0xff
      std::unique_ptr<uint8_t[]> check_buf(new uint8_t[vmo_size]());
      memset(check_buf.get(), out.get()[0], vmo_size);

      ASSERT_EQ(memcmp(check_buf.get(), out.get(), vmo_size), 0,
                "Read data not equal to trimmed data(0x%x)", out.get()[0]);
    }
  }

  // Close the current vmo
  request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
  ASSERT_OK(block_client.Transaction(&request, 1));
}

}  // namespace tests
