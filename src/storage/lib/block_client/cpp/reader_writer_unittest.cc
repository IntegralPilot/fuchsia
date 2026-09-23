// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_client/cpp/reader_writer.h"

#include <gtest/gtest.h>

#include "src/storage/lib/block_client/cpp/fake_block_device.h"

namespace block_client {
namespace {

void CreateAndRegisterVmo(BlockDevice& device, size_t size, zx::vmo& vmo,
                          storage::OwnedVmoid& vmoid) {
  fuchsia_storage_block::wire::BlockInfo info = {};
  ASSERT_EQ(device.BlockGetInfo(&info), ZX_OK);
  ASSERT_EQ(zx::vmo::create(size, 0, &vmo), ZX_OK);
  ASSERT_EQ(device.BlockAttachVmo(vmo, &vmoid.GetReference(&device)), ZX_OK);
}

TEST(ReaderTest, Read) {
  const uint64_t kBlockCount = 2048;
  const uint32_t kBlockSize = 512;

  FakeBlockDevice device(kBlockCount, kBlockSize);

  const uint64_t kBufferSize = 1024 * 1024;
  zx::vmo vmo;
  storage::OwnedVmoid vmoid;
  ASSERT_NO_FATAL_FAILURE(CreateAndRegisterVmo(device, kBufferSize, vmo, vmoid));

  std::vector<uint8_t> buf(kBufferSize);

  for (unsigned i = 0; i < kBufferSize; ++i)
    buf[i] = static_cast<uint8_t>(i * 17);

  ASSERT_EQ(vmo.write(buf.data(), 0, buf.size()), ZX_OK);

  BlockFifoRequest request{
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
      .vmoid = vmoid.get(),
      .length = kBufferSize / kBlockSize,
  };
  ASSERT_EQ(device.FifoTransaction(&request, 1), ZX_OK);

  ReaderWriter reader(device);
  std::vector<uint8_t> read_buf(kBufferSize);
  ASSERT_EQ(reader.Read(0, kBufferSize, read_buf.data()), ZX_OK);

  EXPECT_EQ(read_buf, buf);
}

TEST(ReaderTest, ReadVmo) {
  const uint64_t kBlockCount = 2048;
  const uint32_t kBlockSize = 512;

  FakeBlockDevice device(kBlockCount, kBlockSize);

  const uint64_t kBufferSize = 1024 * 1024;
  zx::vmo vmo;
  storage::OwnedVmoid vmoid;
  ASSERT_NO_FATAL_FAILURE(CreateAndRegisterVmo(device, kBufferSize, vmo, vmoid));

  std::vector<uint8_t> buf(kBufferSize);

  for (unsigned i = 0; i < kBufferSize; ++i)
    buf[i] = static_cast<uint8_t>(i * 17);

  ASSERT_EQ(vmo.write(buf.data(), 0, buf.size()), ZX_OK);

  BlockFifoRequest request{
      .command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0},
      .vmoid = vmoid.get(),
      .length = kBufferSize / kBlockSize,
  };
  ASSERT_EQ(device.FifoTransaction(&request, 1), ZX_OK);

  ReaderWriter reader(device);
  zx::vmo out_vmo;
  ASSERT_EQ(zx::vmo::create(kBufferSize, 0, &out_vmo), ZX_OK);
  ASSERT_EQ(reader.Read(0, kBufferSize, out_vmo, 0), ZX_OK);

  std::vector<uint8_t> read_buf(kBufferSize);
  ASSERT_EQ(out_vmo.read(read_buf.data(), 0, kBufferSize), ZX_OK);

  EXPECT_EQ(read_buf, buf);
}

TEST(WriterTest, Write) {
  const uint64_t kBlockCount = 2048;
  const uint32_t kBlockSize = 512;

  FakeBlockDevice device(kBlockCount, kBlockSize);
  ReaderWriter writer(device);

  const uint64_t kBufferSize = 1024 * 1024;
  zx::vmo vmo;
  storage::OwnedVmoid vmoid;
  ASSERT_NO_FATAL_FAILURE(CreateAndRegisterVmo(device, kBufferSize, vmo, vmoid));

  std::vector<uint8_t> buf(kBufferSize);

  for (unsigned i = 0; i < kBufferSize; ++i)
    buf[i] = static_cast<uint8_t>(i * 17);

  ASSERT_EQ(writer.Write(0, kBufferSize, buf.data()), ZX_OK);

  BlockFifoRequest request{
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .vmoid = vmoid.get(),
      .length = kBufferSize / kBlockSize,
  };
  ASSERT_EQ(device.FifoTransaction(&request, 1), ZX_OK);

  std::vector<uint8_t> read_buf(kBufferSize);
  ASSERT_EQ(vmo.read(read_buf.data(), 0, read_buf.size()), ZX_OK);

  EXPECT_EQ(read_buf, buf);
}

TEST(WriterTest, WriteVmo) {
  const uint64_t kBlockCount = 2048;
  const uint32_t kBlockSize = 512;

  FakeBlockDevice device(kBlockCount, kBlockSize);
  ReaderWriter writer(device);

  const uint64_t kBufferSize = 1024 * 1024;
  zx::vmo vmo;
  storage::OwnedVmoid vmoid;
  ASSERT_NO_FATAL_FAILURE(CreateAndRegisterVmo(device, kBufferSize, vmo, vmoid));

  std::vector<uint8_t> buf(kBufferSize);

  for (unsigned i = 0; i < kBufferSize; ++i)
    buf[i] = static_cast<uint8_t>(i * 17);
  zx::vmo write_vmo;
  ASSERT_EQ(zx::vmo::create(kBufferSize, 0, &write_vmo), ZX_OK);
  ASSERT_EQ(write_vmo.write(buf.data(), 0, kBufferSize), ZX_OK);

  ASSERT_EQ(writer.Write(0, kBufferSize, write_vmo, 0), ZX_OK);

  BlockFifoRequest request{
      .command = {.opcode = BLOCK_OPCODE_READ, .flags = 0},
      .vmoid = vmoid.get(),
      .length = kBufferSize / kBlockSize,
  };
  ASSERT_EQ(device.FifoTransaction(&request, 1), ZX_OK);

  std::vector<uint8_t> read_buf(kBufferSize);
  ASSERT_EQ(vmo.read(read_buf.data(), 0, read_buf.size()), ZX_OK);

  EXPECT_EQ(read_buf, buf);
}

TEST(ReaderWriterTest, InvalidBlockSizes) {
  // Test with block size of 0
  {
    FakeBlockDevice device(1024, 0);
    ReaderWriter reader_writer(device);
    std::vector<uint8_t> buf(512);
    EXPECT_EQ(reader_writer.Read(0, 512, buf.data()), ZX_ERR_INVALID_ARGS);
  }

  // Test with non-power-of-two block size
  {
    FakeBlockDevice device(1024, 513);
    ReaderWriter reader_writer(device);
    std::vector<uint8_t> buf(513);
    EXPECT_EQ(reader_writer.Read(0, 513, buf.data()), ZX_ERR_INVALID_ARGS);
  }
}

TEST(ReaderWriterTest, LargeValidBlockSize) {
  // Test with block size 64 KB (valid and <= kMaxBlockSize = 64 KB)
  {
    const uint32_t kBlockSize = 64 * 1024;
    FakeBlockDevice device(10, kBlockSize);
    ReaderWriter reader_writer(device);
    std::vector<uint8_t> buf(kBlockSize);
    EXPECT_EQ(reader_writer.Write(0, kBlockSize, buf.data()), ZX_OK);
  }

  // Test with block size 128 KB (exceeds kMaxBlockSize = 64 KB)
  {
    const uint32_t kBlockSize = 128 * 1024;
    FakeBlockDevice device(10, kBlockSize);
    ReaderWriter reader_writer(device);
    std::vector<uint8_t> buf(kBlockSize);
    EXPECT_EQ(reader_writer.Write(0, kBlockSize, buf.data()), ZX_ERR_NOT_SUPPORTED);
  }
}

}  // namespace
}  // namespace block_client
